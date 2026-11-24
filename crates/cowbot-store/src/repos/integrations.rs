//! Persistence for IM integrations, their bindings, principals, and dedupe
//! rows (ADR-0056).
//!
//! Configuration is deliberately behind its own method. Every other read here
//! returns a record without the bot token, so a caller cannot leak a credential
//! by serializing what it listed.

use chrono::Utc;
use cowbot_core::agent::AgentId;
use cowbot_core::channel::ChannelId;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::{UserId, WorkspaceId};
use cowbot_core::integration::{
    IntegrationBinding, IntegrationId, IntegrationKind, IntegrationPrincipal, IntegrationRecord,
    NewIntegration,
};

use crate::error::{db, parse_datetime, parse_optional_datetime};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct IntegrationRow {
    id: String,
    workspace_id: String,
    kind: String,
    label: String,
    created_at: String,
    revoked_at: Option<String>,
}

impl IntegrationRow {
    fn into_domain(self) -> CowResult<IntegrationRecord> {
        let kind = self
            .kind
            .parse::<IntegrationKind>()
            .map_err(|_| CowError::Storage(format!("unknown integration kind '{}'", self.kind)))?;
        Ok(IntegrationRecord {
            id: IntegrationId(self.id),
            workspace_id: WorkspaceId(self.workspace_id),
            kind,
            label: self.label,
            created_at: parse_datetime(&self.created_at)?,
            revoked_at: parse_optional_datetime(self.revoked_at.as_deref())?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct BindingRow {
    integration_id: String,
    external_id: String,
    channel_id: String,
    agent_id: Option<String>,
}

impl BindingRow {
    fn into_domain(self) -> IntegrationBinding {
        IntegrationBinding {
            integration_id: IntegrationId(self.integration_id),
            external_id: self.external_id,
            channel_id: ChannelId(self.channel_id),
            agent_id: self.agent_id.map(AgentId),
        }
    }
}

#[derive(sqlx::FromRow)]
struct PrincipalRow {
    integration_id: String,
    external_id: String,
    user_id: String,
    display_name: String,
}

impl PrincipalRow {
    fn into_domain(self) -> IntegrationPrincipal {
        IntegrationPrincipal {
            integration_id: IntegrationId(self.integration_id),
            external_id: self.external_id,
            user_id: UserId(self.user_id),
            display_name: self.display_name,
        }
    }
}

const INTEGRATION_COLUMNS: &str = "id, workspace_id, kind, label, created_at, revoked_at";
const BINDING_COLUMNS: &str = "integration_id, external_id, channel_id, agent_id";
const PRINCIPAL_COLUMNS: &str = "integration_id, external_id, user_id, display_name";

impl Store {
    /// Record a new IM connection.
    pub async fn create_integration(&self, integration: &NewIntegration) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO integrations (id, workspace_id, kind, label, config, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&integration.id.0)
        .bind(&integration.workspace_id.0)
        .bind(integration.kind.as_str())
        .bind(&integration.label)
        .bind(&integration.config)
        .bind(integration.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// One connection, without its configuration.
    pub async fn get_integration(
        &self,
        id: &IntegrationId,
    ) -> CowResult<Option<IntegrationRecord>> {
        let row = sqlx::query_as::<_, IntegrationRow>(&format!(
            "SELECT {INTEGRATION_COLUMNS} FROM integrations WHERE id = ?"
        ))
        .bind(&id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(IntegrationRow::into_domain).transpose()
    }

    /// Every connection in a workspace, newest last.
    ///
    /// Revoked rows are included: an operator needs to see what was retired.
    /// [`Store::list_active_integrations`] is what a supervisor starts.
    pub async fn list_integrations(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Vec<IntegrationRecord>> {
        let rows = sqlx::query_as::<_, IntegrationRow>(&format!(
            "SELECT {INTEGRATION_COLUMNS} FROM integrations
             WHERE workspace_id = ? ORDER BY created_at"
        ))
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(IntegrationRow::into_domain).collect()
    }

    /// Every connection a supervisor should run.
    pub async fn list_active_integrations(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<Vec<IntegrationRecord>> {
        Ok(self
            .list_integrations(workspace_id)
            .await?
            .into_iter()
            .filter(IntegrationRecord::is_active)
            .collect())
    }

    /// The adapter configuration of one connection.
    ///
    /// Named for what it returns so a caller cannot pick it up by accident. The
    /// only intended caller is the adapter being started; nothing on a read path
    /// should reach it.
    pub async fn integration_config(&self, id: &IntegrationId) -> CowResult<Option<String>> {
        sqlx::query_scalar::<_, String>("SELECT config FROM integrations WHERE id = ?")
            .bind(&id.0)
            .fetch_optional(self.pool())
            .await
            .map_err(db)
    }

    /// Retire a connection. Its bindings and principals stay, so an operator can
    /// read what it was bound to after the fact.
    pub async fn revoke_integration(&self, id: &IntegrationId) -> CowResult<bool> {
        let result = sqlx::query(
            "UPDATE integrations SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(&id.0)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(result.rows_affected() > 0)
    }

    /// Bind an external conversation to a channel, replacing an existing binding
    /// for that conversation.
    pub async fn bind_integration_channel(&self, binding: &IntegrationBinding) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO integration_channels (integration_id, external_id, channel_id, agent_id)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(integration_id, external_id) DO UPDATE SET
                channel_id = excluded.channel_id,
                agent_id = excluded.agent_id",
        )
        .bind(&binding.integration_id.0)
        .bind(&binding.external_id)
        .bind(&binding.channel_id.0)
        .bind(binding.agent_id.as_ref().map(|id| id.0.clone()))
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// The binding for one external conversation.
    pub async fn integration_binding(
        &self,
        integration_id: &IntegrationId,
        external_id: &str,
    ) -> CowResult<Option<IntegrationBinding>> {
        let row = sqlx::query_as::<_, BindingRow>(&format!(
            "SELECT {BINDING_COLUMNS} FROM integration_channels
             WHERE integration_id = ? AND external_id = ?"
        ))
        .bind(&integration_id.0)
        .bind(external_id)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        Ok(row.map(BindingRow::into_domain))
    }

    /// Every binding of one connection.
    pub async fn list_integration_bindings(
        &self,
        integration_id: &IntegrationId,
    ) -> CowResult<Vec<IntegrationBinding>> {
        let rows = sqlx::query_as::<_, BindingRow>(&format!(
            "SELECT {BINDING_COLUMNS} FROM integration_channels
             WHERE integration_id = ? ORDER BY external_id"
        ))
        .bind(&integration_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(BindingRow::into_domain).collect())
    }

    /// The binding of one channel, for the outbound mirror.
    ///
    /// A channel may be bound by several conversations of one integration — a
    /// Telegram group and its topic, say — so the newest match is returned and
    /// the reply goes to one place.
    pub async fn integration_binding_for_channel(
        &self,
        integration_id: &IntegrationId,
        channel_id: &ChannelId,
    ) -> CowResult<Option<IntegrationBinding>> {
        let row = sqlx::query_as::<_, BindingRow>(&format!(
            "SELECT {BINDING_COLUMNS} FROM integration_channels
             WHERE integration_id = ? AND channel_id = ? ORDER BY external_id LIMIT 1"
        ))
        .bind(&integration_id.0)
        .bind(&channel_id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        Ok(row.map(BindingRow::into_domain))
    }

    /// Remove one binding.
    pub async fn unbind_integration_channel(
        &self,
        integration_id: &IntegrationId,
        external_id: &str,
    ) -> CowResult<bool> {
        let result = sqlx::query(
            "DELETE FROM integration_channels WHERE integration_id = ? AND external_id = ?",
        )
        .bind(&integration_id.0)
        .bind(external_id)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(result.rows_affected() > 0)
    }

    /// Record the CowBot user a platform account speaks as, replacing a previous
    /// mapping and refreshing the name the platform shows.
    pub async fn save_integration_principal(
        &self,
        principal: &IntegrationPrincipal,
    ) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO integration_principals
                (integration_id, external_id, user_id, display_name)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(integration_id, external_id) DO UPDATE SET
                user_id = excluded.user_id,
                display_name = excluded.display_name",
        )
        .bind(&principal.integration_id.0)
        .bind(&principal.external_id)
        .bind(&principal.user_id.0)
        .bind(&principal.display_name)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// The CowBot user for a platform account, if one was mapped.
    pub async fn integration_principal(
        &self,
        integration_id: &IntegrationId,
        external_id: &str,
    ) -> CowResult<Option<IntegrationPrincipal>> {
        let row = sqlx::query_as::<_, PrincipalRow>(&format!(
            "SELECT {PRINCIPAL_COLUMNS} FROM integration_principals
             WHERE integration_id = ? AND external_id = ?"
        ))
        .bind(&integration_id.0)
        .bind(external_id)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        Ok(row.map(PrincipalRow::into_domain))
    }

    /// Every principal of one connection.
    pub async fn list_integration_principals(
        &self,
        integration_id: &IntegrationId,
    ) -> CowResult<Vec<IntegrationPrincipal>> {
        let rows = sqlx::query_as::<_, PrincipalRow>(&format!(
            "SELECT {PRINCIPAL_COLUMNS} FROM integration_principals
             WHERE integration_id = ? ORDER BY display_name"
        ))
        .bind(&integration_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(PrincipalRow::into_domain).collect())
    }

    /// Claim a platform event id for handling.
    ///
    /// Returns `true` when this call claimed it and `false` when it was already
    /// claimed, so the caller can tell "first delivery" from "a retry" with one
    /// statement and no read-then-write race. The claim must be made *before*
    /// posting the message: a duplicated message in the timeline is worse than a
    /// delivery dropped because the process died between the claim and the post,
    /// since the platform will not retry a delivery it saw acknowledged twice
    /// anyway.
    pub async fn claim_integration_event(
        &self,
        integration_id: &IntegrationId,
        external_event_id: &str,
    ) -> CowResult<bool> {
        let result = sqlx::query(
            "INSERT INTO integration_events (integration_id, external_event_id, received_at)
             VALUES (?, ?, ?)
             ON CONFLICT(integration_id, external_event_id) DO NOTHING",
        )
        .bind(&integration_id.0)
        .bind(external_event_id)
        .bind(Utc::now().to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(result.rows_affected() > 0)
    }

    /// Forgo a claimed event id, so a delivery that failed before posting can be
    /// retried by the platform.
    ///
    /// Only the window between the claim and the post should call this; once the
    /// message is stored, the claim must stand or a retry would post twice.
    pub async fn release_integration_event(
        &self,
        integration_id: &IntegrationId,
        external_event_id: &str,
    ) -> CowResult<bool> {
        let result = sqlx::query(
            "DELETE FROM integration_events WHERE integration_id = ? AND external_event_id = ?",
        )
        .bind(&integration_id.0)
        .bind(external_event_id)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(result.rows_affected() > 0)
    }

    /// Whether a platform event id was already handled.
    pub async fn integration_event_seen(
        &self,
        integration_id: &IntegrationId,
        external_event_id: &str,
    ) -> CowResult<bool> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM integration_events WHERE integration_id = ? AND external_event_id = ?",
        )
        .bind(&integration_id.0)
        .bind(external_event_id)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        Ok(found.is_some())
    }

    /// Drop dedupe rows received before `before`, so the table does not grow for
    /// the life of the deployment.
    pub async fn purge_integration_events(
        &self,
        integration_id: &IntegrationId,
        before: chrono::DateTime<chrono::Utc>,
    ) -> CowResult<u64> {
        let result = sqlx::query(
            "DELETE FROM integration_events WHERE integration_id = ? AND received_at < ?",
        )
        .bind(&integration_id.0)
        .bind(before.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(result.rows_affected())
    }
}
