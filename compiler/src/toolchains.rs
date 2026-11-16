//! Per-chain project scaffolding and build commands.
//!
//! Each chain gets a throwaway project directory holding the user's single
//! source file plus a manifest this service writes. Only the source file
//! comes from the user, so dependencies, build scripts and compiler flags are
//! always ours.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    Sui,
    Aptos,
    Stellar,
    Solana,
    Starknet,
}

impl Chain {
    pub const ALL: [Chain; 5] = [Chain::Sui, Chain::Aptos, Chain::Stellar, Chain::Solana, Chain::Starknet];

    /// The executable whose presence decides whether this chain can compile here.
    pub fn tool(self) -> &'static str {
        match self {
            Chain::Sui => "sui",
            Chain::Aptos => "aptos",
            Chain::Stellar => "stellar",
            Chain::Solana => "cargo-build-sbf",
            Chain::Starknet => "scarb",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CompileRequest {
    pub chain: Chain,
    pub source: String,
    /// Contract or package name. Sanitized before it reaches any manifest.
    #[serde(default)]
    pub name: String,
    /// Solana: base58 program id written into `declare_id!`, so the binary
    /// matches the program account the browser will deploy to.
    #[serde(default)]
    pub program_id: Option<String>,
    /// Aptos: the publishing account, bound to the package's named address.
    #[serde(default)]
    pub sender: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CompileResponse {
    pub ok: bool,
    pub logs: String,
    /// Chain-specific build output; absent when `ok` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error("{0}")]
    Rejected(String),
    #[error("The {0} toolchain is not installed on this compiler host.")]
    MissingToolchain(&'static str),
    #[error("The build took longer than {0} seconds and was stopped.")]
    Timeout(u64),
    #[error("{0}")]
    Internal(String),
}

pub const MAX_SOURCE_BYTES: usize = 200 * 1024;

/// Rust and Move macros that read files or environment variables at compile
/// time. A source using them could pull host files into the artifact, so
/// they are refused outright. `contractimport!` (Soroban) and
/// `declare_program!` (Anchor) read files by path too.
fn forbidden_patterns(chain: Chain) -> Vec<(Regex, &'static str)> {
    let rust = [
        (r"\binclude(_str|_bytes)?\s*!", "include!/include_str!/include_bytes!"),
        (r"\b(option_)?env\s*!", "env!/option_env!"),
        (r"#\s*!?\s*\[\s*path\b", "#[path]"),
        (r"\bmod\s+\w+\s*;", "file modules (`mod name;`). Put everything in one file"),
        (r"\bcontractimport\s*!", "contractimport!"),
        (r"\bdeclare_program\s*!", "declare_program!"),
        (r"\bextern\s+crate\b", "extern crate"),
    ];
    match chain {
        Chain::Stellar | Chain::Solana => rust
            .iter()
            .map(|(p, why)| (Regex::new(p).expect("valid regex"), *why))
            .collect(),
        Chain::Starknet => vec![(
            Regex::new(r"\bmod\s+\w+\s*;").expect("valid regex"),
            "file modules (`mod name;`). Put everything in one file",
        )],
        Chain::Sui | Chain::Aptos => vec![],
    }
}

pub fn check_source(chain: Chain, source: &str) -> Result<(), CompileError> {
    if source.trim().is_empty() {
        return Err(CompileError::Rejected("The source is empty.".into()));
    }
    if source.len() > MAX_SOURCE_BYTES {
        return Err(CompileError::Rejected(format!(
            "The source is {} KB; the limit is {} KB.",
            source.len() / 1024,
            MAX_SOURCE_BYTES / 1024
        )));
    }
    for (re, why) in forbidden_patterns(chain) {
        if re.is_match(source) {
            return Err(CompileError::Rejected(format!(
                "This compiler does not allow {why}: it could read files on the build server."
            )));
        }
    }
    Ok(())
}

/// Lowercase snake_case identifier safe for Cargo, Scarb and Move manifests.
pub fn package_name(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    match out.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => out.chars().take(40).collect(),
        _ => "contract".into(),
    }
}

pub struct Runner {
    pub timeout: Duration,
    /// Parallel rustc jobs per build (CARGO_BUILD_JOBS).
    pub build_jobs: usize,
}

impl Runner {
    /// Runs `program` in `dir` with a minimal environment. Returns combined
    /// output and whether it exited successfully.
    async fn run(&self, dir: &Path, program: &str, args: &[&str]) -> Result<(bool, String), CompileError> {
        self.run_env(dir, program, args, &[]).await
    }

    async fn run_env(
        &self,
        dir: &Path,
        program: &str,
        args: &[&str],
        extra_env: &[(&str, &Path)],
    ) -> Result<(bool, String), CompileError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(dir)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own process group, so the cargo and rustc processes a build spawns
        // can be killed with it (see `kill_group`).
        #[cfg(unix)]
        cmd.process_group(0);
        // Toolchains need their install and cache locations, nothing else.
        for key in ["PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME", "XDG_CACHE_HOME", "SCARB_CACHE", "LANG", "MOVE_HOME", "SUI_CONFIG_DIR"] {
            if let Ok(v) = std::env::var(key) {
                cmd.env(key, v);
            }
        }
        cmd.env("CARGO_TERM_COLOR", "never").env("NO_COLOR", "1");
        // Each rustc job can take 1 GB or more; cargo defaults to one job per CPU.
        cmd.env("CARGO_BUILD_JOBS", self.build_jobs.to_string());
        for (key, value) in extra_env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CompileError::MissingToolchain(leak_name(program))
            } else {
                CompileError::Internal(format!("Could not start {program}: {e}"))
            }
        })?;
        let pid = child.id();
        let stdout = child.stdout.take().map(|s| tokio::spawn(read_capped(s, MAX_LOG_BYTES)));
        let stderr = child.stderr.take().map(|s| tokio::spawn(read_capped(s, MAX_LOG_BYTES)));

        let status = tokio::time::timeout(self.timeout, child.wait()).await;
        // Always, not only on timeout: build tools can leave children running
        // after the top process exits, and they would keep holding memory.
        if let Some(pid) = pid {
            kill_group(pid);
        }
        let _ = child.kill().await;

        let mut logs = String::new();
        for reader in [stdout, stderr].into_iter().flatten() {
            // Every writer is dead, so the pipes close; the bound covers a
            // process that escaped the group and still holds one open.
            if let Ok(Ok(bytes)) = tokio::time::timeout(Duration::from_secs(5), reader).await {
                logs.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
        let status = status
            .map_err(|_| CompileError::Timeout(self.timeout.as_secs()))?
            .map_err(|e| CompileError::Internal(e.to_string()))?;
        Ok((status.success(), logs))
    }
}

/// Most output kept per stream. A build can print far more (macro errors,
/// a chatty dependency); the rest is read and dropped.
const MAX_LOG_BYTES: usize = 256 * 1024;

async fn read_capped<R: AsyncRead + Unpin>(mut reader: R, cap: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut dropped = 0usize;
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..n.min(room)]);
                dropped += n.saturating_sub(room);
            }
        }
    }
    if dropped > 0 {
        kept.extend_from_slice(format!("\n[{dropped} more bytes of output dropped]\n").as_bytes());
    }
    kept
}

/// SIGKILL to the whole process group the build started.
fn kill_group(pid: u32) {
    #[cfg(unix)]
    if let Ok(pgid) = i32::try_from(pid) {
        // SAFETY: killpg only sends a signal; a group that is already gone returns ESRCH.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
}

fn leak_name(program: &str) -> &'static str {
    match program {
        "sui" => "Sui",
        "aptos" => "Aptos",
        "stellar" => "Stellar",
        "cargo-build-sbf" | "cargo" => "Solana",
        "scarb" => "Starknet (Scarb)",
        _ => "build",
    }
}

pub fn tool_available(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

fn write(path: &Path, contents: &str) -> Result<(), CompileError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CompileError::Internal(e.to_string()))?;
    }
    std::fs::write(path, contents).map_err(|e| CompileError::Internal(e.to_string()))
}

fn read_b64(path: &Path) -> Result<String, CompileError> {
    std::fs::read(path)
        .map(|bytes| B64.encode(bytes))
        .map_err(|e| CompileError::Internal(format!("Build output missing ({}): {e}", path.display())))
}

fn find_file(dir: &Path, suffix: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(suffix))
    })
}

/// The newest Solana platform-tools install, from `SOLANA_PLATFORM_TOOLS`
/// or `~/.cache/solana/<version>/platform-tools`.
fn solana_platform_tools() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("SOLANA_PLATFORM_TOOLS").map(PathBuf::from) {
        return dir.join("rust/bin/rustc").is_file().then_some(dir);
    }
    let cache = PathBuf::from(std::env::var_os("HOME")?).join(".cache/solana");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(cache)
        .ok()?
        .flatten()
        .map(|e| e.path().join("platform-tools"))
        .filter(|p| p.join("rust/bin/rustc").is_file())
        .collect();
    // Version directories look like v1.54; compare numerically.
    versions.sort_by_key(|p| {
        p.parent()
            .and_then(|v| v.file_name())
            .and_then(|n| n.to_str())
            .map(|n| n.trim_start_matches('v').split('.').map(|x| x.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    versions.pop()
}

fn cache_base() -> PathBuf {
    std::env::var_os("COMPILER_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("txio-compiler-cache"))
}

/// Shared `target` directory per chain, so dependencies compile once rather
/// than on every request.
fn target_dir(chain: Chain) -> PathBuf {
    cache_base().join(format!("{chain:?}").to_lowercase())
}

/// Crate name for a build that uses a shared target directory: unique per
/// build, so its own artifacts can be found and removed afterwards (see
/// `remove_build_artifacts`) and concurrent builds never overwrite each other.
fn unique_crate_name(name: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{}_{}", package_name(name).chars().take(30).collect::<String>(), &id[..12])
}

/// Deletes one build's own outputs from a shared target directory, keeping
/// the compiled dependencies other builds reuse. Matches `<crate>`,
/// `<crate>-<hash>` and `lib<crate>` entries a few levels down.
pub fn remove_build_artifacts(target: &Path, crate_name: &str) {
    fn walk(dir: &Path, crate_name: &str, depth: u8) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let ours = name.strip_prefix("lib").unwrap_or(&name).starts_with(crate_name);
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if ours {
                let _ = if is_dir { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            } else if is_dir && depth > 0 {
                walk(&path, crate_name, depth - 1);
            }
        }
    }
    walk(target, crate_name, 4);
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Keeps the shared build cache under `max_bytes`: removes whole per-chain
/// target directories, largest first. A removed chain rebuilds its
/// dependencies on its next compile. Returns the bytes freed.
pub fn prune_cache(max_bytes: u64) -> u64 {
    let mut chains: Vec<(PathBuf, u64)> = Chain::ALL
        .iter()
        .map(|c| target_dir(*c))
        .map(|p| {
            let size = dir_size(&p);
            (p, size)
        })
        .collect();
    let mut total: u64 = chains.iter().map(|(_, s)| s).sum();
    chains.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
    let mut freed = 0;
    for (path, size) in chains {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            total -= size;
            freed += size;
        }
    }
    freed
}

const RUST_RELEASE_PROFILE: &str = r#"
[profile.release]
opt-level = "z"
overflow-checks = true
debug = 0
strip = "symbols"
debug-assertions = false
panic = "abort"
codegen-units = 1
lto = true
"#;

impl Runner {
    pub async fn compile(&self, dir: &Path, req: &CompileRequest) -> Result<CompileResponse, CompileError> {
        match req.chain {
            Chain::Stellar => self.stellar(dir, req).await,
            Chain::Solana => self.solana(dir, req).await,
            Chain::Starknet => self.starknet(dir, req).await,
            Chain::Sui => self.sui(dir, req).await,
            Chain::Aptos => self.aptos(dir, req).await,
        }
    }

    async fn stellar(&self, dir: &Path, req: &CompileRequest) -> Result<CompileResponse, CompileError> {
        let name = unique_crate_name(&req.name);
        let target = target_dir(Chain::Stellar);
        let result = self.stellar_build(dir, req, &name, &target).await;
        remove_build_artifacts(&target, &name);
        result
    }

    async fn stellar_build(&self, dir: &Path, req: &CompileRequest, name: &str, target: &Path) -> Result<CompileResponse, CompileError> {
        write(
            &dir.join("Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\ndoctest = false\n\n[dependencies]\nsoroban-sdk = \"{}\"\n{RUST_RELEASE_PROFILE}",
                std::env::var("SOROBAN_SDK_VERSION").unwrap_or_else(|_| "23".into())
            ),
        )?;
        write(&dir.join("src/lib.rs"), &req.source)?;
        let out = dir.join("out");
        let (ok, logs) = self
            .run_env(
                dir,
                "stellar",
                &["contract", "build", "--manifest-path", "Cargo.toml", "--out-dir", out.to_str().unwrap_or("out")],
                &[("CARGO_TARGET_DIR", target)],
            )
            .await?;
        if !ok {
            return Ok(CompileResponse { ok, logs, artifact: None });
        }
        let wasm = find_file(&out, ".wasm").ok_or_else(|| CompileError::Internal("No .wasm was produced.".into()))?;
        Ok(CompileResponse { ok, logs, artifact: Some(json!({ "wasm": read_b64(&wasm)? })) })
    }

    async fn solana(&self, dir: &Path, req: &CompileRequest) -> Result<CompileResponse, CompileError> {
        let name = unique_crate_name(&req.name);
        let target = target_dir(Chain::Solana);
        let result = self.solana_build(dir, req, &name, &target).await;
        remove_build_artifacts(&target, &name);
        result
    }

    async fn solana_build(&self, dir: &Path, req: &CompileRequest, name: &str, target: &Path) -> Result<CompileResponse, CompileError> {
        let anchor = req.source.contains("anchor_lang");
        let mut source = req.source.clone();
        if let Some(id) = &req.program_id {
            if !Regex::new(r"^[1-9A-HJ-NP-Za-km-z]{32,44}$").expect("valid regex").is_match(id) {
                return Err(CompileError::Rejected("program_id must be a base58 public key.".into()));
            }
            let declare = Regex::new(r#"declare_id!\s*\(\s*"[^"]*"\s*\)"#).expect("valid regex");
            if declare.is_match(&source) {
                source = declare.replace(&source, format!("declare_id!(\"{id}\")").as_str()).into_owned();
            } else if anchor {
                return Err(CompileError::Rejected("Anchor programs need a declare_id!(\"...\") line.".into()));
            }
        }
        let dependency = if anchor {
            format!("anchor-lang = \"{}\"", std::env::var("ANCHOR_LANG_VERSION").unwrap_or_else(|_| "1.2".into()))
        } else {
            format!("solana-program = \"{}\"", std::env::var("SOLANA_PROGRAM_VERSION").unwrap_or_else(|_| "2.3".into()))
        };
        write(
            &dir.join("Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\", \"lib\"]\n\n[features]\ndefault = []\ncpi = [\"no-entrypoint\"]\nno-entrypoint = []\nno-idl = []\nno-log-ix-name = []\nidl-build = []\ncustom-heap = []\ncustom-panic = []\nanchor-debug = []\n\n[dependencies]\n{dependency}\n\n[profile.release]\noverflow-checks = true\nlto = \"fat\"\ncodegen-units = 1\n"
            ),
        )?;
        write(&dir.join("src/lib.rs"), &source)?;
        let out = dir.join("out");
        // SBPF v3: mainnet executes it, and clusters with SIMD-0500 active
        // (local validators today) refuse to deploy anything older.
        let arch = std::env::var("SOLANA_SBPF_ARCH").unwrap_or_else(|_| "v3".into());
        let mut args = vec!["--manifest-path", "Cargo.toml", "--sbf-out-dir", out.to_str().unwrap_or("out"), "--arch", arch.as_str()];
        // Point cargo-build-sbf straight at the platform-tools rustc instead of
        // a rustup "+solana" override: newer rustup prints its toolchain list
        // in a format cargo-build-sbf 4.x fails to parse.
        let tools = solana_platform_tools();
        let rustc = tools.as_ref().map(|t| t.join("rust/bin/rustc"));
        let path = tools.as_ref().map(|t| {
            let mut paths = vec![t.join("rust/bin"), t.join("llvm/bin")];
            paths.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
            PathBuf::from(std::env::join_paths(paths).unwrap_or_default())
        });
        let mut env: Vec<(&str, &Path)> = vec![("CARGO_TARGET_DIR", target)];
        if let (Some(rustc), Some(path)) = (&rustc, &path) {
            args.push("--no-rustup-override");
            env.push(("RUSTC", rustc));
            env.push(("PATH", path));
        }
        let (ok, logs) = self.run_env(dir, "cargo-build-sbf", &args, &env).await?;
        if !ok {
            return Ok(CompileResponse { ok, logs, artifact: None });
        }
        let so = find_file(&out, ".so").ok_or_else(|| CompileError::Internal("No .so was produced.".into()))?;
        Ok(CompileResponse { ok, logs, artifact: Some(json!({ "program": read_b64(&so)?, "programId": req.program_id })) })
    }

    async fn starknet(&self, dir: &Path, req: &CompileRequest) -> Result<CompileResponse, CompileError> {
        let name = package_name(&req.name);
        let (_, version_out) = self.run(dir, "scarb", &["--version"]).await?;
        // `scarb --version` prints "cairo: 2.x.y ..."; the starknet package
        // version must match the bundled Cairo compiler exactly.
        let cairo = Regex::new(r"cairo:\s*([0-9]+\.[0-9]+\.[0-9]+)")
            .expect("valid regex")
            .captures(&version_out)
            .map(|c| c[1].to_string())
            .ok_or_else(|| CompileError::Internal(format!("Could not read the Cairo version from: {version_out}")))?;
        write(
            &dir.join("Scarb.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024_07\"\n\n[dependencies]\nstarknet = \"{cairo}\"\n\n[[target.starknet-contract]]\nsierra = true\ncasm = true\n"
            ),
        )?;
        write(&dir.join("src/lib.cairo"), &req.source)?;
        let (ok, logs) = self.run(dir, "scarb", &["--offline", "build"]).await?;
        // --offline fails only when the starknet package is not cached yet.
        let (ok, logs) = if !ok && logs.contains("offline") { self.run(dir, "scarb", &["build"]).await? } else { (ok, logs) };
        if !ok {
            return Ok(CompileResponse { ok, logs, artifact: None });
        }
        let out = dir.join("target/dev");
        let mut contracts = Vec::new();
        for entry in std::fs::read_dir(&out).map_err(|e| CompileError::Internal(e.to_string()))?.flatten() {
            let path = entry.path();
            let Some(file) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(stem) = file.strip_suffix(".contract_class.json") else { continue };
            let casm_path = out.join(format!("{stem}.compiled_contract_class.json"));
            let sierra: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|e| CompileError::Internal(e.to_string()))?)
                .map_err(|e| CompileError::Internal(e.to_string()))?;
            let casm: Value = serde_json::from_slice(&std::fs::read(&casm_path).map_err(|e| CompileError::Internal(e.to_string()))?)
                .map_err(|e| CompileError::Internal(e.to_string()))?;
            let contract = stem.strip_prefix(&format!("{name}_")).unwrap_or(stem).to_string();
            contracts.push(json!({ "name": contract, "sierra": sierra, "casm": casm }));
        }
        if contracts.is_empty() {
            return Ok(CompileResponse {
                ok: false,
                logs: format!("{logs}\nNo #[starknet::contract] module was found."),
                artifact: None,
            });
        }
        Ok(CompileResponse { ok, logs, artifact: Some(json!({ "contracts": contracts })) })
    }

    /// The address name the source's modules use, e.g. `my_pkg` in `module my_pkg::coin`.
    fn move_address_name(source: &str) -> String {
        Regex::new(r"\bmodule\s+([A-Za-z_][A-Za-z0-9_]*)::")
            .expect("valid regex")
            .captures(source)
            .map(|c| c[1].to_string())
            .unwrap_or_else(|| "contract".into())
    }

    async fn sui(&self, dir: &Path, req: &CompileRequest) -> Result<CompileResponse, CompileError> {
        let name = package_name(&req.name);
        let addr = Self::move_address_name(&req.source);
        write(
            &dir.join("Move.toml"),
            &format!("[package]\nname = \"{name}\"\nedition = \"2024\"\n\n[addresses]\n{addr} = \"0x0\"\n"),
        )?;
        write(&dir.join(format!("sources/{name}.move")), &req.source)?;
        let (ok, logs) = self
            .run(dir, "sui", &["move", "build", "--dump-bytecode-as-base64", "--silence-warnings", "--path", "."])
            .await?;
        if !ok {
            return Ok(CompileResponse { ok, logs, artifact: None });
        }
        // The JSON payload is the last line of stdout; build notes precede it.
        let payload = logs
            .lines()
            .rev()
            .find_map(|l| serde_json::from_str::<Value>(l.trim()).ok().filter(|v| v.get("modules").is_some()))
            .ok_or_else(|| CompileError::Internal("sui did not print the bytecode JSON.".into()))?;
        Ok(CompileResponse { ok, logs, artifact: Some(payload) })
    }

    async fn aptos(&self, dir: &Path, req: &CompileRequest) -> Result<CompileResponse, CompileError> {
        let sender = req
            .sender
            .as_deref()
            .filter(|s| Regex::new(r"^0x[0-9a-fA-F]{1,64}$").expect("valid regex").is_match(s))
            .ok_or_else(|| CompileError::Rejected("Connect an Aptos wallet: the package is compiled for your account address.".into()))?;
        let name = package_name(&req.name);
        let addr = Self::move_address_name(&req.source);
        write(
            &dir.join("Move.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n\n[addresses]\n{addr} = \"_\"\n\n[dependencies.AptosFramework]\ngit = \"https://github.com/aptos-labs/aptos-framework.git\"\nrev = \"mainnet\"\nsubdir = \"aptos-framework\"\n"
            ),
        )?;
        write(&dir.join(format!("sources/{name}.move")), &req.source)?;
        let payload_path = dir.join("payload.json");
        let named = format!("{addr}={sender}");
        let (ok, logs) = self
            .run(
                dir,
                "aptos",
                &[
                    "move", "build-publish-payload",
                    "--package-dir", ".",
                    "--named-addresses", &named,
                    "--json-output-file", payload_path.to_str().unwrap_or("payload.json"),
                    "--assume-yes",
                ],
            )
            .await?;
        if !ok {
            return Ok(CompileResponse { ok, logs, artifact: None });
        }
        let payload: Value = serde_json::from_slice(&std::fs::read(&payload_path).map_err(|e| CompileError::Internal(e.to_string()))?)
            .map_err(|e| CompileError::Internal(e.to_string()))?;
        Ok(CompileResponse { ok, logs, artifact: Some(payload) })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unique_crate_names_differ_and_stay_valid() {
        let a = unique_crate_name("My Counter!");
        let b = unique_crate_name("My Counter!");
        assert_ne!(a, b);
        assert!(a.starts_with("my_counter_"));
        assert!(a.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
    }

    #[test]
    fn removes_only_the_builds_own_artifacts() {
        let root = std::env::temp_dir().join(format!("txio-artifacts-{}", uuid::Uuid::new_v4()));
        let release = root.join("wasm32v1-none/release");
        let ours = "counter_abc123";
        for path in [
            release.join(format!("{ours}.wasm")),
            release.join(format!("deps/{ours}-1f2e.wasm")),
            release.join(format!("deps/lib{ours}-1f2e.rlib")),
            release.join(format!(".fingerprint/{ours}-1f2e/lib")),
            release.join("deps/soroban_sdk-9a8b.rlib"),
            release.join(".fingerprint/soroban_sdk-9a8b/lib"),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"x").unwrap();
        }
        remove_build_artifacts(&root, ours);
        assert!(!release.join(format!("{ours}.wasm")).exists());
        assert!(!release.join(format!("deps/{ours}-1f2e.wasm")).exists());
        assert!(!release.join(format!("deps/lib{ours}-1f2e.rlib")).exists());
        assert!(!release.join(format!(".fingerprint/{ours}-1f2e")).exists());
        assert!(release.join("deps/soroban_sdk-9a8b.rlib").exists());
        assert!(release.join(".fingerprint/soroban_sdk-9a8b/lib").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn caps_captured_output() {
        let input = [b'a'; 100];
        let kept = read_capped(&input[..], 10).await;
        let text = String::from_utf8(kept).unwrap();
        assert!(text.starts_with("aaaaaaaaaa\n"));
        assert!(text.contains("[90 more bytes of output dropped]"));
    }

    use super::*;

    #[test]
    fn names_become_safe_identifiers() {
        assert_eq!(package_name("AssetBridge"), "asset_bridge");
        assert_eq!(package_name("my contract!"), "my_contract");
        assert_eq!(package_name("9lives"), "contract");
        assert_eq!(package_name("\"; rm -rf /"), "rm_rf");
    }

    #[test]
    fn file_reading_macros_are_refused() {
        for src in [
            "const X: &str = include_str!(\"/etc/passwd\");",
            "const X: &[u8] = include_bytes !(\"x\");",
            "const K: &str = env!(\"HOME\");",
            "#[path = \"/tmp/x.rs\"] mod x;",
            "mod other;",
            "soroban_sdk::contractimport!(file = \"/x.wasm\");",
        ] {
            assert!(check_source(Chain::Stellar, src).is_err(), "{src}");
        }
        assert!(check_source(Chain::Stellar, "mod inner { pub fn f() {} }").is_ok());
    }

    #[test]
    fn move_address_name_is_read_from_the_module() {
        assert_eq!(Runner::move_address_name("module counter::counter {"), "counter");
        assert_eq!(Runner::move_address_name("fun x() {}"), "contract");
    }
}
