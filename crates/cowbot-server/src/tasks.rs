//! Task, assignment, run, approval, audit, permission-rule, and agent routes.
//!
//! Every task-shaped route resolves the task, then the workspace that owns it,
//! then the caller's role. A task in another workspace is `404` (ADR-0045).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use cowbot_core::agent::AgentProfile;
use cowbot_core::approval::{Approval, BypassCritical, WorkspacePolicy};
use cowbot_core::assignment::{Assignment, AssignmentEvent};
use cowbot_core::audit::AuditEvent;
use cowbot_core::channel::ChannelId;
use cowbot_core::identity::{Role, WorkspaceId};
use cowbot_core::permission::{PermissionRule, RuleDecision};
use cowbot_core::run::{Run, ToolCallRecord};
use cowbot_core::task::{Task, TaskId};
use cowbot_service::AgentUpdate;
use serde::Deserialize;

use crate::auth::AuthenticatedUser;
use crate::error::ApiError;
use crate::router::AppState;
use crate::workspaces::require_member;

/// Parse the approval level names the CLI and desktop use.
fn parse_approval_level(value: &str) -> Option<cowbot_core::approval::ApprovalLevel> {
    use cowbot_core::approval::ApprovalLevel;
    match value {
        "ask" | "manual" => Some(ApprovalLevel::Ask),
        "accept-edits" | "accept_edits" => Some(ApprovalLevel::AcceptEdits),
        "safe-auto" | "safe_auto" => Some(ApprovalLevel::SafeAuto),
        "plan-only" | "plan_only" | "plan" => Some(ApprovalLevel::PlanOnly),
        "trusted" => Some(ApprovalLevel::Trusted),
        _ => None,
    }
}

/// The workspace that owns a task.
async fn task_workspace(state: &AppState, task_id: &TaskId) -> Result<WorkspaceId, ApiError> {
    state
        .store
        .task_workspace(task_id)
        .await?
        .ok_or_else(|| ApiError::not_found("the task was not found"))
}

/// A task plus the caller's role in its workspace.
async fn task_with_role(
    state: &AppState,
    user: &AuthenticatedUser,
    task_id: &TaskId,
) -> Result<(Task, Role), ApiError> {
    let Some(task) = state.store.get_task(task_id).await? else {
        return Err(ApiError::not_found("the task was not found"));
    };
    let workspace_id = task_workspace(state, task_id).await?;
    let role = require_member(state, user, &workspace_id).await?;
    Ok((task, role))
}

/// Tasks of one workspace.
pub async fn list_tasks(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<Task>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    Ok(Json(state.tasks().list(&workspace_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    pub prompt: String,
    pub assignee: String,
    pub channel_id: Option<String>,
    #[serde(default)]
    pub approval_level: Option<String>,
}

/// Create a task and queue its root assignment.
pub async fn create_task(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<Task>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow creating tasks",
        ));
    }
    // The assignee comes from the body, so it must be an agent of this
    // workspace; otherwise a task could name another tenant's agent.
    let assignee = cowbot_core::agent::AgentId::new(body.assignee.clone());
    if !state
        .store
        .agent_in_workspace(&assignee, &workspace_id)
        .await?
    {
        return Err(ApiError::not_found("the agent was not found"));
    }
    let mut task = Task::new(body.title.trim(), body.prompt.trim()).assign_to(assignee);
    if let Some(channel_id) = &body.channel_id {
        // A channel named in the body must belong to this workspace, or a
        // caller could attach a task to another tenant's channel.
        let channel_id = ChannelId::new(channel_id.clone());
        if !state
            .store
            .channel_in_workspace(&channel_id, &workspace_id)
            .await?
        {
            return Err(ApiError::not_found("the channel was not found"));
        }
        // Carry the channel on the task itself: `Task::channel_id` is skipped
        // when `None`, and the store reads the column from the same model, so
        // without this the persisted row, the `201` body, and the published
        // `TaskCreated` event would all disagree with local mode.
        task = task.with_channel(channel_id);
    }
    let approval_level = match body.approval_level.as_deref() {
        Some(value) => parse_approval_level(value)
            .ok_or_else(|| ApiError::bad_request(format!("unknown approval level '{value}'")))?,
        None => cowbot_core::approval::ApprovalLevel::Ask,
    };
    state
        .tasks()
        .create(
            &workspace_id,
            task.channel_id.as_ref(),
            &task,
            cowbot_core::assignment::AssignmentRequester::User(user.user.id.0.clone()),
            approval_level,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(task)))
}

/// One task.
pub async fn get_task(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(task_id): Path<String>,
) -> Result<Json<Task>, ApiError> {
    let (task, _) = task_with_role(&state, &user, &TaskId::new(task_id)).await?;
    Ok(Json(task))
}

/// Cancel a task. Requires write access.
pub async fn cancel_task(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(task_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let task_id = TaskId::new(task_id);
    let (_, role) = task_with_role(&state, &user, &task_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow cancelling tasks",
        ));
    }
    state
        .tasks()
        .cancel(&task_id, format!("cancelled by {}", user.user.id.0))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ArchiveRequest {
    pub archived: bool,
}

pub async fn archive_task(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(task_id): Path<String>,
    Json(body): Json<ArchiveRequest>,
) -> Result<Json<Task>, ApiError> {
    let task_id = TaskId::new(task_id);
    let (_, role) = task_with_role(&state, &user, &task_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow archiving tasks",
        ));
    }
    Ok(Json(
        state.tasks().set_archived(&task_id, body.archived).await?,
    ))
}

/// Approve a Plan-mode task's artifact.
pub async fn approve_task_plan(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(task_id): Path<String>,
) -> Result<Json<Task>, ApiError> {
    let task_id = TaskId::new(task_id);
    let (_, role) = task_with_role(&state, &user, &task_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow approving a plan",
        ));
    }
    Ok(Json(state.tasks().approve_plan(&task_id).await?))
}

pub async fn list_assignments(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<Assignment>>, ApiError> {
    let task_id = TaskId::new(task_id);
    task_with_role(&state, &user, &task_id).await?;
    Ok(Json(state.store.list_assignments_for_task(&task_id).await?))
}

pub async fn list_runs(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<Run>>, ApiError> {
    let task_id = TaskId::new(task_id);
    task_with_role(&state, &user, &task_id).await?;
    Ok(Json(state.store.list_runs(&task_id).await?))
}

/// A run plus the caller's role in its task's workspace.
async fn run_with_role(
    state: &AppState,
    user: &AuthenticatedUser,
    run_id: &cowbot_core::run::RunId,
) -> Result<Run, ApiError> {
    let Some(run) = state.store.get_run(run_id).await? else {
        return Err(ApiError::not_found("the run was not found"));
    };
    let workspace_id = task_workspace(state, &run.task_id).await?;
    require_member(state, user, &workspace_id).await?;
    Ok(run)
}

pub async fn get_run(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(run_id): Path<String>,
) -> Result<Json<Run>, ApiError> {
    Ok(Json(
        run_with_role(&state, &user, &cowbot_core::run::RunId::new(run_id)).await?,
    ))
}

pub async fn list_tool_calls(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(run_id): Path<String>,
) -> Result<Json<Vec<ToolCallRecord>>, ApiError> {
    let run_id = cowbot_core::run::RunId::new(run_id);
    run_with_role(&state, &user, &run_id).await?;
    Ok(Json(state.store.list_tool_calls(&run_id).await?))
}

pub async fn list_assignment_events(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(assignment_id): Path<String>,
) -> Result<Json<Vec<AssignmentEvent>>, ApiError> {
    let assignment_id = cowbot_core::assignment::AssignmentId::new(assignment_id);
    let Some(assignment) = state.store.get_assignment(&assignment_id).await? else {
        return Err(ApiError::not_found("the assignment was not found"));
    };
    let workspace_id = task_workspace(&state, &assignment.task_id).await?;
    require_member(&state, &user, &workspace_id).await?;
    Ok(Json(
        state.store.list_assignment_events(&assignment_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    #[serde(default = "default_audit_limit")]
    pub limit: u32,
    pub source: Option<String>,
}

fn default_audit_limit() -> u32 {
    100
}

pub async fn list_audit(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEvent>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow reading the audit log",
        ));
    }
    Ok(Json(
        state
            .store
            .list_audit_events_by_source(
                &workspace_id,
                query.source.as_deref(),
                query.limit.clamp(1, 500),
            )
            .await?,
    ))
}

/// Pending approvals of one workspace.
pub async fn pending_approvals(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<Approval>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    Ok(Json(
        state
            .approvals(&workspace_id)
            .pending_for_workspace()
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct DecideApprovalRequest {
    pub approved: bool,
    /// Remember the decision for this process, as the desktop's session
    /// decisions do.
    #[serde(default)]
    pub for_session: bool,
}

pub async fn decide_approval(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(approval_id): Path<String>,
    Json(body): Json<DecideApprovalRequest>,
) -> Result<Json<Approval>, ApiError> {
    let approval_id = cowbot_core::approval::ApprovalId::new(approval_id);
    // An approval names a task; its workspace decides who may act on it.
    let Some(approval) = state.store.get_approval(&approval_id).await? else {
        return Err(ApiError::not_found("the approval was not found"));
    };
    let workspace_id = task_workspace(&state, &approval.task_id).await?;
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_write() {
        return Err(ApiError::forbidden(
            "your role does not allow deciding approvals",
        ));
    }
    let service = state.approvals(&workspace_id);
    let status = if body.approved {
        cowbot_core::approval::ApprovalStatus::Approved
    } else {
        cowbot_core::approval::ApprovalStatus::Rejected
    };
    let decided_by = user.user.id.0.clone();
    let decided = if body.for_session {
        service
            .decide_for_session(&approval_id, status, &decided_by)
            .await?
    } else {
        service.decide(&approval_id, status, &decided_by).await?
    };
    Ok(Json(decided))
}

/// Workspace permission rules.
pub async fn list_permission_rules(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<PermissionRule>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    Ok(Json(state.permissions(&workspace_id).list().await?))
}

#[derive(Debug, Deserialize)]
pub struct CreateRuleRequest {
    pub tool: String,
    pub command: Option<String>,
    pub decision: RuleDecision,
}

pub async fn create_permission_rule(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(raw): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<PermissionRule>), ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow changing permission rules",
        ));
    }
    let body: CreateRuleRequest = crate::error::parse_body(raw)?;
    let rule = state
        .permissions(&workspace_id)
        .create(&body.tool, body.command.as_deref(), body.decision)
        .await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

#[derive(Debug, Deserialize)]
pub struct RuleEnabledRequest {
    pub enabled: bool,
}

pub async fn set_permission_rule_enabled(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, rule_id)): Path<(String, String)>,
    Json(body): Json<RuleEnabledRequest>,
) -> Result<Json<PermissionRule>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow changing permission rules",
        ));
    }
    let service = state.permissions(&workspace_id);
    // A rule id from another workspace must not be reachable here.
    if !service.list().await?.iter().any(|rule| rule.id == rule_id) {
        return Err(ApiError::not_found("the rule was not found"));
    }
    Ok(Json(service.set_enabled(&rule_id, body.enabled).await?))
}

pub async fn delete_permission_rule(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path((workspace_id, rule_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow changing permission rules",
        ));
    }
    let service = state.permissions(&workspace_id);
    if !service.list().await?.iter().any(|rule| rule.id == rule_id) {
        return Err(ApiError::not_found("the rule was not found"));
    }
    service.delete(&rule_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn workspace_policy(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<WorkspacePolicy>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    Ok(Json(state.permissions(&workspace_id).policy().await?))
}

pub async fn set_bypass_critical(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<WorkspacePolicy>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow changing the approval policy",
        ));
    }
    // The body is read after the role check, so an unprivileged caller learns
    // nothing about which values are valid.
    let setting: BypassCritical = serde_json::from_value(
        body.get("bypass_critical")
            .cloned()
            .ok_or_else(|| ApiError::bad_request("bypass_critical is required"))?,
    )
    .map_err(|_| ApiError::bad_request("unknown bypass_critical value"))?;
    Ok(Json(
        state
            .permissions(&workspace_id)
            .set_bypass_critical(setting)
            .await?,
    ))
}

/// Agents of one workspace.
pub async fn list_agents(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<AgentProfile>>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    require_member(&state, &user, &workspace_id).await?;
    Ok(Json(state.store.list_agents(&workspace_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    pub id: String,
    pub name: Option<String>,
    pub role: Option<String>,
    pub tools: Option<Vec<String>>,
}

pub async fn update_agent(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(workspace_id): Path<String>,
    Json(body): Json<UpdateAgentRequest>,
) -> Result<Json<AgentProfile>, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let role = require_member(&state, &user, &workspace_id).await?;
    if !role.can_administer() {
        return Err(ApiError::forbidden(
            "your role does not allow changing agents",
        ));
    }
    let agent_id = cowbot_core::agent::AgentId::new(body.id);
    // An agent id from the body must belong to the workspace in the path, or an
    // admin of one workspace could rewrite another workspace's agent by id.
    // This is also what makes the refusal a `404`: `AgentService::update_agent`
    // enforces the same rule, but reports it as a bad request.
    if !state
        .store
        .agent_in_workspace(&agent_id, &workspace_id)
        .await?
    {
        return Err(ApiError::not_found("the agent was not found"));
    }
    let Some(existing) = state.store.get_agent(&agent_id).await? else {
        return Err(ApiError::not_found("the agent was not found"));
    };
    // `AgentUpdate` replaces the whole profile, so a field the caller omits
    // keeps its current value rather than being cleared.
    let agent = state
        .agents()
        .update_agent(
            &workspace_id,
            &AgentUpdate {
                id: agent_id,
                name: body.name.unwrap_or(existing.name),
                role: body.role.unwrap_or(existing.role),
                tools: body.tools.unwrap_or_else(|| existing.tools.clone()),
            },
        )
        .await?;
    Ok(Json(agent))
}
