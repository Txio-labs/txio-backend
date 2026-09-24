use crate::dtos::scheduled_task_dtos::CreateScheduledTaskRequest;
use crate::model::scheduled_task::{ScheduledTask, ScheduledTaskStatus};
use crate::repositories::scheduled_task_repository::ScheduledTaskRepository;
use crate::repositories::session_key_repository::SessionKeyRepository;
use crate::utils::error::AppError;
use mongodb::bson::oid::ObjectId;

#[derive(Clone)]
pub struct ScheduledTaskService {
    task_repo: ScheduledTaskRepository,
    session_key_repo: SessionKeyRepository,
}

impl ScheduledTaskService {
    pub fn new(task_repo: ScheduledTaskRepository, session_key_repo: SessionKeyRepository) -> Self {
        Self {
            task_repo,
            session_key_repo,
        }
    }

    pub async fn create(
        &self,
        user_id: ObjectId,
        req: CreateScheduledTaskRequest,
    ) -> Result<ScheduledTask, AppError> {
        let session_key_id = ObjectId::parse_str(&req.session_key_id)
            .map_err(|_| AppError::BadRequest("Invalid session key id".into()))?;

        // A schedule must be backed by a session key the caller actually
        // owns and that is currently active — creating a task against an
        // expired/revoked/someone-else's key would silently never run (or
        // worse, run once it's re-activated unexpectedly), so this is
        // checked eagerly rather than only at execution time.
        let key = self.session_key_repo.find_by_id(session_key_id, user_id).await?;
        if !key.is_active() {
            return Err(AppError::BadRequest(
                "The selected session key is expired or revoked".into(),
            ));
        }

        let task = ScheduledTask::new(
            user_id,
            session_key_id,
            req.name,
            req.trigger,
            req.request_template,
        );
        self.task_repo.insert(&task).await
    }

    pub async fn list(&self, user_id: ObjectId) -> Result<Vec<ScheduledTask>, AppError> {
        self.task_repo.find_by_user(user_id).await
    }

    pub async fn set_status(
        &self,
        id: ObjectId,
        user_id: ObjectId,
        status: ScheduledTaskStatus,
    ) -> Result<(), AppError> {
        self.task_repo.set_status(id, user_id, status).await
    }

    /// Active, time-based tasks (Recurring/TimeOnce) whose `next_run_at` has
    /// passed — the scheduler worker's poll target for time-driven triggers.
    pub async fn due_tasks_for_worker(&self) -> Result<Vec<ScheduledTask>, AppError> {
        self.task_repo.find_due().await
    }

    /// Active PriceThreshold tasks — condition-based, so the worker
    /// evaluates all of them every tick rather than filtering by a stored
    /// next-run time.
    pub async fn price_triggered_tasks_for_worker(&self) -> Result<Vec<ScheduledTask>, AppError> {
        self.task_repo.find_active_price_triggered().await
    }

    pub async fn record_run(
        &self,
        id: ObjectId,
        next_run_at: Option<chrono::DateTime<chrono::Utc>>,
        error: Option<String>,
    ) -> Result<(), AppError> {
        self.task_repo.mark_ran(id, next_run_at, error).await
    }
}
