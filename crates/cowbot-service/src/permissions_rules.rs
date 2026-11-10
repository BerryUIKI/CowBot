use cowbot_core::approval::{BypassCritical, WorkspacePolicy};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::permission::{PermissionRule, RuleDecision};
use cowbot_store::Store;

/// Workspace-level allow / ask / deny rules for tool calls.
#[derive(Clone)]
pub struct PermissionService {
    store: Store,
    workspace_id: WorkspaceId,
}

impl PermissionService {
    pub fn new(store: Store, workspace_id: WorkspaceId) -> Self {
        Self {
            store,
            workspace_id,
        }
    }

    pub async fn list(&self) -> CowResult<Vec<PermissionRule>> {
        self.store.list_permission_rules(&self.workspace_id).await
    }

    pub async fn policy(&self) -> CowResult<WorkspacePolicy> {
        self.store.get_workspace_policy(&self.workspace_id).await
    }

    pub async fn set_bypass_critical(
        &self,
        bypass_critical: BypassCritical,
    ) -> CowResult<WorkspacePolicy> {
        let policy = WorkspacePolicy {
            workspace_id: self.workspace_id.clone(),
            bypass_critical,
        };
        self.store.save_workspace_policy(&policy).await?;
        Ok(policy)
    }

    pub async fn create(
        &self,
        tool: &str,
        command: Option<&str>,
        decision: RuleDecision,
    ) -> CowResult<PermissionRule> {
        let tool = tool.trim();
        if tool.is_empty() {
            return Err(CowError::Config(
                "a permission rule needs a tool name or '*'".to_string(),
            ));
        }
        if tool != "*"
            && !tool
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(CowError::Config(format!(
                "'{tool}' is not a valid tool name"
            )));
        }
        let command = command.map(str::trim).filter(|command| !command.is_empty());
        if command.is_some() && tool != "execute_code" && tool != "*" {
            return Err(CowError::Config(
                "command patterns only apply to execute_code rules".to_string(),
            ));
        }
        if command.is_some_and(|command| command.chars().count() > 200) {
            return Err(CowError::Config(
                "command patterns are limited to 200 characters".to_string(),
            ));
        }

        let rule = PermissionRule::new(
            self.workspace_id.clone(),
            tool,
            command.map(str::to_string),
            decision,
        );
        self.store.upsert_permission_rule(&rule).await?;
        Ok(rule)
    }

    pub async fn set_enabled(&self, id: &str, enabled: bool) -> CowResult<PermissionRule> {
        let rule = self.load(id).await?;
        self.store
            .set_permission_rule_enabled(&rule.id, enabled)
            .await?;
        self.load(id).await
    }

    pub async fn delete(&self, id: &str) -> CowResult<()> {
        let rule = self.load(id).await?;
        self.store.delete_permission_rule(&rule.id).await
    }

    async fn load(&self, id: &str) -> CowResult<PermissionRule> {
        let rule = self
            .store
            .get_permission_rule(id)
            .await?
            .ok_or_else(|| CowError::Task(format!("permission rule '{id}' was not found")))?;
        if rule.workspace_id != self.workspace_id {
            return Err(CowError::Task(format!(
                "permission rule '{id}' belongs to another workspace"
            )));
        }
        Ok(rule)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap_local;

    async fn service() -> PermissionService {
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let context = bootstrap_local(&store).await.unwrap();
        PermissionService::new(store, context.workspace.id)
    }

    #[tokio::test]
    async fn rules_are_created_listed_and_deleted() {
        let service = service().await;
        let rule = service
            .create("execute_code", Some("git status*"), RuleDecision::Allow)
            .await
            .unwrap();
        assert!(rule.enabled);
        assert_eq!(rule.source, cowbot_core::permission::RuleSource::Local);

        let listed = service.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].command.as_deref(), Some("git status*"));

        let disabled = service.set_enabled(&rule.id, false).await.unwrap();
        assert!(!disabled.enabled);

        service.delete(&rule.id).await.unwrap();
        assert!(service.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_rules_are_rejected() {
        let service = service().await;
        assert!(service.create("  ", None, RuleDecision::Ask).await.is_err());
        assert!(service
            .create("write_file", Some("git *"), RuleDecision::Ask)
            .await
            .is_err());
        assert!(service
            .create("bad tool", None, RuleDecision::Ask)
            .await
            .is_err());
    }
}
