use crate::api::handlers::public_api_handler;
use crate::services::public_api_service::PublicApiService;
use axum::{routing::{get, post}, Router};

// GET /routes (LI.FI route-comparison proxy) is intentionally not included
// yet — it would require porting lib/lifi.ts's client to Rust, which the
// Phase 6 plan flagged as a "solves for later" benefit, not a hard
// requirement for this router. The frontend's own lib/lifi.ts remains the
// only route-comparison client for now; a public caller wanting routes
// today would need to run that logic themselves against the LI.FI API
// directly, same as any other external client.
pub fn router(service: PublicApiService) -> Router {
    Router::new()
        .route("/history", get(public_api_handler::get_history))
        .route("/transactions/simulate", post(public_api_handler::simulate_transaction))
        .route("/transactions/execute", post(public_api_handler::execute_transaction))
        .with_state(service)
}
