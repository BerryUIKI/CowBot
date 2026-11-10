use chrono::Utc;
use cowbot_core::approval::{BypassCritical, WorkspacePolicy};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;

use crate::error::db;
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct WorkspacePolicyRow {
    bypass_critical: String,
}

fn bypass_critical_from_str(value: &str) -> CowResult<BypassCritical> {
    match value {
        "allow" => Ok(BypassCritical::Allow),
        "ask" => Ok(BypassCritical::Ask),
        "block" => Ok(BypassCritical::Block),
        other => Err(CowError::Storage(format!(
            "unknown bypass critical policy '{other}'"
        ))),
    }
}

impl Store {
    /// Read the workspace policy, falling back to the default (Allow) when the
    /// workspace has never changed it.
    pub async fn get_workspace_policy(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<WorkspacePolicy> {
        let row = sqlx::query_as::<_, WorkspacePolicyRow>(
            "SELECT bypass_critical FROM workspace_policies WHERE workspace_id = ?",
        )
        .bind(&workspace_id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        let bypass_critical = match row {
            Some(row) => bypass_critical_from_str(&row.bypass_critical)?,
            None => BypassCritical::default(),
        };
        Ok(WorkspacePolicy {
            workspace_id: workspace_id.clone(),
            bypass_critical,
        })
    }

    pub async fn save_workspace_policy(&self, policy: &WorkspacePolicy) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO workspace_policies (workspace_id, bypass_critical, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(workspace_id) DO UPDATE SET
                bypass_critical = excluded.bypass_critical,
                updated_at = excluded.updated_at",
        )
        .bind(&policy.workspace_id.0)
        .bind(policy.bypass_critical.as_str())
        .bind(Utc::now().to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }
}
