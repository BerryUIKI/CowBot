use chrono::{DateTime, Utc};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::permission::{PermissionRule, RuleDecision, RuleSource};

use crate::error::{db, parse_datetime};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct PermissionRuleRow {
    id: String,
    workspace_id: String,
    tool: String,
    command: Option<String>,
    decision: String,
    enabled: i64,
    source: String,
    created_at: String,
    updated_at: String,
}

impl PermissionRuleRow {
    fn into_domain(self) -> CowResult<PermissionRule> {
        Ok(PermissionRule {
            id: self.id,
            workspace_id: WorkspaceId(self.workspace_id),
            tool: self.tool,
            command: self.command,
            decision: rule_decision_from_str(&self.decision)?,
            enabled: self.enabled != 0,
            source: rule_source_from_str(&self.source)?,
            created_at: parse_datetime(&self.created_at)?,
            updated_at: parse_datetime(&self.updated_at)?,
        })
    }
}

fn rule_decision_to_str(decision: RuleDecision) -> &'static str {
    decision.as_str()
}

fn rule_decision_from_str(value: &str) -> CowResult<RuleDecision> {
    match value {
        "allow" => Ok(RuleDecision::Allow),
        "ask" => Ok(RuleDecision::Ask),
        "deny" => Ok(RuleDecision::Deny),
        other => Err(CowError::Storage(format!(
            "unknown permission rule decision '{other}'"
        ))),
    }
}

fn rule_source_from_str(value: &str) -> CowResult<RuleSource> {
    match value {
        "local" => Ok(RuleSource::Local),
        "imported" => Ok(RuleSource::Imported),
        other => Err(CowError::Storage(format!(
            "unknown permission rule source '{other}'"
        ))),
    }
}

impl Store {
    pub async fn list_permission_rules(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Vec<PermissionRule>> {
        let rows = sqlx::query_as::<_, PermissionRuleRow>(
            "SELECT id, workspace_id, tool, command, decision, enabled, source,
                    created_at, updated_at
             FROM permission_rules WHERE workspace_id = ? ORDER BY created_at, id",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(PermissionRuleRow::into_domain)
            .collect()
    }

    pub async fn get_permission_rule(&self, id: &str) -> CowResult<Option<PermissionRule>> {
        let row = sqlx::query_as::<_, PermissionRuleRow>(
            "SELECT id, workspace_id, tool, command, decision, enabled, source,
                    created_at, updated_at
             FROM permission_rules WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(PermissionRuleRow::into_domain).transpose()
    }

    pub async fn upsert_permission_rule(&self, rule: &PermissionRule) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO permission_rules
                (id, workspace_id, tool, command, decision, enabled, source, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                tool = excluded.tool,
                command = excluded.command,
                decision = excluded.decision,
                enabled = excluded.enabled,
                source = excluded.source,
                updated_at = excluded.updated_at",
        )
        .bind(&rule.id)
        .bind(&rule.workspace_id.0)
        .bind(&rule.tool)
        .bind(&rule.command)
        .bind(rule_decision_to_str(rule.decision))
        .bind(i64::from(rule.enabled))
        .bind(rule.source.as_str())
        .bind(rule.created_at.to_rfc3339())
        .bind(rule.updated_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn set_permission_rule_enabled(&self, id: &str, enabled: bool) -> CowResult<()> {
        let updated =
            sqlx::query("UPDATE permission_rules SET enabled = ?, updated_at = ? WHERE id = ?")
                .bind(i64::from(enabled))
                .bind(Utc::now().to_rfc3339())
                .bind(id)
                .execute(self.pool())
                .await
                .map_err(db)?
                .rows_affected();
        if updated == 0 {
            return Err(CowError::Storage(format!(
                "permission rule '{id}' was not found"
            )));
        }
        Ok(())
    }

    pub async fn delete_permission_rule(&self, id: &str) -> CowResult<()> {
        let deleted = sqlx::query("DELETE FROM permission_rules WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db)?
            .rows_affected();
        if deleted == 0 {
            return Err(CowError::Storage(format!(
                "permission rule '{id}' was not found"
            )));
        }
        Ok(())
    }

    /// Touch the updated timestamp after an in-memory edit.
    pub async fn touch_permission_rule(
        &self,
        id: &str,
        updated_at: DateTime<Utc>,
    ) -> CowResult<()> {
        sqlx::query("UPDATE permission_rules SET updated_at = ? WHERE id = ?")
            .bind(updated_at.to_rfc3339())
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }
}
