use crate::api::handlers::scheduled_task_handler;
use crate::services::scheduled_task_service::ScheduledTaskService;
use axum::{routing::{get, post}, Router};

pub fn router(service: ScheduledTaskService) -> Router {
    Router::new()
        .route("/", post(scheduled_task_handler::create_task))
        .route("/", get(scheduled_task_handler::list_tasks))
        .route("/:id/pause", post(scheduled_task_handler::pause_task))
        .route("/:id/resume", post(scheduled_task_handler::resume_task))
        .route("/:id/cancel", post(scheduled_task_handler::cancel_task))
        .with_state(service)
}
