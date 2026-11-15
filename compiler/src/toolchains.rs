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
        // Toolchains need their install and cache locations, nothing else.
        for key in ["PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME", "XDG_CACHE_HOME", "SCARB_CACHE", "LANG", "MOVE_HOME", "SUI_CONFIG_DIR"] {
            if let Ok(v) = std::env::var(key) {
                cmd.env(key, v);
            }
        }
        cmd.env("CARGO_TERM_COLOR", "never").env("NO_COLOR", "1");
        for (key, value) in extra_env {
            cmd.env(key, value);
        }

        let child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CompileError::MissingToolchain(leak_name(program))
            } else {
                CompileError::Internal(format!("Could not start {program}: {e}"))
            }
        })?;
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| CompileError::Timeout(self.timeout.as_secs()))?
            .map_err(|e| CompileError::Internal(e.to_string()))?;
        let mut logs = String::from_utf8_lossy(&output.stdout).into_owned();
        logs.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok((output.status.success(), logs))
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

/// Shared `target` directory per chain, so dependencies compile once rather
/// than on every request.
fn target_dir(chain: Chain) -> PathBuf {
    let base = std::env::var_os("COMPILER_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("txio-compiler-cache"));
    base.join(format!("{chain:?}").to_lowercase())
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
        let name = package_name(&req.name);
        write(
            &dir.join("Cargo.toml"),
            &format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\ndoctest = false\n\n[dependencies]\nsoroban-sdk = \"{}\"\n{RUST_RELEASE_PROFILE}",
                std::env::var("SOROBAN_SDK_VERSION").unwrap_or_else(|_| "23".into())
            ),
        )?;
        write(&dir.join("src/lib.rs"), &req.source)?;
        let target = target_dir(Chain::Stellar);
        let out = dir.join("out");
        let (ok, logs) = self
            .run_env(
                dir,
                "stellar",
                &["contract", "build", "--manifest-path", "Cargo.toml", "--out-dir", out.to_str().unwrap_or("out")],
                &[("CARGO_TARGET_DIR", &target)],
            )
            .await?;
        if !ok {
            return Ok(CompileResponse { ok, logs, artifact: None });
        }
        let wasm = find_file(&out, ".wasm").ok_or_else(|| CompileError::Internal("No .wasm was produced.".into()))?;
        Ok(CompileResponse { ok, logs, artifact: Some(json!({ "wasm": read_b64(&wasm)? })) })
    }

    async fn solana(&self, dir: &Path, req: &CompileRequest) -> Result<CompileResponse, CompileError> {
        let name = package_name(&req.name);
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
        let target = target_dir(Chain::Solana);
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
        let mut env: Vec<(&str, &Path)> = vec![("CARGO_TARGET_DIR", &target)];
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
