//! txio-compiler: builds Contract Builder sources for Sui, Aptos, Stellar,
//! Solana and Starknet.
//!
//! Security model: this process compiles code that users submit, so it must
//! hold no secrets and should run in its own container (see
//! `compiler/Dockerfile`). Sources that use file- or env-reading macros are
//! refused before any build runs, every build runs in a fresh directory with
//! a minimal environment, and builds are capped in size, time and
//! concurrency.
//!
//! Environment:
//! - `COMPILER_PORT` (default 8090)
//! - `COMPILER_ALLOWED_ORIGINS`: comma-separated browser origins (default http://localhost:3000)
//! - `COMPILER_MAX_CONCURRENT` (default 2), `COMPILER_TIMEOUT_SECS` (default 600)
//! - `COMPILER_CACHE_DIR`: shared build caches (default: system temp dir)

mod toolchains;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tokio::sync::Semaphore;
use tower_http::cors::CorsLayer;

use toolchains::{check_source, tool_available, Chain, CompileError, CompileRequest, Runner, MAX_SOURCE_BYTES};

struct AppState {
    runner: Runner,
    permits: Semaphore,
}

impl IntoResponse for CompileError {
    fn into_response(self) -> Response {
        let status = match self {
            CompileError::Rejected(_) => StatusCode::BAD_REQUEST,
            CompileError::MissingToolchain(_) => StatusCode::SERVICE_UNAVAILABLE,
            CompileError::Timeout(_) => StatusCode::REQUEST_TIMEOUT,
            CompileError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

/// Which chains this host can compile right now.
async fn toolchains() -> impl IntoResponse {
    let chains: serde_json::Map<String, serde_json::Value> = Chain::ALL
        .iter()
        .map(|c| {
            let name = serde_json::to_value(c).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            (name, json!({ "available": tool_available(c.tool()), "tool": c.tool() }))
        })
        .collect();
    Json(json!({ "chains": chains }))
}

async fn compile(State(state): State<Arc<AppState>>, Json(req): Json<CompileRequest>) -> Result<impl IntoResponse, CompileError> {
    check_source(req.chain, &req.source)?;
    if !tool_available(req.chain.tool()) {
        return Err(CompileError::MissingToolchain(match req.chain {
            Chain::Sui => "Sui",
            Chain::Aptos => "Aptos",
            Chain::Stellar => "Stellar",
            Chain::Solana => "Solana",
            Chain::Starknet => "Starknet (Scarb)",
        }));
    }
    let _permit = state
        .permits
        .acquire()
        .await
        .map_err(|_| CompileError::Internal("The compiler is shutting down.".into()))?;

    let dir = std::env::temp_dir().join(format!("txio-build-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| CompileError::Internal(e.to_string()))?;
    let started = std::time::Instant::now();
    let result = state.runner.compile(&dir, &req).await;
    let _ = std::fs::remove_dir_all(&dir);
    tracing::info!(chain = ?req.chain, ok = result.as_ref().map(|r| r.ok).unwrap_or(false), ms = started.elapsed().as_millis() as u64, "compile");
    Ok(Json(result?))
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub fn router() -> Router {
    let origins: Vec<HeaderValue> = std::env::var("COMPILER_ALLOWED_ORIGINS")
        .unwrap_or_else(|_| "http://localhost:3000".into())
        .split(',')
        .filter_map(|o| HeaderValue::from_str(o.trim()).ok())
        .collect();
    let state = Arc::new(AppState {
        runner: Runner { timeout: Duration::from_secs(env_or("COMPILER_TIMEOUT_SECS", 600)) },
        permits: Semaphore::new(env_or("COMPILER_MAX_CONCURRENT", 2)),
    });
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/toolchains", get(toolchains))
        .route("/compile", post(compile))
        .layer(DefaultBodyLimit::max(MAX_SOURCE_BYTES + 16 * 1024))
        .layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([axum::http::header::CONTENT_TYPE]),
        )
        .with_state(state)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let port: u16 = env_or("COMPILER_PORT", 8090);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    for chain in Chain::ALL {
        tracing::info!(?chain, tool = chain.tool(), available = tool_available(chain.tool()), "toolchain");
    }
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind compiler port");
    tracing::info!(%addr, "txio-compiler listening");
    axum::serve(listener, router()).await.expect("server");
}
