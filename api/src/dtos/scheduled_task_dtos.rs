use crate::model::scheduled_task::TriggerKind;
use serde::Deserialize;
use serde_json::Value;
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateScheduledTaskRequest {
    #[validate(length(min = 1, message = "Name cannot be empty"))]
    pub name: String,

    pub session_key_id: String,

    pub trigger: TriggerKind,

    pub request_template: Value,
}
