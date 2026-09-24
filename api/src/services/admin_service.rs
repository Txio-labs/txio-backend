use crate::dtos::admin_dtos::{
    AdminCollectionEntry, AdminEndpointStatsEntry, AdminLogEntry, AdminOverviewResponse,
    AdminRequestEntry, AdminStatsResponse, AdminUserEntry,
};
use crate::model::user::User;
use crate::repositories::admin_repository::AdminRepository;
use crate::repositories::rpc_repository::RpcRepository;
use crate::repositories::session_repository::SessionRepository;
use crate::repositories::user_repository::UserRepository;
use crate::utils::auth_jwt::Claims;
use crate::utils::error::AppError;
use mongodb::bson::oid::ObjectId;

#[derive(Clone)]
pub struct AdminService {
    user_repo: UserRepository,
    rpc_repo: RpcRepository,
    session_repo: SessionRepository,
    admin_repo: AdminRepository,
}

impl AdminService {
    pub fn new(
        user_repo: UserRepository,
        rpc_repo: RpcRepository,
        session_repo: SessionRepository,
        admin_repo: AdminRepository,
    ) -> Self {
        Self {
            user_repo,
            rpc_repo,
            session_repo,
            admin_repo,
        }
    }

    /// Admin access is granted solely by the durable `User.is_admin` flag.
    /// Email allowlist matching against JWT claims is intentionally not used.
    pub(crate) fn ensure_admin_flag(user: &User) -> Result<(), AppError> {
        if user.is_admin {
            Ok(())
        } else {
            Err(AppError::Forbidden("Admin access required".into()))
        }
    }

    async fn require_admin(&self, claims: &Claims) -> Result<(), AppError> {
        let oid = ObjectId::parse_str(&claims.sub)
            .map_err(|_| AppError::Unauthorized("Invalid token subject".into()))?;
        let user = self.user_repo.find_by_id(&oid).await?;
        Self::ensure_admin_flag(&user)
    }

    pub async fn list_user_emails(&self, claims: &Claims) -> Result<Vec<String>, AppError> {
        self.require_admin(claims).await?;
        self.user_repo.list_all_emails().await
    }

    /// Guards an admin-initiated deletion: admins can't remove themselves
    /// (no lock-out by misclick) or other admins (demotion is an ops task).
    pub(crate) fn ensure_deletable(actor_id: &ObjectId, target: &User) -> Result<(), AppError> {
        if target.id.as_ref() == Some(actor_id) {
            return Err(AppError::BadRequest(
                "You can't delete your own account from the admin dashboard".into(),
            ));
        }
        if target.is_admin {
            return Err(AppError::Forbidden(
                "Admin accounts can't be deleted from the dashboard".into(),
            ));
        }
        Ok(())
    }

    pub async fn delete_user(&self, claims: &Claims, email: &str) -> Result<String, AppError> {
        self.require_admin(claims).await?;
        let actor_id = ObjectId::parse_str(&claims.sub)
            .map_err(|_| AppError::Unauthorized("Invalid token subject".into()))?;

        let user = self
            .user_repo
            .find_by_email(&email.trim().to_ascii_lowercase())
            .await?;
        Self::ensure_deletable(&actor_id, &user)?;
        let oid = user
            .id
            .ok_or_else(|| AppError::InternalError("User ID missing".into()))?;

        // Sessions first so the account loses access immediately; then owned
        // data; the user document last, so any failure leaves an account an
        // admin can find and retry rather than orphaned data with no owner.
        self.session_repo.delete_all_by_user_id(&oid).await?;
        let removed = self.admin_repo.purge_user_data(oid).await?;
        let deleted = self.user_repo.delete_by_id(&oid.to_hex()).await?;

        tracing::info!(
            actor = %claims.sub,
            deleted_user = %oid,
            ?removed,
            "Admin deleted user"
        );
        Ok(deleted.email)
    }

    pub async fn stats(&self, claims: &Claims) -> Result<AdminStatsResponse, AppError> {
        self.require_admin(claims).await?;

        let user_count = self.user_repo.count_documents().await?;
        let rpc_log_count = self.rpc_repo.count_all().await?;

        Ok(AdminStatsResponse {
            user_count,
            rpc_log_count,
        })
    }

    pub async fn list_logs(
        &self,
        claims: &Claims,
        limit: i64,
    ) -> Result<Vec<AdminLogEntry>, AppError> {
        self.require_admin(claims).await?;

        let logs = self.rpc_repo.find_recent(limit).await?;
        let emails = self
            .admin_repo
            .emails_for(logs.iter().map(|log| log.user_id))
            .await?;
        Ok(logs
            .into_iter()
            .map(|log| AdminLogEntry {
                user_email: emails.get(&log.user_id).cloned(),
                method: log.method,
                success: log.success,
                error: log.error,
                timestamp: log.timestamp.to_rfc3339(),
                endpoint: log.endpoint,
                duration_ms: log.duration_ms,
            })
            .collect())
    }

    /// Per-endpoint rollup (usage volume, failures, latency) over the most
    /// recent logged calls — backs the admin "Endpoints" view.
    pub async fn endpoint_stats(
        &self,
        claims: &Claims,
        sample_size: i64,
    ) -> Result<Vec<AdminEndpointStatsEntry>, AppError> {
        self.require_admin(claims).await?;

        let stats = self.rpc_repo.endpoint_stats(sample_size).await?;
        Ok(stats
            .into_iter()
            .map(|s| AdminEndpointStatsEntry {
                endpoint: s.endpoint,
                total_calls: s.total_calls,
                failed_calls: s.failed_calls,
                avg_duration_ms: s.avg_duration_ms,
                recent_errors: s.recent_errors,
            })
            .collect())
    }

    pub async fn overview(&self, claims: &Claims) -> Result<AdminOverviewResponse, AppError> {
        self.require_admin(claims).await?;
        self.admin_repo.overview().await
    }

    pub async fn list_accounts(
        &self,
        claims: &Claims,
        limit: i64,
    ) -> Result<Vec<AdminUserEntry>, AppError> {
        self.require_admin(claims).await?;
        self.admin_repo.list_users(limit).await
    }

    pub async fn recent_requests(
        &self,
        claims: &Claims,
        limit: i64,
    ) -> Result<Vec<AdminRequestEntry>, AppError> {
        self.require_admin(claims).await?;
        self.admin_repo.recent_requests(limit).await
    }

    pub async fn list_collections(
        &self,
        claims: &Claims,
        limit: i64,
    ) -> Result<Vec<AdminCollectionEntry>, AppError> {
        self.require_admin(claims).await?;
        self.admin_repo.list_collections(limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::user::User;

    fn sample_user(is_admin: bool) -> User {
        let mut user = User::new("admin@example.com".into(), "hash".into());
        user.is_admin = is_admin;
        user
    }

    #[test]
    fn ensure_admin_flag_accepts_admin_user() {
        assert!(AdminService::ensure_admin_flag(&sample_user(true)).is_ok());
    }

    #[test]
    fn ensure_admin_flag_rejects_non_admin_user() {
        assert!(matches!(
            AdminService::ensure_admin_flag(&sample_user(false)),
            Err(AppError::Forbidden(_))
        ));
    }

    #[test]
    fn ensure_deletable_rejects_self() {
        let mut me = sample_user(true);
        let id = ObjectId::new();
        me.id = Some(id);
        me.is_admin = false;
        assert!(matches!(
            AdminService::ensure_deletable(&id, &me),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn ensure_deletable_rejects_other_admins() {
        let mut other = sample_user(true);
        other.id = Some(ObjectId::new());
        assert!(matches!(
            AdminService::ensure_deletable(&ObjectId::new(), &other),
            Err(AppError::Forbidden(_))
        ));
    }

    #[test]
    fn ensure_deletable_allows_regular_users() {
        let mut user = sample_user(false);
        user.id = Some(ObjectId::new());
        assert!(AdminService::ensure_deletable(&ObjectId::new(), &user).is_ok());
    }

    #[test]
    fn ensure_admin_flag_ignores_email_string() {
        // Even with an "admin-looking" email, privilege requires the flag.
        let user = User::new("admin@txio.io".into(), "hash".into());
        assert!(AdminService::ensure_admin_flag(&user).is_err());
    }
}
