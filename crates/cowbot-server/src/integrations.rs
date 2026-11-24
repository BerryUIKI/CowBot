//! IM integration routes (ADR-0056).
//!
//! These exist so an operator can stand up a bot without hand-editing the
//! database. Creating one is an administer-level action — a Member who could
//! mint a bot connection could point it at a webhook they control and read what
//! the workspace says — while reading the list is a Member action, because an
//! operator deciding whether to approve something needs to know which bots feed
//! which channels.
//!
//! **The configuration never leaves the server.** A create takes it, a read
//! never returns it, and the two are different types so the compiler enforces it.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use cowbot_core::agent::AgentId;
use cowbot_core::channel::ChannelId;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::integration::{
    IntegrationBinding, IntegrationId, IntegrationKind, IntegrationRecord, NewIntegration,
};
use serde::{Deserialize, Serialize};

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::router::AppState;
use crate::workspaces::{audit, require_member};

/// An integration as the API returns it.
///
/// Every field of [`IntegrationRecord`] and nothing else — no configuration
/// field exists to leak.
#[derive(Debug, Serialize)]
pub struct IntegrationBody {
    pub id: String,
    pub workspace_id: String,
    pub kind: IntegrationKind,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Whether a build like this one has an adapter for the kind, so the desktop
    /// can say "this server cannot run it" instead of offering a start action
    /// that would do nothing.
    pub supported: bool,
    /// How many external conversations feed a channel.
    pub binding_count: usize,
}

#[derive(Debug, Serialize)]
pub struct BindingBody {
    pub integration_id: String,
    pub external_id: String,
    pub channel_id: String,
    pub agent_id: Option<String>,
    /// What the adapter says about reaching this conversation, when a bind was
    /// able to ask.
    ///
    /// `"reachable"`, `"unreachable"`, or `"unknown"`, with `health_detail`
    /// carrying the reason for the last two. A list omits it: checking every
    /// binding on every list would turn a cheap read into N round trips to the
    /// platform, and the startup check already posts a notice into the CowBot
    /// channel an operator is looking at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_detail: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateIntegrationRequest {
    pub kind: String,
    pub label: String,
    /// Adapter configuration as a JSON object: endpoint, bot token, signing
    /// secret. Shapes are adapter-specific and validated by the adapter, not
    /// here — this route only checks that it parses and says so plainly when it
    /// does not.
    pub config: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct BindRequest {
    /// The platform's own conversation id.
    pub external_id: String,
    pub channel_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// The kinds this build can actually start.
///
/// A server without the Mattermost adapter still lists a Mattermost row if one
/// was created, so an operator sees it and knows why nothing happens. Reporting
/// it on the row is cheaper than a separate capability endpoint and harder to
/// forget to check.
fn supported_kinds() -> Vec<IntegrationKind> {
    crate::adapters::registry().kinds()
}

async fn integration_body(
    state: &AppState,
    record: IntegrationRecord,
) -> Result<IntegrationBody, ApiError> {
    let binding_count = state
        .store
        .list_integration_bindings(&record.id)
        .await?
        .len();
    Ok(IntegrationBody {
        supported: supported_kinds().contains(&record.kind),
        id: record.id.0,
        workspace_id: record.workspace_id.0,
        kind: record.kind,
        label: record.label,
        created_at: record.created_at,
        revoked_at: record.revoked_at,
        binding_count,
    })
}

/// The workspace's integrations, including revoked ones.
pub async fn list_integrations(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<IntegrationBody>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let records = state.store.list_integrations(&workspace_id).await?;
    let mut result = Vec::with_capacity(records.len());
    for record in records {
        result.push(integration_body(&state, record).await?);
    }
    Ok(Json(result))
}

/// Create an integration.
///
/// The configuration is stored as the JSON the caller sent. It is checked for
/// being an object — an array or a bare string is a client bug worth reporting
/// at the boundary rather than when the adapter fails to parse it at startup —
/// and otherwise passed through, because only the adapter knows its own shape.
pub async fn create_integration(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateIntegrationRequest>,
) -> Result<(StatusCode, Json<IntegrationBody>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow creating an integration",
        ));
    }

    let kind = body.kind.parse::<IntegrationKind>().map_err(|_| {
        ApiError::bad_request(format!("'{}' is not a known integration kind", body.kind))
    })?;
    let label = body.label.trim();
    if label.is_empty() {
        return Err(ApiError::bad_request("an integration needs a label"));
    }
    if !body.config.is_object() {
        return Err(ApiError::bad_request(
            "config must be a JSON object; its shape depends on the adapter",
        ));
    }

    let new = NewIntegration::new(&workspace_id, kind, label, body.config.to_string());
    state.store.create_integration(&new).await?;
    audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "integration.created",
        new.id.0.clone(),
    )
    .await?;

    let record = state
        .store
        .get_integration(&new.id)
        .await?
        .ok_or_else(|| ApiError::not_found("the integration was not found"))?;
    Ok((
        StatusCode::CREATED,
        Json(integration_body(&state, record).await?),
    ))
}

/// Retire an integration.
///
/// The row is kept and only `revoked_at` is set: the bindings and principals stay
/// readable, so an operator can see what a retired bot was connected to, and the
/// supervisor simply stops starting it.
pub async fn revoke_integration(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, integration_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow revoking an integration",
        ));
    }
    let integration_id = IntegrationId::new(integration_id);

    // Resolve through the workspace so a row from another deployment reads as
    // missing rather than as forbidden.
    let Some(record) = state.store.get_integration(&integration_id).await? else {
        return Err(ApiError::not_found("the integration was not found"));
    };
    if record.workspace_id != workspace_id {
        return Err(ApiError::not_found("the integration was not found"));
    }

    state.store.revoke_integration(&integration_id).await?;
    audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "integration.revoked",
        integration_id.0,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The external conversations that feed this integration's channels.
pub async fn list_bindings(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, integration_id)): Path<(String, String)>,
) -> Result<Json<Vec<BindingBody>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    let integration_id = require_integration(&state, &workspace_id, &integration_id).await?;
    let bindings = state
        .store
        .list_integration_bindings(&integration_id)
        .await?;
    Ok(Json(bindings.into_iter().map(binding_body).collect()))
}

/// Bind a platform conversation to a channel.
///
/// `agent_id` is required for a group channel and pointless for a direct
/// conversation. It is not enforced here: which agent a bot *is* depends on how
/// the deployment bound it, and a binding that names none still stores messages —
/// it just cannot turn an IM mention into `@Agent`.
pub async fn bind_conversation(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, integration_id)): Path<(String, String)>,
    Json(body): Json<BindRequest>,
) -> Result<(StatusCode, Json<BindingBody>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow binding a conversation",
        ));
    }
    let integration_id = require_integration(&state, &workspace_id, &integration_id).await?;

    let external_id = body.external_id.trim();
    if external_id.is_empty() {
        return Err(ApiError::bad_request(
            "a binding needs the platform's conversation id",
        ));
    }
    let channel_id = ChannelId::new(body.channel_id);
    // The channel must be in this workspace: a binding is a write path into a
    // channel, so pointing it at another team's channel would post there.
    let channel_workspace = state.store.channel_workspace(&channel_id).await?;
    if channel_workspace.as_ref() != Some(&workspace_id) {
        return Err(ApiError::not_found("the channel was not found"));
    }

    let agent_id = match body.agent_id.map(AgentId) {
        Some(agent_id) => {
            // `get_agent` is not workspace-scoped, so the scoped lookup is what
            // keeps a caller from naming another team's agent and having the
            // binding translate mentions into a name this workspace never
            // answers.
            if !state
                .store
                .agent_in_workspace(&agent_id, &workspace_id)
                .await?
            {
                return Err(ApiError::not_found("the agent was not found"));
            }
            Some(agent_id)
        }
        None => None,
    };

    let binding = IntegrationBinding {
        integration_id: integration_id.clone(),
        external_id: external_id.to_string(),
        channel_id,
        agent_id,
    };
    state.store.bind_integration_channel(&binding).await?;
    audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "integration.bound",
        format!("{}:{}", integration_id.0, binding.external_id),
    )
    .await?;

    // Ask the adapter whether the bot can actually reach this conversation. A
    // start-of-life check would mean the operator binds a channel, gets a 201,
    // and hears nothing until the next restart; asking here puts the answer in
    // the response they are looking at.
    let health = binding_health(&state, &integration_id, &binding.external_id).await;
    let mut body = binding_body(binding);
    if let Some(health) = health {
        body.health = Some(health_name(&health).to_string());
        body.health_detail = health.problem().map(str::to_string);
    }
    Ok((StatusCode::CREATED, Json(body)))
}

/// Ask the adapter behind an integration about one conversation.
///
/// `None` when the configuration no longer parses — the binding is still stored
/// and the next startup will report the configuration problem, so failing the
/// bind over it would tell the caller nothing they can act on here.
async fn binding_health(
    state: &AppState,
    integration_id: &IntegrationId,
    external_id: &str,
) -> Option<cowbot_integrations::BindingHealth> {
    let record = state
        .store
        .get_integration(integration_id)
        .await
        .ok()
        .flatten()?;
    let config = state
        .store
        .integration_config(integration_id)
        .await
        .ok()
        .flatten()?;
    let adapter = crate::adapters::registry()
        .build(record.kind, &config)
        .ok()
        .flatten()?;
    Some(adapter.check_binding(external_id).await)
}

fn health_name(health: &cowbot_integrations::BindingHealth) -> &'static str {
    match health {
        cowbot_integrations::BindingHealth::Reachable => "reachable",
        cowbot_integrations::BindingHealth::Unreachable(_) => "unreachable",
        cowbot_integrations::BindingHealth::Unknown(_) => "unknown",
    }
}

/// Remove one binding.
pub async fn unbind_conversation(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, integration_id, external_id)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow unbinding a conversation",
        ));
    }
    let integration_id = require_integration(&state, &workspace_id, &integration_id).await?;
    if !state
        .store
        .unbind_integration_channel(&integration_id, &external_id)
        .await?
    {
        return Err(ApiError::not_found("the binding was not found"));
    }
    audit(
        &state,
        &workspace_id,
        &user.user.id.0,
        "integration.unbound",
        format!("{}:{external_id}", integration_id.0),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Resolve an integration id inside a workspace, or report it missing.
async fn require_integration(
    state: &AppState,
    workspace_id: &WorkspaceId,
    integration_id: &str,
) -> Result<IntegrationId, ApiError> {
    let id = IntegrationId::new(integration_id);
    let Some(record) = state.store.get_integration(&id).await? else {
        return Err(ApiError::not_found("the integration was not found"));
    };
    if &record.workspace_id != workspace_id {
        return Err(ApiError::not_found("the integration was not found"));
    }
    Ok(id)
}

fn binding_body(binding: IntegrationBinding) -> BindingBody {
    BindingBody {
        integration_id: binding.integration_id.0,
        external_id: binding.external_id,
        channel_id: binding.channel_id.0,
        agent_id: binding.agent_id.map(|agent| agent.0),
        health: None,
        health_detail: None,
    }
}
