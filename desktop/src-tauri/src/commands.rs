use cowbot_agent::{AgentInstance, Crew, StandardRoles};
use cowbot_client::{
    AgentUpdate, BindingView, Client, ConversationSummary, IntegrationView, InviteGrant,
    InviteView, MemberView, MemoryFilter, MemoryNotes, MemorySave, MemorySettingsUpdate,
    MemoryUpdate, NewBinding, NewConversation, NewIntegrationRequest, NewMemory, NewTask, NoteSave,
    RecalledMemory,
};
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::{Approval, ApprovalId, ApprovalLevel, BypassCritical, WorkspacePolicy};
use cowbot_core::assignment::{
    Assignment, AssignmentEvent, AssignmentId, AssignmentOutcome, AssignmentStatus,
};
use cowbot_core::audit::AuditEvent;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage};
use cowbot_core::command::{
    command_catalogue, parse_command as command_parse, CommandParse, CommandSpec,
};
use cowbot_core::conversation::{
    context_usage_state, usable_window, CacheUsageSummary, ChannelParticipant, CompactionOutcome,
    ContextProjection, ContextUsageState, ConversationContext, ConversationSettingsUpdate,
    ConversationTurn, RoutingMode, TurnId,
};
use cowbot_core::identity::{Role, UserId, WorkspaceId};
use cowbot_core::integration::{IntegrationId, IntegrationKind};
use cowbot_core::memory::{MemoryEntry, MemoryKind, MemorySettings, MemoryStatus};
use cowbot_core::permission::{PermissionRule, RuleDecision};
use cowbot_core::prompt::{PromptKind, PromptPlan};
use cowbot_core::prompt_snapshot::{
    PromptSnapshotDetail, PromptSnapshotSettings, PromptSnapshotSummary,
};
use cowbot_core::run::{Run, RunId, ToolCallRecord};
use cowbot_core::task::{Task, TaskId};
use cowbot_core::template::TemplateDocument;
use cowbot_core::tool::ToolDefinition;
use cowbot_core::usage::TokenSource;
use cowbot_core::worktree::{TaskWorktree, TaskWorktreeDiff, WorktreeStatus};
use cowbot_llm::{token_counter_for_model, ChatCompletionRequest, ChatMessage, LlmProvider};
use cowbot_service::{ChannelTurnOptions, ParticipantCapacity};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{config, startup, AppState};

/// Payload emitted on `cowbot-run-finished` when a crew run settles.
#[derive(Debug, Clone, Serialize)]
pub struct RunFinished {
    pub task_id: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Payload emitted on `cowbot-channel-reply` when a channel turn settles.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelReplyFinished {
    pub message_id: String,
    pub reply_count: usize,
    pub error: Option<String>,
    /// Group responders whose answer was posted.
    pub replied: Vec<String>,
    /// Group responders whose task failed.
    pub failed: Vec<String>,
    /// Group responders skipped because their branch was full.
    pub skipped: Vec<String>,
    /// Responders selected by mention, router, or default resolution.
    pub selected: Vec<String>,
    /// Route source of a group turn (`mention`, `router`, `default`, ...).
    pub route: Option<String>,
    /// Persisted group turn row.
    pub turn_id: Option<String>,
}

/// The client as a local runtime, for operations that need files, a worktree,
/// or an in-process crew. Server mode answers with a clear error instead of
/// pretending the operation worked.
fn local(state: &AppState) -> Result<&cowbot_client::LocalClient, String> {
    state.local_client.as_deref().ok_or_else(|| {
        "this action needs the local runtime and is not available in server mode".to_string()
    })
}

#[tauri::command]
pub async fn get_session(
    state: State<'_, AppState>,
) -> Result<cowbot_client::ClientSession, String> {
    state
        .client
        .session()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_user_profile(
    state: State<'_, AppState>,
    name: String,
    email: String,
) -> Result<cowbot_client::ClientSession, String> {
    state
        .client
        .update_user_profile(&name, &email)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_channels(state: State<'_, AppState>) -> Result<Vec<Channel>, String> {
    state
        .client
        .list_channels()
        .await
        .map_err(|error| error.to_string())
}

/// Return every channel, provisioning a direct conversation for each agent
/// that does not have one yet.
///
/// Provisioning is a local-runtime write; a server workspace already has its
/// conversations, so it simply lists what exists.
#[tauri::command]
pub async fn ensure_agent_channels(state: State<'_, AppState>) -> Result<Vec<Channel>, String> {
    match state.local_client.as_deref() {
        Some(client) => client
            .ensure_agent_channels()
            .await
            .map_err(|error| error.to_string()),
        None => state
            .client
            .list_channels()
            .await
            .map_err(|error| error.to_string()),
    }
}

#[tauri::command]
pub async fn create_channel(
    state: State<'_, AppState>,
    name: String,
    topic: String,
) -> Result<Channel, String> {
    state
        .client
        .create_channel(&name, &topic)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn rename_channel(
    state: State<'_, AppState>,
    channel_id: String,
    name: String,
    topic: String,
) -> Result<Channel, String> {
    state
        .client
        .rename_channel(&ChannelId::new(channel_id), &name, &topic)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_channel(
    state: State<'_, AppState>,
    channel_id: String,
) -> Result<Channel, String> {
    state
        .client
        .delete_channel(&ChannelId::new(channel_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_channel_pinned(
    state: State<'_, AppState>,
    channel_id: String,
    pinned: bool,
) -> Result<Channel, String> {
    state
        .client
        .set_channel_pinned(&ChannelId::new(channel_id), pinned)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn clear_channel_messages(
    state: State<'_, AppState>,
    channel_id: String,
) -> Result<u64, String> {
    state
        .client
        .clear_channel_messages(&ChannelId::new(channel_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_messages(
    state: State<'_, AppState>,
    channel_id: String,
    limit: u32,
) -> Result<Vec<ChannelMessage>, String> {
    state
        .client
        .list_messages(&ChannelId::new(channel_id), limit)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn post_message(
    state: State<'_, AppState>,
    app: AppHandle,
    channel_id: String,
    content: String,
    approval_level: Option<ApprovalLevel>,
) -> Result<ChannelMessage, String> {
    let channel_id = ChannelId::new(channel_id);
    let channel = state
        .client
        .get_channel(&channel_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("channel '{}' was not found", channel_id.0))?;
    let approval_level = approval_level.unwrap_or(ApprovalLevel::Ask);

    // The capacity rule lives in the service (`ChannelAgentService::reply`), so
    // every transport applies it the same way. The desktop keeps one cheap
    // pre-send refusal on top of it: a conversation that is already full cannot
    // get an answer, and saying so before the message is stored beats storing
    // it and posting a notice. Auto-compaction moved into the service for the
    // same reason — it needs the provider, and only the service has it on every
    // transport (ADR-0055).
    let mut participant_capacity = Vec::new();
    let (llm, model) = if channel.is_conversation() {
        let settings = state
            .client
            .conversation_settings(&channel_id)
            .await
            .map_err(|error| error.to_string())?;
        let runtime = conversation_runtime(&state, &settings);
        let counter = token_counter_for_model(&settings.model);
        let view = conversation_context_view(&state, &channel_id).await?;

        if view.participants.len() <= 1 {
            let pending = counter.count_message(content.trim());
            if let Some(usable) = view.usable_tokens {
                if view.total_tokens.saturating_add(pending) >= usable {
                    return Err(
                        "This conversation's context is full. Compact the context, clear it, or \
                         start a new conversation before sending."
                            .to_string(),
                    );
                }
            }
        }

        if let Some(usable) = view.usable_tokens {
            participant_capacity = view
                .participants
                .iter()
                .map(|participant| ParticipantCapacity {
                    agent_id: AgentId::new(participant.agent_id.clone()),
                    usable_window: usable,
                    tool_tokens: participant.tool_tokens,
                })
                .collect();
        }
        runtime
    } else {
        state.runtime_model()
    };

    let message = state
        .client
        .post_message(&channel_id, &content)
        .await
        .map_err(|error| error.to_string())?;

    // Agents answer in the background so the composer stays responsive; the
    // timeline refreshes from MessagePosted events and the turn-finished event.
    // The reply runs in-process, so server mode posts without a local turn.
    let Some(client) = state.local_client.clone() else {
        return Ok(message);
    };
    let tools = state.tools.clone();
    let channel = channel_id.clone();
    let posted = message.clone();
    let reply_id = message.id.clone();
    let running = state.replies.clone();
    let use_model_permissions = state.uses_remote_model();
    let platform_context = project_platform_context(&state);
    if !running.lock().insert(reply_id.clone()) {
        return Ok(message);
    }

    tauri::async_runtime::spawn(async move {
        let outcome = client
            .reply_to_mentions(
                &channel,
                &posted,
                llm,
                tools,
                &model,
                ChannelTurnOptions {
                    approval_level,
                    max_concurrency: 4,
                    use_model_permissions,
                    // The desktop draws the approval card on screen, so the
                    // service's five-minute default is right for it (ADR-0060).
                    approval_timeout: None,
                    platform_context,
                    participant_capacity,
                },
            )
            .await;
        running.lock().remove(&reply_id);

        let payload = match outcome {
            Ok(report) if report.is_empty() && report.route.is_none() => None,
            Ok(report) => Some(ChannelReplyFinished {
                message_id: reply_id,
                reply_count: report.agents.len(),
                error: None,
                replied: report.replied.iter().map(|agent| agent.0.clone()).collect(),
                failed: report.failed.iter().map(|agent| agent.0.clone()).collect(),
                skipped: report.skipped.iter().map(|agent| agent.0.clone()).collect(),
                selected: report.agents.iter().map(|agent| agent.0.clone()).collect(),
                route: report.route.map(|source| source.as_str().to_string()),
                turn_id: report.turn_id.map(|turn| turn.0),
            }),
            Err(error) => Some(ChannelReplyFinished {
                message_id: reply_id,
                reply_count: 0,
                error: Some(error.to_string()),
                replied: Vec::new(),
                failed: Vec::new(),
                skipped: Vec::new(),
                selected: Vec::new(),
                route: None,
                turn_id: None,
            }),
        };
        if let Some(payload) = payload {
            let _ = app.emit("cowbot-channel-reply", payload);
        }
    });

    Ok(message)
}

#[tauri::command]
pub async fn list_integrations(state: State<'_, AppState>) -> Result<Vec<IntegrationView>, String> {
    state
        .client
        .list_integrations()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_integration(
    state: State<'_, AppState>,
    kind: String,
    label: String,
    config: serde_json::Value,
) -> Result<IntegrationView, String> {
    let kind = kind
        .parse::<IntegrationKind>()
        .map_err(|_| format!("'{kind}' is not a known integration kind"))?;
    state
        .client
        .create_integration(&NewIntegrationRequest {
            kind,
            label,
            config,
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn revoke_integration(
    state: State<'_, AppState>,
    integration_id: String,
) -> Result<(), String> {
    state
        .client
        .revoke_integration(&IntegrationId::new(integration_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_integration_bindings(
    state: State<'_, AppState>,
    integration_id: String,
) -> Result<Vec<BindingView>, String> {
    state
        .client
        .list_integration_bindings(&IntegrationId::new(integration_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn bind_integration_channel(
    state: State<'_, AppState>,
    integration_id: String,
    external_id: String,
    channel_id: String,
    agent_id: Option<String>,
) -> Result<BindingView, String> {
    state
        .client
        .bind_integration_channel(
            &IntegrationId::new(integration_id),
            &NewBinding {
                external_id,
                channel_id: ChannelId::new(channel_id),
                agent_id: agent_id.map(AgentId::new),
            },
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn unbind_integration_channel(
    state: State<'_, AppState>,
    integration_id: String,
    external_id: String,
) -> Result<(), String> {
    state
        .client
        .unbind_integration_channel(&IntegrationId::new(integration_id), &external_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_agents(state: State<'_, AppState>) -> Result<Vec<AgentProfile>, String> {
    state
        .client
        .list_agents()
        .await
        .map_err(|error| error.to_string())
}

/// Everyone in the current workspace, with their role.
#[tauri::command]
pub async fn get_members(state: State<'_, AppState>) -> Result<Vec<MemberView>, String> {
    state
        .client
        .list_members()
        .await
        .map_err(|error| error.to_string())
}

/// Change a member's role. The server decides whether this caller may.
#[tauri::command]
pub async fn set_member_role(
    state: State<'_, AppState>,
    user_id: String,
    role: Role,
) -> Result<MemberView, String> {
    state
        .client
        .set_member_role(&UserId::new(user_id), role)
        .await
        .map_err(|error| error.to_string())
}

/// Remove a member from the workspace. Their account and history stay.
#[tauri::command]
pub async fn remove_member(state: State<'_, AppState>, user_id: String) -> Result<(), String> {
    state
        .client
        .remove_member(&UserId::new(user_id))
        .await
        .map_err(|error| error.to_string())
}

/// Invitations that are still redeemable, without their tokens.
#[tauri::command]
pub async fn list_invites(state: State<'_, AppState>) -> Result<Vec<InviteView>, String> {
    state
        .client
        .list_invites()
        .await
        .map_err(|error| error.to_string())
}

/// Mint an invitation and return its token. This is the only copy of it.
#[tauri::command]
pub async fn create_invite(state: State<'_, AppState>, role: Role) -> Result<InviteGrant, String> {
    state
        .client
        .create_invite(role)
        .await
        .map_err(|error| error.to_string())
}

/// Cancel an invitation nobody has redeemed yet.
#[tauri::command]
pub async fn revoke_invite(state: State<'_, AppState>, invite_id: String) -> Result<(), String> {
    state
        .client
        .revoke_invite(&invite_id)
        .await
        .map_err(|error| error.to_string())
}

/// Join a workspace this account was invited to, without leaving the current
/// one. The screen offers the new workspace in its picker afterwards; switching
/// to it is the existing restart-into-it path.
#[tauri::command]
pub async fn accept_invite(
    state: State<'_, AppState>,
    token: String,
) -> Result<ServerWorkspaceView, String> {
    let joined = state
        .client
        .accept_invite(&token)
        .await
        .map_err(|error| error.to_string())?;
    Ok(ServerWorkspaceView {
        id: joined.id,
        name: joined.name,
        role: joined.role,
    })
}

#[tauri::command]
pub async fn update_agent(
    state: State<'_, AppState>,
    agent_id: String,
    name: String,
    role: String,
    tools: Vec<String>,
) -> Result<AgentProfile, String> {
    state
        .client
        .update_agent(&AgentUpdate {
            id: AgentId::new(agent_id),
            name,
            role,
            tools,
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_available_tools(state: State<'_, AppState>) -> Vec<ToolDefinition> {
    state.tools.definitions()
}

#[tauri::command]
pub async fn list_templates(state: State<'_, AppState>) -> Result<Vec<TemplateDocument>, String> {
    state
        .client
        .list_templates()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn instantiate_agents(
    state: State<'_, AppState>,
    template_id: String,
) -> Result<Vec<AgentProfile>, String> {
    state
        .client
        .instantiate_agents(&template_id)
        .await
        .map_err(|error| error.to_string())
}

/// Project and base branch chosen for a worktree task.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeSelection {
    pub project_id: String,
    pub base_branch: String,
}

#[tauri::command]
pub async fn create_task(
    state: State<'_, AppState>,
    description: String,
    expected_output: String,
    assignee: String,
    channel_id: Option<String>,
    approval_level: ApprovalLevel,
    worktree: Option<WorktreeSelection>,
) -> Result<Task, String> {
    // Resolve the project and branch before creating anything, so a bad
    // selection leaves no orphaned task behind.
    let requested_worktree = match &worktree {
        Some(selection) => {
            let config = load_config(&state)?;
            let project = config
                .project(&selection.project_id)
                .cloned()
                .ok_or_else(|| format!("project '{}' is not registered", selection.project_id))?;
            let branches = config::project_branches(&project.path)
                .map_err(|error| format!("project '{}' is not available: {error}", project.name))?;
            if selection.base_branch.trim().is_empty() {
                return Err("Choose a base branch for the worktree.".to_string());
            }
            if !branches
                .branches
                .iter()
                .any(|branch| branch == &selection.base_branch)
            {
                return Err(format!(
                    "base branch '{}' does not exist in '{}'. Available branches: {}",
                    selection.base_branch,
                    project.name,
                    branches.branches.join(", ")
                ));
            }
            Some((project, selection.base_branch.clone()))
        }
        None => None,
    };

    let mut task = NewTask::new(description, expected_output, AgentId::new(assignee));
    if let Some(channel_id) = channel_id {
        task = task.in_channel(ChannelId::new(channel_id));
    }
    task = task.with_approval_level(approval_level);

    let created = state
        .client
        .create_task(&task)
        .await
        .map_err(|error| error.to_string())?;

    if let Some((project, base_branch)) = requested_worktree {
        local(&state)?
            .request_worktree(&created.id, &project.id, &project.path, &base_branch)
            .await
            .map_err(|error| error.to_string())?;
    }

    Ok(created)
}

#[tauri::command]
pub async fn run_task(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    task_id: String,
    approval_level: ApprovalLevel,
) -> Result<(), String> {
    let task = state
        .client
        .get_task(&TaskId::new(task_id.clone()))
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("task '{task_id}' was not found"))?;

    // Running a task needs this machine; check before preparing anything so a
    // server-mode call leaves no worktree behind.
    let Some(client) = state.local_client.clone() else {
        return Err(
            "running a task needs the local runtime and is not available in server mode"
                .to_string(),
        );
    };

    // A worktree task is prepared before spawning so a git failure rejects
    // the run and the task stays pending.
    let worktree = match local(&state)?
        .task_worktree(&task.id)
        .await
        .map_err(|error| error.to_string())?
    {
        Some(record) => match record.status {
            WorktreeStatus::Requested | WorktreeStatus::Ready => Some(
                local(&state)?
                    .prepare_worktree(&task.id)
                    .await
                    .map_err(|error| format!("The worktree could not be prepared: {error}"))?,
            ),
            WorktreeStatus::Removed => {
                return Err(
                    "This task's worktree was discarded. Create a new task to run in a worktree."
                        .to_string(),
                )
            }
            WorktreeStatus::Missing => {
                return Err(
                    "This task's worktree folder is missing. Discard the worktree and create a new task."
                        .to_string(),
                )
            }
        },
        None => None,
    };

    let running = state.running.clone();
    if !running.lock().insert(task_id.clone()) {
        return Err(format!("task '{task_id}' is already running"));
    }

    let (llm, model) = state.runtime_model();
    let tools = match &worktree {
        Some(record) => AppState::rooted_tools(
            Path::new(&record.worktree_path),
            state.config_path.clone(),
            state.client.clone(),
        ),
        None => state.tools.clone(),
    };
    let run_id = task_id.clone();
    let use_model_permissions = state.uses_remote_model();

    tauri::async_runtime::spawn(async move {
        let outcome: Result<(), String> = async {
            let mut options = cowbot_service::CrewRunOptions {
                approval_level,
                use_model_permissions,
                ..cowbot_service::CrewRunOptions::default()
            };
            // Plan runs and approved plans belong to the stored task: run it
            // with its own assignee so its plan, status, and history stay
            // attached to it. Other runs keep the standard crew pipeline.
            if approval_level == ApprovalLevel::PlanOnly || task.plan_approved {
                let agents = client
                    .list_agents()
                    .await
                    .map_err(|error| error.to_string())?;
                let assignee = task.assigned_agent.clone();
                let profile = agents
                    .into_iter()
                    .find(|agent| Some(&agent.id) == assignee.as_ref())
                    .ok_or_else(|| format!("task '{}' has no assignable agent", task.id))?;
                let agent = AgentInstance::new(profile, llm.clone(), tools.clone(), &model);
                if approval_level == ApprovalLevel::PlanOnly {
                    options.capture_plan_task = Some(task.id.clone());
                }
                client
                    .run_existing_task(&task, agent, options)
                    .await
                    .map_err(|error| error.to_string())?;
            } else {
                let mut crew = build_standard_crew(&task, worktree.as_ref(), &llm, &tools, &model);
                client
                    .run_crew_with_options(&mut crew, options)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        }
        .await;

        running.lock().remove(&run_id);

        let payload = match outcome {
            Ok(()) => RunFinished {
                task_id: run_id,
                ok: true,
                error: None,
            },
            Err(error) => RunFinished {
                task_id: run_id,
                ok: false,
                error: Some(error),
            },
        };
        let _ = app.emit("cowbot-run-finished", payload);
    });

    Ok(())
}

/// Build the Planner/Coder/Reviewer pipeline used by the CLI, with the
/// submitted task's description as the objective.
///
/// Every crew task carries the submitted task's id as its run group, so the
/// run has one owning task for background cards and worktrees.
fn build_standard_crew(
    submitted: &Task,
    worktree: Option<&TaskWorktree>,
    llm: &std::sync::Arc<dyn cowbot_llm::LlmProvider>,
    tools: &cowbot_tools::ToolRegistry,
    model: &str,
) -> Crew {
    let objective = submitted.description.as_str();
    let run_group = submitted.id.clone();
    let checkout_note = worktree
        .map(|worktree| {
            format!(
                "\n\nRun in the git worktree at {} on branch {} (base branch {}). \
                 File and command tools are rooted there; do not modify the main checkout.",
                worktree.worktree_path, worktree.branch, worktree.base_branch
            )
        })
        .unwrap_or_default();
    let planner_profile = StandardRoles::planner();
    let planner_id = planner_profile.id.clone();
    let planner = AgentInstance::new(planner_profile, llm.clone(), tools.clone(), model);

    let coder_profile = StandardRoles::coder();
    let coder_id = coder_profile.id.clone();
    let coder = AgentInstance::new(coder_profile, llm.clone(), tools.clone(), model);

    let reviewer_profile = StandardRoles::reviewer();
    let reviewer_id = reviewer_profile.id.clone();
    let reviewer = AgentInstance::new(reviewer_profile, llm.clone(), tools.clone(), model);

    let mut crew = Crew::new();
    crew.add_agent(planner);
    crew.add_agent(coder);
    crew.add_agent(reviewer);

    let plan_task = Task::new(
        format!("Analyze objective and formulate architecture plan: {objective}{checkout_note}"),
        "A clear, step-by-step implementation and verification plan.",
    )
    .assign_to(planner_id)
    .in_run_group(run_group.clone());
    let plan_id = plan_task.id.clone();

    let code_task = Task::new(
        format!(
            "Execute code implementation and verify functionality in the sandbox.{checkout_note}"
        ),
        "Working source code and sandbox execution results.",
    )
    .assign_to(coder_id)
    .with_context_task(plan_id)
    .in_run_group(run_group.clone());

    let review_task = Task::new(
        format!(
            "Review implementation, tests, and diffs for correctness and edge cases.{checkout_note}"
        ),
        "Comprehensive review findings and final approval status.",
    )
    .assign_to(reviewer_id)
    .with_context_task(code_task.id.clone())
    .in_run_group(run_group);

    crew.add_task(plan_task);
    crew.add_task(code_task);
    crew.add_task(review_task);
    crew
}

#[tauri::command]
pub async fn list_tasks(state: State<'_, AppState>) -> Result<Vec<Task>, String> {
    state
        .client
        .list_tasks()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_run_tasks(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<Task>, String> {
    state
        .client
        .list_run_tasks(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_task_worktree(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Option<TaskWorktree>, String> {
    // A server workspace has no worktree on this machine. Reading one is a
    // detail refresh the task view always performs, so it answers "none"
    // rather than failing the whole panel.
    let Some(client) = state.local_client.as_deref() else {
        return Ok(None);
    };
    client
        .task_worktree(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn task_worktree_diff(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<TaskWorktreeDiff, String> {
    local(&state)?
        .worktree_diff(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn discard_task_worktree(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<TaskWorktree, String> {
    local(&state)?
        .discard_worktree(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

/// One run in the background overview.
#[derive(Debug, Clone, Serialize)]
pub struct TaskOverviewEntry {
    pub run_id: String,
    pub title: String,
    pub channel_id: Option<String>,
    pub status: String,
    pub progress: TaskOverviewProgress,
    pub assignments: Vec<TaskOverviewAssignment>,
    pub worktree: Option<TaskWorktree>,
    pub latest_tool_call: Option<ToolCallRecord>,
    pub started_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskOverviewProgress {
    pub completed: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskOverviewAssignment {
    pub id: String,
    pub assignee: String,
    pub status: AssignmentStatus,
    pub detail: Option<String>,
}

/// Display status for a run, derived from its assignments.
fn overview_status(assignments: &[Assignment]) -> &'static str {
    use AssignmentStatus::*;
    if assignments.is_empty() {
        return "queued";
    }
    if assignments
        .iter()
        .any(|assignment| assignment.status == Running)
    {
        return "running";
    }
    if assignments
        .iter()
        .any(|assignment| assignment.status == NeedsInput)
    {
        return "needs_input";
    }
    if assignments
        .iter()
        .any(|assignment| assignment.status == Waiting)
    {
        return "waiting";
    }
    if assignments
        .iter()
        .any(|assignment| assignment.status == Resume)
    {
        return "resume";
    }
    if assignments
        .iter()
        .any(|assignment| assignment.status == Queued)
    {
        return "queued";
    }
    if assignments
        .iter()
        .any(|assignment| assignment.status == Failed)
    {
        return "failed";
    }
    if assignments
        .iter()
        .all(|assignment| assignment.status == Cancelled)
    {
        return "cancelled";
    }
    "completed"
}

fn overview_detail(assignment: &Assignment) -> Option<String> {
    if let Some(error) = &assignment.last_error {
        return Some(error.clone());
    }
    match &assignment.outcome {
        Some(AssignmentOutcome::Completed { output }) => Some(output.clone()),
        Some(AssignmentOutcome::Failed { error }) => Some(error.clone()),
        Some(AssignmentOutcome::Cancelled { reason }) => Some(reason.clone()),
        None => None,
    }
}

/// Assignments of a run, including its crew tasks.
///
/// The local runtime answers from one query that follows `run_group`; a server
/// workspace combines its assignments route with the tasks it lists, so the
/// background panel shows the same groups in both modes.
async fn run_assignments(state: &AppState, run_id: &TaskId) -> Result<Vec<Assignment>, String> {
    if let Some(client) = state.local_client.as_deref() {
        return client
            .list_run_assignments(run_id)
            .await
            .map_err(|error| error.to_string());
    }
    let mut assignments = state
        .client
        .list_assignments(run_id)
        .await
        .map_err(|error| error.to_string())?;
    let tasks = state
        .client
        .list_tasks()
        .await
        .map_err(|error| error.to_string())?;
    for task in tasks
        .iter()
        .filter(|task| task.run_group.as_ref() == Some(run_id))
    {
        assignments.extend(
            state
                .client
                .list_assignments(&task.id)
                .await
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(assignments)
}

/// The newest tool call in a run, from any task in its group.
async fn latest_run_tool_call(
    state: &AppState,
    run_id: &TaskId,
) -> Result<Option<ToolCallRecord>, String> {
    if let Some(client) = state.local_client.as_deref() {
        return client
            .latest_tool_call_for_task(run_id)
            .await
            .map_err(|error| error.to_string());
    }
    // A server route exists per run, so walk the group's runs and keep the
    // newest call.
    let mut members = vec![run_id.clone()];
    if let Ok(tasks) = state.client.list_tasks().await {
        members.extend(
            tasks
                .into_iter()
                .filter(|task| task.run_group.as_ref() == Some(run_id))
                .map(|task| task.id),
        );
    }
    let mut latest: Option<ToolCallRecord> = None;
    for member in members {
        for run in state
            .client
            .list_runs(&member)
            .await
            .map_err(|error| error.to_string())?
        {
            for call in state
                .client
                .list_tool_calls(&run.id)
                .await
                .map_err(|error| error.to_string())?
            {
                if latest
                    .as_ref()
                    .map(|current| call.created_at > current.created_at)
                    .unwrap_or(true)
                {
                    latest = Some(call);
                }
            }
        }
    }
    Ok(latest)
}

/// One summary entry per run group for sidebar cards and the background panel.
///
/// A server workspace has no worktrees on this machine, so the cards it shows
/// carry assignments only. The rest is computed the same way in both modes.
#[tauri::command]
pub async fn list_task_overview(
    state: State<'_, AppState>,
) -> Result<Vec<TaskOverviewEntry>, String> {
    let tasks = state
        .client
        .list_tasks()
        .await
        .map_err(|error| error.to_string())?;
    let worktree_by_task: HashMap<String, TaskWorktree> = match state.local_client.as_deref() {
        Some(client) => client
            .list_worktrees()
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|worktree| (worktree.task_id.0.clone(), worktree))
            .collect(),
        None => HashMap::new(),
    };

    let mut groups: HashMap<String, Vec<Task>> = HashMap::new();
    for task in tasks {
        let run_id = task
            .run_group
            .as_ref()
            .map(|id| id.0.clone())
            .unwrap_or_else(|| task.id.0.clone());
        groups.entry(run_id).or_default().push(task);
    }

    let mut entries = Vec::new();
    for (run_id, group_tasks) in groups {
        let worktree = worktree_by_task.get(&run_id).cloned();
        let assignments = run_assignments(&state, &TaskId::new(run_id.clone())).await?;
        if assignments.is_empty() && worktree.is_none() {
            continue;
        }
        let Some(owner) = group_tasks.iter().find(|task| task.id.0 == run_id) else {
            continue;
        };
        if owner.archived {
            continue;
        }

        let started_at = assignments
            .iter()
            .map(|assignment| assignment.created_at.to_rfc3339())
            .chain(
                worktree
                    .iter()
                    .map(|worktree| worktree.created_at.to_rfc3339()),
            )
            .min();
        let updated_at = assignments
            .iter()
            .map(|assignment| assignment.updated_at.to_rfc3339())
            .chain(
                worktree
                    .iter()
                    .map(|worktree| worktree.updated_at.to_rfc3339()),
            )
            .max();
        let (Some(started_at), Some(updated_at)) = (started_at, updated_at) else {
            continue;
        };

        let completed = assignments
            .iter()
            .filter(|assignment| assignment.status.is_terminal())
            .count() as u32;
        let total = assignments.len() as u32;
        let latest_tool_call = latest_run_tool_call(&state, &TaskId::new(run_id.clone())).await?;
        let title = if owner.title.trim().is_empty() {
            owner.description.clone()
        } else {
            owner.title.clone()
        };
        let channel_id = owner
            .channel_id
            .as_ref()
            .map(|channel| channel.0.clone())
            .or_else(|| {
                group_tasks
                    .iter()
                    .find_map(|task| task.channel_id.as_ref().map(|channel| channel.0.clone()))
            });

        entries.push(TaskOverviewEntry {
            run_id,
            title,
            channel_id,
            status: overview_status(&assignments).to_string(),
            progress: TaskOverviewProgress { completed, total },
            assignments: assignments
                .iter()
                .map(|assignment| TaskOverviewAssignment {
                    id: assignment.id.0.clone(),
                    assignee: assignment.assignee.0.clone(),
                    status: assignment.status,
                    detail: overview_detail(assignment),
                })
                .collect(),
            worktree,
            latest_tool_call,
            started_at,
            updated_at,
        });
    }

    entries.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(entries)
}

#[tauri::command]
pub async fn cancel_task(
    state: State<'_, AppState>,
    task_id: String,
    reason: String,
) -> Result<(), String> {
    state
        .client
        .cancel_task(&TaskId::new(task_id), &reason)
        .await
        .map_err(|error| error.to_string())
}

/// Stop a running conversation, group, or task turn (ADR-0041).
///
/// Returns whether a live turn was stopped. The turn keeps the text it
/// produced, stored with a stopped marker, and reloads show that state.
#[tauri::command]
pub async fn cancel_turn(state: State<'_, AppState>, turn_id: String) -> Result<bool, String> {
    state
        .client
        .cancel_turn(&TurnId::new(turn_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn archive_task(state: State<'_, AppState>, task_id: String) -> Result<Task, String> {
    state
        .client
        .archive_task(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn unarchive_task(state: State<'_, AppState>, task_id: String) -> Result<Task, String> {
    state
        .client
        .unarchive_task(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn approve_task_plan(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Task, String> {
    state
        .client
        .approve_task_plan(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_assignments(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<Vec<Assignment>, String> {
    state
        .client
        .list_assignments(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_assignment_events(
    state: State<'_, AppState>,
    assignment_id: String,
) -> Result<Vec<AssignmentEvent>, String> {
    state
        .client
        .list_assignment_events(&AssignmentId::new(assignment_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_runs(state: State<'_, AppState>, task_id: String) -> Result<Vec<Run>, String> {
    state
        .client
        .list_runs(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_tool_calls(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<Vec<ToolCallRecord>, String> {
    state
        .client
        .list_tool_calls(&RunId::new(run_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn pending_approvals(state: State<'_, AppState>) -> Result<Vec<Approval>, String> {
    state
        .client
        .pending_approvals()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn decide_approval(
    state: State<'_, AppState>,
    approval_id: String,
    approved: bool,
    for_session: Option<bool>,
) -> Result<Approval, String> {
    let approval_id = ApprovalId::new(approval_id);
    let result = if for_session.unwrap_or(false) {
        state
            .client
            .decide_approval_for_session(&approval_id, approved)
            .await
    } else {
        state.client.decide_approval(&approval_id, approved).await
    };
    result.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn session_decisions(
    state: State<'_, AppState>,
) -> Result<Vec<cowbot_client::SessionDecisionView>, String> {
    state
        .client
        .session_decisions()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn clear_session_decisions(state: State<'_, AppState>) -> Result<(), String> {
    state
        .client
        .clear_session_decisions()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_permission_rules(
    state: State<'_, AppState>,
) -> Result<Vec<PermissionRule>, String> {
    state
        .client
        .list_permission_rules()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_permission_rule(
    state: State<'_, AppState>,
    tool: String,
    command: Option<String>,
    decision: RuleDecision,
) -> Result<PermissionRule, String> {
    state
        .client
        .create_permission_rule(&tool, command.as_deref(), decision)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_permission_rule_enabled(
    state: State<'_, AppState>,
    rule_id: String,
    enabled: bool,
) -> Result<PermissionRule, String> {
    state
        .client
        .set_permission_rule_enabled(&rule_id, enabled)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_permission_rule(
    state: State<'_, AppState>,
    rule_id: String,
) -> Result<(), String> {
    state
        .client
        .delete_permission_rule(&rule_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_workspace_policy(state: State<'_, AppState>) -> Result<WorkspacePolicy, String> {
    state
        .client
        .workspace_policy()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_bypass_critical(
    state: State<'_, AppState>,
    bypass_critical: BypassCritical,
) -> Result<WorkspacePolicy, String> {
    state
        .client
        .set_bypass_critical(bypass_critical)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_audit_events(
    state: State<'_, AppState>,
    limit: Option<u32>,
    source: Option<String>,
) -> Result<Vec<AuditEvent>, String> {
    state
        .client
        .list_audit_events(limit.unwrap_or(200).min(1_000), source.as_deref())
        .await
        .map_err(|error| error.to_string())
}

/// One provider as exposed to the frontend, without its API key.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub protocol: config::ProviderProtocol,
    pub base_url: String,
    pub models: Vec<String>,
    pub model: String,
    pub model_metadata: BTreeMap<String, config::ModelMetadata>,
    pub has_api_key: bool,
    pub active: bool,
}

/// Model provider state rendered by the settings view.
#[derive(Debug, Clone, Serialize)]
pub struct ModelSettings {
    pub providers: Vec<ProviderView>,
    pub active_provider_id: Option<String>,
    pub active_label: String,
    pub config_path: String,
}

fn load_config(state: &AppState) -> Result<config::AppConfig, String> {
    config::load(&state.config_path)
        .map_err(|error| format!("failed to read {}: {error}", state.config_path.display()))
}

/// Registered projects and their current branches for the assistant context.
fn project_platform_context(state: &AppState) -> Option<String> {
    let config = load_config(state).ok()?;
    if config.projects.is_empty() {
        return None;
    }
    let mut lines = vec![
        "Registered git projects (create_task with project and base_branch runs the task in a worktree):"
            .to_string(),
    ];
    for project in &config.projects {
        let branch = config::project_current_branch(&project.path)
            .unwrap_or_else(|| "unavailable".to_string());
        lines.push(format!(
            "- {} — current branch {} — {}",
            project.name, branch, project.path
        ));
    }
    Some(lines.join("\n"))
}

fn persist_config(
    state: &AppState,
    config: config::AppConfig,
) -> Result<config::AppConfig, String> {
    config::save(&state.config_path, &config)
        .map_err(|error| format!("failed to write {}: {error}", state.config_path.display()))?;
    state.apply_config(&config);
    Ok(config)
}

fn model_settings(state: &AppState, config: &config::AppConfig) -> ModelSettings {
    let active_id = config.active_provider().map(|provider| provider.id.clone());
    let providers = config
        .providers
        .iter()
        .map(|provider| ProviderView {
            id: provider.id.clone(),
            name: provider.name.clone(),
            protocol: provider.protocol,
            base_url: provider.base_url.clone(),
            models: provider.models.clone(),
            model: provider.model.clone(),
            model_metadata: provider.model_metadata.clone(),
            has_api_key: provider.api_key.is_some(),
            active: active_id.as_deref() == Some(provider.id.as_str()),
        })
        .collect();
    let active_label = config
        .active_provider()
        .map(|provider| provider.name.clone())
        .unwrap_or_else(|| "Offline mock model".to_string());
    ModelSettings {
        providers,
        active_provider_id: active_id,
        active_label,
        config_path: state.config_path.display().to_string(),
    }
}

#[tauri::command]
pub fn get_model_settings(state: State<'_, AppState>) -> Result<ModelSettings, String> {
    let config = load_config(&state)?;
    Ok(model_settings(&state, &config))
}

/// Context meter values for one participant of a conversation.
#[derive(Debug, Clone, Serialize)]
pub struct ConversationParticipantView {
    pub agent_id: String,
    pub agent_name: String,
    pub system_tokens: u32,
    pub tool_tokens: u32,
    pub history_tokens: u32,
    pub latest_tokens: u32,
    pub total_tokens: u32,
    /// Estimated tokens the next turn's memory block would add.
    pub memory_tokens: u32,
    pub active_entries: u32,
    pub excluded_entries: u32,
    pub percent_used: Option<f32>,
    pub state: ContextUsageState,
    pub cache_last_run: CacheUsageSummary,
    pub cache_conversation: CacheUsageSummary,
}

/// Context meter values for one conversation.
#[derive(Debug, Clone, Serialize)]
pub struct ConversationContextView {
    pub channel_id: String,
    pub agent_id: String,
    pub model: String,
    pub provider_id: Option<String>,
    /// Provider display name, when one supplies the model.
    pub provider_name: Option<String>,
    /// Whether the conversation follows the desktop's default model.
    pub model_follows_default: bool,
    pub system_tokens: u32,
    pub tool_tokens: u32,
    pub history_tokens: u32,
    pub latest_tokens: u32,
    pub total_tokens: u32,
    pub active_entries: u32,
    pub excluded_entries: u32,
    /// Where the token counts came from: an exact model tokenizer or the
    /// character estimator.
    pub token_source: TokenSource,
    /// Label of the counter, for example `o200k_base`.
    pub token_counter: Option<String>,
    /// Estimated tokens the next turn's memory block would add.
    pub memory_tokens: u32,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub usable_tokens: Option<u32>,
    pub percent_used: Option<f32>,
    pub state: ContextUsageState,
    pub auto_compact: bool,
    pub compact_threshold: u32,
    pub router_enabled: bool,
    /// How an unmentioned group message picks its responder (ADR-0042).
    pub routing_mode: String,
    /// Participants kept in the roster but never selected.
    pub muted_participants: Vec<String>,
    /// Whether group replies are stripped of speaker labels and cross-talk.
    pub reply_cleanup: bool,
    pub default_responder: Option<String>,
    pub cache_last_turn: CacheUsageSummary,
    pub cache_conversation: CacheUsageSummary,
    /// One entry per participant; the top-level fields mirror the first.
    pub participants: Vec<ConversationParticipantView>,
}

/// Provider and model a conversation's next turn will use.
///
/// A conversation that follows the default resolves through the active
/// provider, so it picks up a changed default. A pinned conversation keeps the
/// pair recorded at creation while that provider still exists; once the
/// provider is gone it falls back to the active default instead of sending a
/// model name the new endpoint may not serve (ADR-0049).
fn effective_conversation_model(
    state: &AppState,
    settings: &cowbot_core::conversation::ConversationSettings,
) -> (Option<String>, String) {
    if !settings.model_follows_default {
        if let Some(provider_id) = settings.provider_id.as_deref() {
            let provider_exists = load_config(state)
                .ok()
                .and_then(|config| config.provider(provider_id).map(|_| ()))
                .is_some();
            if provider_exists {
                return (Some(provider_id.to_string()), settings.model.clone());
            }
        }
    }
    let (_, model) = state.runtime_model();
    let provider_id = load_config(state)
        .ok()
        .and_then(|config| config.active_provider().map(|provider| provider.id.clone()));
    (provider_id, model)
}

/// Provider/model of a conversation, falling back to the active runtime when
/// the conversation follows the default or its provider was removed.
fn conversation_runtime(
    state: &AppState,
    settings: &cowbot_core::conversation::ConversationSettings,
) -> (Arc<dyn LlmProvider>, String) {
    let (provider_id, model) = effective_conversation_model(state, settings);
    if let Some(provider_id) = provider_id.as_deref() {
        if let Ok(config) = load_config(state) {
            if let Some(provider) = config.provider(provider_id) {
                return (
                    config::build_remote_provider(
                        provider.protocol,
                        &provider.base_url,
                        provider.api_key.clone(),
                    ),
                    model,
                );
            }
        }
    }
    let (llm, _) = state.runtime_model();
    (llm, model)
}

/// Context window and output limit metadata recorded for the conversation model.
fn conversation_window(
    state: &AppState,
    settings: &cowbot_core::conversation::ConversationSettings,
) -> (Option<u32>, Option<u32>) {
    let Ok(config) = load_config(state) else {
        return (None, None);
    };
    let (provider_id, model) = effective_conversation_model(state, settings);
    let provider = provider_id
        .as_deref()
        .and_then(|id| config.provider(id))
        .or_else(|| config.active_provider());
    provider
        .and_then(|provider| provider.model_metadata.get(&model))
        .map(|metadata| (metadata.context_window, metadata.max_output_tokens))
        .unwrap_or((None, None))
}

/// Window metadata for a model the run recorded, from any configured provider.
///
/// A task run records its model with its LLM calls but not the provider that
/// served it, so the active provider is preferred and the rest are searched.
fn model_window(state: &AppState, model: &str) -> (Option<u32>, Option<u32>) {
    let Ok(config) = load_config(state) else {
        return (None, None);
    };
    config
        .active_provider()
        .and_then(|provider| provider.model_metadata.get(model))
        .or_else(|| {
            config
                .providers
                .iter()
                .find_map(|provider| provider.model_metadata.get(model))
        })
        .map(|metadata| (metadata.context_window, metadata.max_output_tokens))
        .unwrap_or((None, None))
}

async fn conversation_context_view(
    state: &AppState,
    channel_id: &ChannelId,
) -> Result<ConversationContextView, String> {
    let context: ConversationContext = state
        .client
        .conversation_context(channel_id)
        .await
        .map_err(|error| error.to_string())?;
    let settings = state
        .client
        .conversation_settings(channel_id)
        .await
        .map_err(|error| error.to_string())?;
    let agents = state
        .client
        .list_agents()
        .await
        .map_err(|error| error.to_string())?;
    let tool_tokens = participant_tool_tokens(state, &context, &agents);
    let (context_window, max_output_tokens) = conversation_window(state, &settings);
    let mut view = context_view_from_parts(
        context,
        &settings,
        &tool_tokens,
        context_window,
        max_output_tokens,
    );
    // Report the pair the next turn will actually use, not a stale snapshot.
    let (provider_id, model) = effective_conversation_model(state, &settings);
    view.model = model;
    view.provider_id = provider_id.clone();
    view.provider_name = provider_id.and_then(|id| {
        load_config(state)
            .ok()
            .and_then(|config| config.provider(&id).map(|provider| provider.name.clone()))
    });
    Ok(view)
}

/// Tool-schema tokens each conversation participant pays in every request.
///
/// Counted with the conversation model's tokenizer, so the meter's tool
/// segment matches the history and system counts beside it (ADR-0035).
fn participant_tool_tokens(
    state: &AppState,
    context: &ConversationContext,
    agents: &[AgentProfile],
) -> HashMap<AgentId, u32> {
    let counter = token_counter_for_model(&context.model);
    let participant_ids: Vec<AgentId> = if context.participants.is_empty() {
        vec![context.agent_id.clone()]
    } else {
        context
            .participants
            .iter()
            .map(|participant| participant.agent_id.clone())
            .collect()
    };
    let mut tool_tokens = HashMap::new();
    for agent_id in participant_ids {
        if let Some(agent) = agents.iter().find(|agent| agent.id == agent_id) {
            let tokens = serde_json::to_string(&state.tools.definitions_for(&agent.tools))
                .map(|serialized| counter.count(&serialized))
                .unwrap_or(0);
            tool_tokens.insert(agent_id, tokens);
        }
    }
    tool_tokens
}

fn context_view_from_parts(
    context: ConversationContext,
    settings: &cowbot_core::conversation::ConversationSettings,
    tool_tokens: &HashMap<AgentId, u32>,
    context_window: Option<u32>,
    max_output_tokens: Option<u32>,
) -> ConversationContextView {
    let usable_tokens = usable_window(context_window, max_output_tokens);
    let mut participants = Vec::new();
    if context.participants.is_empty() {
        participants.push(participant_view(
            context.agent_id.0.clone(),
            String::new(),
            &context.projection,
            0,
            tool_tokens.get(&context.agent_id).copied().unwrap_or(0),
            context.cache_last_turn.clone(),
            context.cache_conversation.clone(),
            usable_tokens,
        ));
    } else {
        for participant in &context.participants {
            participants.push(participant_view(
                participant.agent_id.0.clone(),
                participant.agent_name.clone(),
                &participant.projection,
                participant.memory_tokens,
                tool_tokens.get(&participant.agent_id).copied().unwrap_or(0),
                participant.cache_last_run.clone(),
                participant.cache_conversation.clone(),
                usable_tokens,
            ));
        }
    }

    // The top-level fields mirror the first participant for older callers.
    let first = participants
        .first()
        .cloned()
        .expect("one participant view is always built");
    ConversationContextView {
        channel_id: context.channel_id.0,
        agent_id: context.agent_id.0,
        model: context.model,
        provider_id: context.provider_id,
        provider_name: None,
        model_follows_default: settings.model_follows_default,
        system_tokens: first.system_tokens,
        tool_tokens: first.tool_tokens,
        history_tokens: first.history_tokens,
        latest_tokens: first.latest_tokens,
        total_tokens: first.total_tokens,
        active_entries: first.active_entries,
        excluded_entries: first.excluded_entries,
        token_source: context.projection.token_source,
        token_counter: context.projection.token_counter.clone(),
        memory_tokens: first.memory_tokens,
        context_window,
        max_output_tokens,
        usable_tokens,
        percent_used: first.percent_used,
        state: first.state,
        auto_compact: settings.auto_compact,
        compact_threshold: settings.compact_threshold,
        router_enabled: settings.router_enabled,
        routing_mode: settings.routing_mode.as_str().to_string(),
        muted_participants: settings
            .muted_participants
            .iter()
            .map(|agent| agent.0.clone())
            .collect(),
        reply_cleanup: settings.reply_cleanup,
        default_responder: settings
            .default_responder
            .as_ref()
            .map(|agent| agent.0.clone()),
        cache_last_turn: first.cache_last_run,
        cache_conversation: first.cache_conversation,
        participants,
    }
}

#[allow(clippy::too_many_arguments)]
fn participant_view(
    agent_id: String,
    agent_name: String,
    projection: &ContextProjection,
    memory_tokens: u32,
    tool_tokens: u32,
    cache_last_run: CacheUsageSummary,
    cache_conversation: CacheUsageSummary,
    usable_tokens: Option<u32>,
) -> ConversationParticipantView {
    let total_tokens = projection
        .total_tokens()
        .saturating_add(tool_tokens)
        .saturating_add(memory_tokens);
    let (percent_used, state) = context_usage_state(total_tokens, usable_tokens);
    ConversationParticipantView {
        agent_id,
        agent_name,
        system_tokens: projection.system_tokens,
        tool_tokens,
        history_tokens: projection.history_tokens,
        latest_tokens: projection.latest_tokens,
        total_tokens,
        memory_tokens,
        active_entries: projection.active_entries,
        excluded_entries: projection.excluded_entries,
        percent_used,
        state,
        cache_last_run,
        cache_conversation,
    }
}

/// A read-only preview of the request one turn or task run would send.
#[derive(Debug, Clone, Serialize)]
pub struct ContextPreviewView {
    /// `conversation`, `group`, or `task_run`.
    pub kind: PromptKind,
    pub channel_id: Option<String>,
    pub run_id: Option<String>,
    pub task_id: Option<String>,
    pub agent_id: String,
    pub agent_name: String,
    /// Conversation name or task title.
    pub title: String,
    pub plan: PromptPlan,
    /// Total the request would send, including the memory message.
    pub request_tokens: u32,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub usable_tokens: Option<u32>,
    pub percent_used: Option<f32>,
    pub state: ContextUsageState,
    /// True when the request would not fit the usable window.
    pub over_budget: bool,
}

impl ContextPreviewView {
    /// Attach window metadata and the over-budget state to a plan.
    #[allow(clippy::too_many_arguments)]
    fn from_plan(
        kind: PromptKind,
        channel_id: Option<String>,
        run_id: Option<String>,
        task_id: Option<String>,
        agent_id: String,
        agent_name: String,
        title: String,
        plan: PromptPlan,
        context_window: Option<u32>,
        max_output_tokens: Option<u32>,
    ) -> Self {
        let request_tokens = plan.request_tokens();
        let usable_tokens = usable_window(context_window, max_output_tokens);
        let (percent_used, state) = context_usage_state(request_tokens, usable_tokens);
        Self {
            kind,
            channel_id,
            run_id,
            task_id,
            agent_id,
            agent_name,
            title,
            plan,
            request_tokens,
            context_window,
            max_output_tokens,
            usable_tokens,
            percent_used,
            state,
            over_budget: usable_tokens.is_some_and(|usable| request_tokens >= usable),
        }
    }
}

/// Rebuild one conversation participant's next request without writing.
#[tauri::command]
pub async fn get_conversation_context_preview(
    state: State<'_, AppState>,
    channel_id: String,
    agent_id: String,
    draft: Option<String>,
    plan_mode: Option<bool>,
) -> Result<ContextPreviewView, String> {
    let channel_id = ChannelId::new(channel_id);
    let agent_id = AgentId::new(agent_id);
    let settings = state
        .client
        .conversation_settings(&channel_id)
        .await
        .map_err(|error| error.to_string())?;
    let agents = state
        .client
        .list_agents()
        .await
        .map_err(|error| error.to_string())?;
    let agent = agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("agent '{}' was not found", agent_id.0))?;
    let tools = state.tools.definitions_for(&agent.tools);
    let platform_context = project_platform_context(&state);
    let preview = state
        .client
        .conversation_context_preview(
            &channel_id,
            &agent_id,
            draft.as_deref(),
            platform_context.as_deref(),
            plan_mode.unwrap_or(false),
            &tools,
        )
        .await
        .map_err(|error| error.to_string())?;
    let title = state
        .client
        .get_channel(&channel_id)
        .await
        .map_err(|error| error.to_string())?
        .map(|channel| channel.name)
        .unwrap_or_default();
    let (context_window, max_output_tokens) = conversation_window(&state, &settings);
    Ok(ContextPreviewView::from_plan(
        preview.plan.kind,
        Some(preview.channel_id.0),
        None,
        None,
        preview.agent_id.0,
        preview.agent_name,
        title,
        preview.plan,
        context_window,
        max_output_tokens,
    ))
}

/// Rebuild the first request one task run sent without writing.
#[tauri::command]
pub async fn get_task_run_context_preview(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<ContextPreviewView, String> {
    let run_id = RunId::new(run_id);
    let (_, fallback_model) = state.runtime_model();
    let run = state
        .client
        .get_run(&run_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("run '{}' was not found", run_id.0))?;
    let agents = state
        .client
        .list_agents()
        .await
        .map_err(|error| error.to_string())?;
    // Standard crew roles are transient; the run's snapshot is the profile
    // that actually executed it.
    let profile = run
        .agent_profile
        .clone()
        .or_else(|| {
            agents
                .iter()
                .find(|agent| agent.id == run.agent_id)
                .cloned()
        })
        .ok_or_else(|| format!("agent '{}' was not found", run.agent_id.0))?;
    let tools = state.tools.definitions_for(&profile.tools);
    let preview = state
        .client
        .task_run_context_preview(&run_id, &fallback_model, &tools)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("run '{}' was not found", run_id.0))?;
    let (context_window, max_output_tokens) = model_window(&state, &preview.plan.model);
    Ok(ContextPreviewView::from_plan(
        preview.plan.kind,
        None,
        Some(preview.run_id.0),
        Some(preview.task_id.0),
        preview.agent_id.0,
        preview.agent_name,
        preview.task_title,
        preview.plan,
        context_window,
        max_output_tokens,
    ))
}

/// The redacted prompt snapshots recorded for a run, oldest first.
#[tauri::command]
pub async fn list_run_prompt_snapshots(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<Vec<PromptSnapshotSummary>, String> {
    state
        .client
        .list_run_prompt_snapshots(&RunId::new(run_id))
        .await
        .map_err(|error| error.to_string())
}

/// The newest prompt snapshots in the workspace, for the audit view.
#[tauri::command]
pub async fn list_recent_prompt_snapshots(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<PromptSnapshotSummary>, String> {
    state
        .client
        .list_recent_prompt_snapshots(limit.unwrap_or(100))
        .await
        .map_err(|error| error.to_string())
}

/// One recorded prompt snapshot, with the call links it belongs to.
#[tauri::command]
pub async fn get_llm_prompt_snapshot(
    state: State<'_, AppState>,
    call_id: String,
) -> Result<Option<PromptSnapshotDetail>, String> {
    state
        .client
        .prompt_snapshot(&call_id)
        .await
        .map_err(|error| error.to_string())
}

/// The workspace prompt-snapshot settings.
#[tauri::command]
pub async fn get_prompt_snapshot_settings(
    state: State<'_, AppState>,
) -> Result<PromptSnapshotSettings, String> {
    state
        .client
        .prompt_snapshot_settings()
        .await
        .map_err(|error| error.to_string())
}

/// Change whether snapshots are stored and how long they are kept.
#[tauri::command]
pub async fn update_prompt_snapshot_settings(
    state: State<'_, AppState>,
    enabled: Option<bool>,
    retention_days: Option<u32>,
) -> Result<PromptSnapshotSettings, String> {
    state
        .client
        .update_prompt_snapshot_settings(enabled, retention_days)
        .await
        .map_err(|error| error.to_string())
}

/// Delete expired snapshots now and report how many went.
#[tauri::command]
pub async fn purge_prompt_snapshots(state: State<'_, AppState>) -> Result<u64, String> {
    state
        .client
        .purge_prompt_snapshots()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn create_conversation(
    state: State<'_, AppState>,
    title: Option<String>,
    agent_id: Option<String>,
    agent_ids: Option<Vec<String>>,
    router_enabled: Option<bool>,
    default_responder: Option<String>,
) -> Result<Channel, String> {
    let config = load_config(&state)?;
    let (_, model) = state.runtime_model();
    let provider_id = config.active_provider().map(|provider| provider.id.clone());
    let mut participants = agent_ids
        .unwrap_or_default()
        .into_iter()
        .map(AgentId::new)
        .collect::<Vec<_>>();
    if participants.is_empty() {
        if let Some(agent_id) = agent_id {
            participants.push(AgentId::new(agent_id));
        }
    }
    state
        .client
        .create_conversation(&NewConversation {
            title,
            agent_ids: participants,
            provider_id,
            model,
            router_enabled: router_enabled.unwrap_or(false),
            default_responder: default_responder.map(AgentId::new),
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_conversation_summaries(
    state: State<'_, AppState>,
) -> Result<Vec<ConversationSummary>, String> {
    state
        .client
        .list_conversation_summaries()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_conversation_participants(
    state: State<'_, AppState>,
    channel_id: String,
) -> Result<Vec<ChannelParticipant>, String> {
    state
        .client
        .list_conversation_participants(&ChannelId::new(channel_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_conversation_turns(
    state: State<'_, AppState>,
    channel_id: String,
    limit: Option<u32>,
) -> Result<Vec<ConversationTurn>, String> {
    state
        .client
        .list_conversation_turns(&ChannelId::new(channel_id), limit.unwrap_or(50))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_conversation_context(
    state: State<'_, AppState>,
    channel_id: String,
) -> Result<ConversationContextView, String> {
    conversation_context_view(&state, &ChannelId::new(channel_id)).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_conversation_settings(
    state: State<'_, AppState>,
    channel_id: String,
    auto_compact: Option<bool>,
    compact_threshold: Option<u32>,
    router_enabled: Option<bool>,
    default_responder: Option<String>,
    routing_mode: Option<String>,
    muted_participants: Option<Vec<String>>,
    reply_cleanup: Option<bool>,
    provider_id: Option<String>,
    model: Option<String>,
    model_follows_default: Option<bool>,
) -> Result<ConversationContextView, String> {
    let channel_id = ChannelId::new(channel_id);
    let (mut provider_id, mut model) = (provider_id, model);
    if model_follows_default == Some(true) {
        // Following always carries the default's pair, so no reader sees the
        // flag without the snapshot it refers to (ADR-0049).
        if model.as_deref().map(str::trim).unwrap_or("").is_empty() {
            model = Some(state.runtime_model().1);
        }
        if provider_id.is_none() {
            provider_id = load_config(&state)
                .ok()
                .and_then(|config| config.active_provider().map(|provider| provider.id.clone()));
        }
    }
    state
        .client
        .update_conversation_settings(
            &channel_id,
            &ConversationSettingsUpdate {
                auto_compact,
                compact_threshold,
                router_enabled,
                default_responder: default_responder.map(AgentId::new),
                routing_mode: routing_mode
                    .as_deref()
                    .map(str::parse::<RoutingMode>)
                    .transpose()
                    .map_err(|_| format!("unknown routing mode '{:?}'", routing_mode))?,
                muted_participants: muted_participants
                    .map(|ids| ids.into_iter().map(AgentId::new).collect()),
                reply_cleanup,
                provider_id,
                model,
                model_follows_default,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    conversation_context_view(&state, &channel_id).await
}

#[tauri::command]
pub async fn compact_conversation(
    state: State<'_, AppState>,
    channel_id: String,
    agent_id: Option<String>,
) -> Result<CompactionOutcome, String> {
    let channel_id = ChannelId::new(channel_id);
    let settings = state
        .client
        .conversation_settings(&channel_id)
        .await
        .map_err(|error| error.to_string())?;
    let (llm, model) = conversation_runtime(&state, &settings);
    match agent_id {
        Some(agent_id) => local(&state)?
            .compact_conversation_agent(&channel_id, &AgentId::new(agent_id), llm, &model)
            .await
            .map_err(|error| error.to_string()),
        None => local(&state)?
            .compact_conversation(&channel_id, llm, &model)
            .await
            .map_err(|error| error.to_string()),
    }
}

#[tauri::command]
pub async fn clear_conversation_context(
    state: State<'_, AppState>,
    channel_id: String,
    agent_id: Option<String>,
) -> Result<u64, String> {
    let channel_id = ChannelId::new(channel_id);
    match agent_id {
        Some(agent_id) => state
            .client
            .clear_conversation_participant_context(&channel_id, &AgentId::new(agent_id))
            .await
            .map_err(|error| error.to_string()),
        None => state
            .client
            .clear_conversation_context(&channel_id)
            .await
            .map_err(|error| error.to_string()),
    }
}

#[tauri::command]
pub async fn get_memory_notes(
    state: State<'_, AppState>,
    channel_id: Option<String>,
) -> Result<MemoryNotes, String> {
    let channel = channel_id.map(ChannelId::new);
    state
        .client
        .memory_notes(channel.as_ref())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn save_memory_note(
    state: State<'_, AppState>,
    channel_id: Option<String>,
    content: String,
) -> Result<NoteSave, String> {
    let saved = match channel_id {
        Some(channel_id) => {
            state
                .client
                .save_conversation_note(&ChannelId::new(channel_id), &content)
                .await
        }
        None => state.client.save_workspace_note(&content).await,
    };
    saved.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_memory_note(
    state: State<'_, AppState>,
    channel_id: String,
) -> Result<u64, String> {
    state
        .client
        .delete_conversation_note(&ChannelId::new(channel_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_memory_settings(state: State<'_, AppState>) -> Result<MemorySettings, String> {
    state
        .client
        .memory_settings()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_memory_settings(
    state: State<'_, AppState>,
    extraction_enabled: Option<bool>,
    recall_enabled: Option<bool>,
    notes_budget_tokens: Option<u32>,
    recall_budget_tokens: Option<u32>,
) -> Result<MemorySettings, String> {
    state
        .client
        .update_memory_settings(&MemorySettingsUpdate {
            extraction_enabled,
            recall_enabled,
            notes_budget_tokens,
            recall_budget_tokens,
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_memory_entries(
    state: State<'_, AppState>,
    status: Option<MemoryStatus>,
    agent_id: Option<String>,
    channel_id: Option<String>,
) -> Result<Vec<MemoryEntry>, String> {
    state
        .client
        .list_memory_entries(&MemoryFilter {
            status,
            agent_id: agent_id.map(AgentId::new),
            channel_id: channel_id.map(ChannelId::new),
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn add_memory_entry(
    state: State<'_, AppState>,
    content: String,
    kind: MemoryKind,
    agent_id: Option<String>,
    channel_id: Option<String>,
    pinned: Option<bool>,
) -> Result<MemorySave, String> {
    let new = NewMemory::user(content)
        .with_kind(kind)
        .with_scope(agent_id.map(AgentId::new), channel_id.map(ChannelId::new))
        .with_pinned(pinned.unwrap_or(false));
    state
        .client
        .add_memory_entry(new)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_memory_entry(
    state: State<'_, AppState>,
    entry_id: String,
    content: Option<String>,
    kind: Option<MemoryKind>,
    pinned: Option<bool>,
) -> Result<MemorySave, String> {
    state
        .client
        .update_memory_entry(
            &entry_id,
            &MemoryUpdate {
                content,
                kind,
                pinned,
            },
        )
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn approve_memory_entry(
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<MemoryEntry, String> {
    state
        .client
        .approve_memory_entry(&entry_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn dismiss_memory_entry(
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<MemoryEntry, String> {
    state
        .client
        .dismiss_memory_entry(&entry_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn archive_memory_entry(
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<MemoryEntry, String> {
    state
        .client
        .archive_memory_entry(&entry_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_memory_entry(
    state: State<'_, AppState>,
    entry_id: String,
) -> Result<u64, String> {
    state
        .client
        .delete_memory_entry(&entry_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn preview_memory_recall(
    state: State<'_, AppState>,
    channel_id: Option<String>,
    agent_id: String,
    query: String,
) -> Result<Option<RecalledMemory>, String> {
    let channel = channel_id.map(ChannelId::new);
    state
        .client
        .preview_memory_recall(channel.as_ref(), &AgentId::new(agent_id), &query)
        .await
        .map_err(|error| error.to_string())
}

/// Values submitted by the provider editor.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDraft {
    #[serde(default)]
    pub provider_id: Option<String>,
    pub protocol: config::ProviderProtocol,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    pub model: String,
    #[serde(default)]
    pub model_metadata: BTreeMap<String, config::ModelMetadata>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
}

/// Mirror a new default provider/model into every conversation that follows it.
///
/// The stored snapshot is what meters, compaction, and previews count with, so
/// a following conversation is rewritten in the same command that changes the
/// default (ADR-0049).
async fn sync_following_conversations(
    state: &AppState,
    provider_id: Option<String>,
    model: &str,
) -> Result<(), String> {
    let summaries = state
        .client
        .list_conversation_summaries()
        .await
        .map_err(|error| error.to_string())?;
    let provider_id = provider_id.unwrap_or_default();
    for summary in summaries {
        if !summary.settings.model_follows_default {
            continue;
        }
        let update = ConversationSettingsUpdate {
            provider_id: Some(provider_id.clone()),
            model: Some(model.to_string()),
            ..Default::default()
        };
        state
            .client
            .update_conversation_settings(&summary.channel.id, &update)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Apply a create or update to an in-memory configuration.
///
/// A blank API key keeps the stored one; `clear_api_key` removes it. The first
/// provider added to an empty configuration becomes active. Returns the id of
/// the provider that was written.
fn upsert_provider(
    config: &mut config::AppConfig,
    draft: &ProviderDraft,
) -> Result<String, String> {
    let typed_key = config::normalize_api_key(draft.api_key.as_deref());

    match draft.provider_id.as_deref() {
        Some(id) => {
            let index = config
                .providers
                .iter()
                .position(|provider| provider.id == id)
                .ok_or_else(|| format!("provider '{id}' was not found"))?;
            let key = if draft.clear_api_key {
                None
            } else {
                typed_key.or_else(|| config.providers[index].api_key.clone())
            };
            config.providers[index] = config::make_provider(
                id.to_string(),
                &draft.name,
                draft.protocol,
                &draft.base_url,
                &draft.models,
                &draft.model,
                key.as_deref(),
                draft.model_metadata.clone(),
            )?;
            Ok(id.to_string())
        }
        None => {
            let id = config::new_provider_id(&config.providers);
            config.providers.push(config::make_provider(
                id.clone(),
                &draft.name,
                draft.protocol,
                &draft.base_url,
                &draft.models,
                &draft.model,
                typed_key.as_deref(),
                draft.model_metadata.clone(),
            )?);
            if config.active_provider().is_none() {
                config.active_provider_id = Some(id.clone());
            }
            Ok(id)
        }
    }
}

/// Create a provider when `provider_id` is absent, otherwise update it.
#[tauri::command]
pub async fn save_model_provider(
    state: State<'_, AppState>,
    draft: ProviderDraft,
) -> Result<ModelSettings, String> {
    let mut config = load_config(&state)?;
    let provider_id = upsert_provider(&mut config, &draft)?;

    let config = persist_config(&state, config)?;

    // Saving the active provider can change the model runs use; keep the
    // snapshot of conversations that follow the default on that model.
    if config.active_provider_id.as_deref() == Some(provider_id.as_str()) {
        if let Some(provider) = config.provider(&provider_id) {
            let model = provider.model.clone();
            sync_following_conversations(&state, Some(provider_id), &model).await?;
        }
    }
    Ok(model_settings(&state, &config))
}

/// Select one of a provider's known models without resaving the provider.
///
/// Selecting a model also makes its provider the active one, so one action
/// answers "which model do my agents run" (ADR-0048).
#[tauri::command]
pub async fn set_active_model(
    state: State<'_, AppState>,
    provider_id: String,
    model: String,
) -> Result<ModelSettings, String> {
    let mut config = load_config(&state)?;
    let index = config
        .providers
        .iter()
        .position(|provider| provider.id == provider_id)
        .ok_or_else(|| format!("provider '{provider_id}' was not found"))?;
    let provider = &config.providers[index];
    if !provider.models.iter().any(|known| known == &model) {
        return Err(format!(
            "model '{model}' is not known for provider '{}'",
            provider.name
        ));
    }
    config.providers[index].model = model.clone();
    config.active_provider_id = Some(provider_id.clone());

    let config = persist_config(&state, config)?;
    sync_following_conversations(&state, Some(provider_id), &model).await?;
    Ok(model_settings(&state, &config))
}

#[tauri::command]
pub async fn delete_model_provider(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<ModelSettings, String> {
    let mut config = load_config(&state)?;
    let before = config.providers.len();
    config
        .providers
        .retain(|provider| provider.id != provider_id);
    if config.providers.len() == before {
        return Err(format!("provider '{provider_id}' was not found"));
    }
    if config.active_provider_id.as_deref() == Some(provider_id.as_str()) {
        config.active_provider_id = None;
    }

    let config = persist_config(&state, config)?;

    // A conversation pinned to the removed provider would otherwise keep a
    // stale model name; move it onto the new default. Conversations that
    // follow the default take the new snapshot too (ADR-0049).
    let (_, model) = state.runtime_model();
    let active_id = config
        .active_provider()
        .map(|provider| provider.id.clone())
        .unwrap_or_default();
    let summaries = state
        .client
        .list_conversation_summaries()
        .await
        .map_err(|error| error.to_string())?;
    for summary in summaries {
        let pinned_here = summary.settings.provider_id.as_deref() == Some(provider_id.as_str());
        if !summary.settings.model_follows_default && !pinned_here {
            continue;
        }
        let update = ConversationSettingsUpdate {
            provider_id: Some(active_id.clone()),
            model: Some(model.clone()),
            model_follows_default: pinned_here.then_some(true),
            ..Default::default()
        };
        state
            .client
            .update_conversation_settings(&summary.channel.id, &update)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(model_settings(&state, &config))
}

/// Select the provider used for crew runs; `None` selects the offline mock.
#[tauri::command]
pub async fn activate_model_provider(
    state: State<'_, AppState>,
    provider_id: Option<String>,
) -> Result<ModelSettings, String> {
    let mut config = load_config(&state)?;
    match provider_id.as_deref() {
        Some(id) => {
            if config.providers.iter().all(|provider| provider.id != id) {
                return Err(format!("provider '{id}' was not found"));
            }
            config.active_provider_id = Some(id.to_string());
        }
        None => config.active_provider_id = None,
    }

    let config = persist_config(&state, config)?;
    let snapshot = config
        .active_provider()
        .map(|provider| (provider.id.clone(), provider.model.clone()));
    let (provider_id, model) = match snapshot {
        Some((provider_id, model)) => (Some(provider_id), model),
        None => (None, state.runtime_model().1),
    };
    sync_following_conversations(&state, provider_id, &model).await?;
    Ok(model_settings(&state, &config))
}

/// First-run onboarding state for the desktop.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingState {
    pub completed: bool,
    /// Absolute path of the workspace database the app opened.
    pub database: String,
    /// Folder that holds workspace databases.
    pub database_dir: String,
}

fn onboarding_state(config: &config::AppConfig, state: &AppState) -> OnboardingState {
    OnboardingState {
        completed: config.onboarding_completed,
        database: state.database_path.display().to_string(),
        database_dir: state.database_dir.display().to_string(),
    }
}

/// What the frontend needs to decide before the workspace opens.
#[tauri::command]
pub fn get_startup_status(state: State<'_, startup::StartupState>) -> startup::StartupStatus {
    state.status()
}

/// The user's answer to a startup question.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum StartupAction {
    /// Apply the pending migrations directly.
    Upgrade,
    /// Copy the database, then apply the pending migrations.
    UpgradeWithBackup,
    /// Replace the database with one of the reported backups.
    Restore { backup: String },
    /// Move the database aside and open an empty workspace.
    Fresh,
    /// Try the saved server again after a connection failure.
    Retry,
    /// Switch this device back to its local workspace.
    GoLocal,
}

/// Apply a startup decision and open the workspace when it becomes possible.
#[tauri::command]
pub async fn resolve_startup(
    app: AppHandle,
    state: State<'_, startup::StartupState>,
    action: StartupAction,
) -> Result<startup::StartupStatus, String> {
    if app.try_state::<AppState>().is_some() {
        return Ok(startup::StartupStatus::Ready);
    }

    match action {
        StartupAction::Upgrade => {
            return open_workspace(&app, &state).await;
        }
        StartupAction::UpgradeWithBackup => {
            let startup::StartupStatus::Upgrade { backup, .. } = state.status() else {
                return Err("This workspace does not need an upgrade.".to_string());
            };
            startup::backup_database(&state.db_path, Path::new(&backup))
                .map_err(|error| format!("failed to back up the database: {error}"))?;

            // Opening the database applies the migrations; inspecting first
            // would still report the old schema version.
            return open_workspace(&app, &state).await;
        }
        StartupAction::Restore { backup } => {
            startup::restore_from(Path::new(&backup), &state.db_path, &state.database_dir).await?;
        }
        StartupAction::Fresh => {
            startup::start_fresh(&state.db_path)?;
        }
        // A server that could not be opened is retried as it stands, or the
        // device is pointed back at its local workspace; there is no local
        // database to upgrade or replace in that state.
        StartupAction::Retry => {
            return open_workspace(&app, &state).await;
        }
        StartupAction::GoLocal => {
            let mut config = config::load(&state.config_path)
                .map_err(|error| format!("failed to read the configuration: {error}"))?;
            config.backend = config::BackendMode::Local;
            config::save(&state.config_path, &config)
                .map_err(|error| format!("failed to write the configuration: {error}"))?;
            // `finish` reads the configuration again, so opening now lands in
            // the local workspace instead of the unreachable server.
            return open_workspace(&app, &state).await;
        }
    }

    let next = startup::inspect(&state.db_path, &state.database_dir).await;
    state.set_status(next.clone());
    if next.is_ready() {
        return open_workspace(&app, &state).await;
    }
    Ok(next)
}

/// Finish startup and report what the window should show next.
async fn open_workspace(
    app: &AppHandle,
    state: &startup::StartupState,
) -> Result<startup::StartupStatus, String> {
    match crate::finish(app).await {
        Ok(()) => {
            state.set_status(startup::StartupStatus::Ready);
            Ok(startup::StartupStatus::Ready)
        }
        Err(error) => {
            crate::mark_failed(app, &error);
            Ok(state.status())
        }
    }
}

/// Close the app from the startup screen.
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// Set the tray menu's labels to the interface language.
///
/// The shell cannot read the interface locale, so the window reports it. Only
/// the text changes: a tray menu cannot be replaced on Linux once it is set.
#[tauri::command]
pub fn set_tray_labels(app: AppHandle, open: String, quit: String) -> Result<(), String> {
    let Some(handles) = app.try_state::<crate::shell::TrayHandles>() else {
        // A tray that failed to build is not an error the window can act on.
        return Ok(());
    };
    handles
        .open
        .set_text(open)
        .and_then(|()| handles.quit.set_text(quit))
        .map_err(|error| error.to_string())
}

/// Whether closing the window hides CowBot instead of quitting.
#[tauri::command]
pub fn get_close_to_tray(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(load_config(&state)?.close_to_tray)
}

/// Persist whether closing the window hides CowBot instead of quitting.
#[tauri::command]
pub fn set_close_to_tray(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    crate::shell::set_close_to_tray(&state, enabled)
}

/// Whether a settled run or a waiting approval raises a system notification.
#[tauri::command]
pub fn get_notifications_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(load_config(&state)?.notifications_enabled)
}

/// Persist whether CowBot raises system notifications while it is hidden.
#[tauri::command]
pub fn set_notifications_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut config = load_config(&state)?;
    config.notifications_enabled = enabled;
    config::save(&state.config_path, &config)
        .map_err(|error| format!("failed to write {}: {error}", state.config_path.display()))
}

/// Point the app at another folder for workspace databases and restart.
#[tauri::command]
pub fn set_database_directory(
    app: AppHandle,
    state: State<'_, startup::StartupState>,
    path: String,
) -> Result<(), String> {
    let folder = PathBuf::from(path.trim());
    if !folder.is_absolute() {
        return Err("Choose an absolute folder.".to_string());
    }
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("failed to create {}: {error}", folder.display()))?;
    if !folder.is_dir() {
        return Err(format!("{} is not a folder.", folder.display()));
    }

    // Fail here rather than at the next launch when the folder is read-only.
    let probe = folder.join(".cowbot-write-test");
    std::fs::write(&probe, b"")
        .map_err(|error| format!("{} is not writable: {error}", folder.display()))?;
    let _ = std::fs::remove_file(&probe);

    let mut config = config::load(&state.config_path).unwrap_or_default();
    config.database_dir = Some(folder.to_string_lossy().into_owned());
    config.active_workspace_database = None;
    config::save(&state.config_path, &config)
        .map_err(|error| format!("failed to write {}: {error}", state.config_path.display()))?;
    app.restart();
}

#[tauri::command]
pub fn get_onboarding_state(state: State<'_, AppState>) -> Result<OnboardingState, String> {
    let config = load_config(&state)?;
    Ok(onboarding_state(&config, &state))
}

/// Mark the guided first-run setup as completed or skipped.
#[tauri::command]
pub fn complete_onboarding(state: State<'_, AppState>) -> Result<OnboardingState, String> {
    let mut config = load_config(&state)?;
    config.onboarding_completed = true;
    let config = persist_config(&state, config)?;
    Ok(onboarding_state(&config, &state))
}

/// Build version, update-check preference, and backend shown in Settings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub auto_check_updates: bool,
    /// Schema version of the workspace database this build provides.
    pub database_schema: i64,
    /// `local` or `server`: which backend this device opened. Operations that
    /// need this machine hide themselves when it is `server` (ADR-0053).
    pub backend: String,
}

#[tauri::command]
pub fn get_app_info(app: AppHandle, state: State<'_, AppState>) -> Result<AppInfo, String> {
    let auto_check_updates = load_config(&state)?.auto_check_updates;
    Ok(AppInfo {
        version: app.package_info().version.to_string(),
        auto_check_updates,
        database_schema: cowbot_client::latest_database_schema_version(),
        backend: backend_name(state.backend).to_string(),
    })
}

/// Persist whether CowBot checks for updates when it starts.
#[tauri::command]
pub fn set_auto_check_updates(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut config = load_config(&state)?;
    config.auto_check_updates = enabled;
    config::save(&state.config_path, &config)
        .map_err(|error| format!("failed to write {}: {error}", state.config_path.display()))
}

/// The stored interface locale preference, or `None` to follow the system.
#[tauri::command]
pub fn get_ui_locale(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(load_config(&state)?.ui_locale)
}

/// Persist the interface locale preference.
///
/// The backend stores the value without interpreting it; the frontend owns the
/// locale registry and falls back to the system language for unknown values.
#[tauri::command]
pub fn set_ui_locale(state: State<'_, AppState>, locale: String) -> Result<(), String> {
    let locale = locale.trim();
    if locale.is_empty() || locale.len() > 35 {
        return Err("Unsupported interface locale.".to_string());
    }
    let mut config = load_config(&state)?;
    config.ui_locale = Some(locale.to_string());
    config::save(&state.config_path, &config)
        .map_err(|error| format!("failed to write {}: {error}", state.config_path.display()))
}

/// Whether agent names show a `bot` tag in the interface.
#[tauri::command]
pub fn get_show_agent_bot_tags(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(load_config(&state)?.show_agent_bot_tags)
}

/// Persist whether agent names show a `bot` tag in the interface.
#[tauri::command]
pub fn set_show_agent_bot_tags(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut config = load_config(&state)?;
    config.show_agent_bot_tags = enabled;
    config::save(&state.config_path, &config)
        .map_err(|error| format!("failed to write {}: {error}", state.config_path.display()))
}

/// Restart the app so an installed update takes effect.
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}

/// One workspace database offered by the desktop switcher.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceView {
    pub database: String,
    pub name: String,
    pub active: bool,
    pub available: bool,
}

fn ensure_no_running_tasks(state: &AppState) -> Result<(), String> {
    let running = state.running.lock();
    if !running.is_empty() {
        return Err("Wait for running tasks to finish before changing workspaces.".to_string());
    }
    drop(running);

    let replies = state.replies.lock();
    if replies.is_empty() {
        Ok(())
    } else {
        Err(
            "Wait for agents to finish answering in channels before changing workspaces."
                .to_string(),
        )
    }
}

/// A server connection as the settings view sees it.
///
/// The token never crosses this boundary; the view only learns which account
/// and workspace are connected, so a screenshot or a log of the settings page
/// cannot leak a session.
#[derive(Debug, Clone, Serialize)]
pub struct ServerView {
    /// `local` or `server`: which backend this device opens.
    pub mode: String,
    /// Absolute origin of the saved connection, when one exists.
    pub base_url: Option<String>,
    /// Account that signed in, when one did.
    pub email: Option<String>,
    /// Workspace this device is bound to on that server.
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
}

fn server_view(config: &config::AppConfig) -> ServerView {
    ServerView {
        mode: backend_name(config.backend).to_string(),
        base_url: config.server.as_ref().map(|saved| saved.base_url.clone()),
        email: config.server.as_ref().map(|saved| saved.email.clone()),
        workspace_id: config
            .server
            .as_ref()
            .map(|saved| saved.workspace_id.clone()),
        workspace_name: config
            .server
            .as_ref()
            .map(|saved| saved.workspace_name.clone()),
    }
}

/// The stored name of a backend mode, as the frontend sees it.
fn backend_name(backend: config::BackendMode) -> &'static str {
    match backend {
        config::BackendMode::Local => "local",
        config::BackendMode::Server => "server",
    }
}

#[tauri::command]
pub fn get_server_settings(state: State<'_, AppState>) -> Result<ServerView, String> {
    Ok(server_view(&load_config(&state)?))
}

/// One workspace the signed-in account can enter, for the sign-in picker.
#[derive(Debug, Clone, Serialize)]
pub struct ServerWorkspaceView {
    pub id: String,
    pub name: String,
    /// What this account may do in it, so the picker can say so and the member
    /// panel knows what the newly opened workspace will allow.
    pub role: Role,
}

/// The account and workspaces a sign-in would open, before anything is saved.
#[derive(Debug, Clone, Serialize)]
pub struct ServerProbe {
    pub workspaces: Vec<ServerWorkspaceView>,
}

/// Reach a server and sign in without changing where this device points.
///
/// Used by the settings form to list the account's workspaces before the user
/// chooses one, and by "Test connection" when a session is already saved. It
/// never writes the configuration, so a failed attempt leaves the open
/// workspace exactly as it was.
#[tauri::command]
pub async fn probe_server(
    base_url: String,
    email: String,
    password: String,
) -> Result<ServerProbe, String> {
    let base_url = config::normalize_base_url(&base_url)?;
    if email.trim().is_empty() || password.is_empty() {
        return Err("Enter the email and password for the server account.".to_string());
    }
    let client = cowbot_client::RemoteClient::sign_in(&base_url, email.trim(), &password, None)
        .await
        .map_err(|error| error.to_string())?;
    let workspaces = client
        .account_workspaces()
        .await
        .map_err(|error| error.to_string())?;
    Ok(ServerProbe {
        workspaces: workspaces
            .into_iter()
            .map(|workspace| ServerWorkspaceView {
                id: workspace.id,
                name: workspace.name,
                role: workspace.role,
            })
            .collect(),
    })
}

/// Sign in to a server, save the connection, and restart into it.
#[tauri::command]
pub async fn connect_server(
    app: AppHandle,
    state: State<'_, AppState>,
    base_url: String,
    email: String,
    password: String,
    workspace_id: String,
) -> Result<(), String> {
    ensure_no_running_tasks(&state)?;
    let base_url = config::normalize_base_url(&base_url)?;
    if email.trim().is_empty() || password.is_empty() {
        return Err("Enter the email and password for the server account.".to_string());
    }
    if workspace_id.trim().is_empty() {
        return Err("Choose a workspace to open.".to_string());
    }

    // Sign in first: a server that refuses the account must not leave the
    // device pointed at a workspace it cannot open.
    let client = cowbot_client::RemoteClient::sign_in(
        &base_url,
        email.trim(),
        &password,
        Some(workspace_id.trim()),
    )
    .await
    .map_err(|error| error.to_string())?;
    let session = client.session().await.map_err(|error| error.to_string())?;

    let mut config = load_config(&state)?;
    config.backend = config::BackendMode::Server;
    config.server = Some(config::ServerConnection {
        base_url,
        email: session.user.email.clone(),
        workspace_id: session.workspace.id.0.clone(),
        workspace_name: session.workspace.name.clone(),
        token: client.token().to_string(),
    });
    config::save(&state.config_path, &config)
        .map_err(|error| format!("failed to write {}: {error}", state.config_path.display()))?;
    app.restart();
}

/// Join a server by redeeming an invitation, creating the account if needed.
///
/// The counterpart of `connect_server` for someone who has never signed in: the
/// invitation is the credential, so there is no password to sign in with first
/// (ADR-0066). On success the connection is saved and the app restarts into it,
/// exactly as signing in does — the session the server issued is the same kind.
#[tauri::command]
pub async fn redeem_invite(
    app: AppHandle,
    state: State<'_, AppState>,
    base_url: String,
    invite_token: String,
    name: String,
    email: String,
    password: String,
) -> Result<(), String> {
    ensure_no_running_tasks(&state)?;
    let base_url = config::normalize_base_url(&base_url)?;
    if invite_token.trim().is_empty() {
        return Err("Enter the invitation code you were given.".to_string());
    }
    if name.trim().is_empty() || email.trim().is_empty() || password.is_empty() {
        return Err("Enter your name, email, and a password.".to_string());
    }

    // Nothing is written until the server has accepted the code, so a wrong or
    // expired one leaves this device pointed where it was.
    let client = cowbot_client::RemoteClient::redeem(
        &base_url,
        invite_token.trim(),
        name.trim(),
        email.trim(),
        &password,
    )
    .await
    .map_err(|error| error.to_string())?;
    let session = client.session().await.map_err(|error| error.to_string())?;

    let mut config = load_config(&state)?;
    config.backend = config::BackendMode::Server;
    config.server = Some(config::ServerConnection {
        base_url,
        email: session.user.email.clone(),
        workspace_id: session.workspace.id.0.clone(),
        workspace_name: session.workspace.name.clone(),
        token: client.token().to_string(),
    });
    config::save(&state.config_path, &config)
        .map_err(|error| format!("failed to write {}: {error}", state.config_path.display()))?;
    app.restart();
}

/// One of the account's server workspaces, for switching without signing in
/// again.
#[tauri::command]
pub async fn list_server_workspaces(
    state: State<'_, AppState>,
) -> Result<Vec<ServerWorkspaceView>, String> {
    let Some(client) = state.server_client.clone() else {
        return Err("This device is not connected to a server.".to_string());
    };
    let workspaces = client
        .account_workspaces()
        .await
        .map_err(|error| error.to_string())?;
    Ok(workspaces
        .into_iter()
        .map(|workspace| ServerWorkspaceView {
            id: workspace.id,
            name: workspace.name,
            role: workspace.role,
        })
        .collect())
}

/// Open a different workspace on the saved server account.
#[tauri::command]
pub async fn switch_server_workspace(
    app: AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<(), String> {
    ensure_no_running_tasks(&state)?;
    let mut config = load_config(&state)?;
    let Some(saved) = config.server.as_mut() else {
        return Err("This device is not connected to a server.".to_string());
    };
    if saved.workspace_id == workspace_id {
        return Err("That workspace is already open.".to_string());
    }
    // Confirm the account may still enter it, and learn its current name.
    let client =
        cowbot_client::RemoteClient::resume(&saved.base_url, &saved.token, Some(&workspace_id))
            .await
            .map_err(|error| error.to_string())?;
    let session = client.session().await.map_err(|error| error.to_string())?;
    saved.workspace_id = session.workspace.id.0.clone();
    saved.workspace_name = session.workspace.name.clone();
    config::save(&state.config_path, &config).map_err(|error| error.to_string())?;
    app.restart();
}

/// Stop using a server and restart into the local workspace.
///
/// The saved connection is kept, so switching back does not ask for the
/// password again; only `disconnect_server` forgets it.
#[tauri::command]
pub fn use_local_backend(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    ensure_no_running_tasks(&state)?;
    let mut config = load_config(&state)?;
    config.backend = config::BackendMode::Local;
    config::save(&state.config_path, &config).map_err(|error| error.to_string())?;
    app.restart();
}

/// Forget the saved session and restart into the local workspace.
#[tauri::command]
pub fn disconnect_server(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    ensure_no_running_tasks(&state)?;
    let mut config = load_config(&state)?;
    config.backend = config::BackendMode::Local;
    config.server = None;
    config::save(&state.config_path, &config).map_err(|error| error.to_string())?;
    app.restart();
}

#[tauri::command]
pub fn list_workspaces(state: State<'_, AppState>) -> Result<Vec<WorkspaceView>, String> {
    let config = load_config(&state)?;
    let active = config
        .active_workspace_database
        .clone()
        .unwrap_or_else(|| config::DEFAULT_DATABASE.to_string());

    Ok(config
        .workspaces
        .iter()
        .map(|entry| WorkspaceView {
            database: entry.database.clone(),
            name: entry.name.clone(),
            active: entry.database == active,
            available: config::resolve_database_path(&state.database_dir, &entry.database)
                .map(|path| path.exists())
                .unwrap_or(false),
        })
        .collect())
}

/// Switch to another local workspace database and restart the app.
#[tauri::command]
pub fn switch_workspace(
    state: State<'_, AppState>,
    app: AppHandle,
    database: String,
) -> Result<(), String> {
    ensure_no_running_tasks(&state)?;
    let path = config::resolve_database_path(&state.database_dir, &database)?;
    if !path.exists() {
        return Err("Workspace database was not found.".to_string());
    }

    let mut config = load_config(&state)?;
    config.active_workspace_database = Some(database);
    config::save(&state.config_path, &config)
        .map_err(|error| format!("failed to write {}: {error}", state.config_path.display()))?;

    app.restart();
}

/// Append the package extension when the save dialog returns a bare name.
fn package_output_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Choose where to save the package.".to_string());
    }

    let mut path = PathBuf::from(trimmed);
    let has_extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("cowpkg"))
        .unwrap_or(false);
    if !has_extension {
        path.set_extension("cowpkg");
    }
    Ok(path)
}

fn remove_database_files(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

/// Export the current workspace as a `.cowpkg` package.
#[tauri::command]
pub async fn export_workspace(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let path = package_output_path(&path)?;
    let bytes = local(&state)?
        .export_workspace()
        .await
        .map_err(|error| error.to_string())?;

    let temporary = path.with_extension("cowpkg.tmp");
    std::fs::write(&temporary, &bytes)
        .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|error| format!("failed to replace {}: {error}", path.display()))?;
    }
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("failed to write {}: {error}", path.display()));
    }

    Ok(path.display().to_string())
}

/// Import a `.cowpkg` package as a new workspace database and restart.
#[tauri::command]
pub async fn import_workspace(
    state: State<'_, AppState>,
    app: AppHandle,
    path: String,
) -> Result<(), String> {
    ensure_no_running_tasks(&state)?;

    let source = PathBuf::from(path.trim());
    if source.as_os_str().is_empty() {
        return Err("Choose a package to import.".to_string());
    }
    let metadata = std::fs::metadata(&source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?;
    if metadata.len() > cowbot_client::MAX_PACKAGE_BYTES {
        return Err(format!(
            "The package is larger than {} MB.",
            cowbot_client::MAX_PACKAGE_BYTES / (1024 * 1024)
        ));
    }
    let bytes = std::fs::read(&source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?;

    let database = format!("{}/{}.db", config::WORKSPACES_DIR, WorkspaceId::random().0);
    let database_path = config::resolve_database_path(&state.database_dir, &database)?;
    if let Some(parent) = database_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }

    let (client, summary) =
        match cowbot_client::LocalClient::create_from_package(&database_path, &bytes).await {
            Ok(result) => result,
            Err(error) => {
                remove_database_files(&database_path);
                return Err(error.to_string());
            }
        };
    drop(client);

    let mut config = load_config(&state)?;
    config.workspaces.push(config::WorkspaceEntry {
        database: database.clone(),
        name: summary.workspace_name,
    });
    config.active_workspace_database = Some(database);
    if let Err(error) = config::save(&state.config_path, &config) {
        remove_database_files(&database_path);
        return Err(format!(
            "failed to write {}: {error}",
            state.config_path.display()
        ));
    }

    app.restart();
}

/// Create a new empty workspace database and restart into it.
#[tauri::command]
pub async fn create_workspace(
    state: State<'_, AppState>,
    app: AppHandle,
    name: String,
) -> Result<(), String> {
    ensure_no_running_tasks(&state)?;

    let name = name.trim();
    let database = format!("{}/{}.db", config::WORKSPACES_DIR, WorkspaceId::random().0);
    let database_path = config::resolve_database_path(&state.database_dir, &database)?;
    if let Some(parent) = database_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }

    let client =
        match cowbot_client::LocalClient::create_workspace_database(&database_path, name).await {
            Ok(client) => client,
            Err(error) => {
                remove_database_files(&database_path);
                return Err(error.to_string());
            }
        };
    drop(client);

    let mut config = load_config(&state)?;
    config.workspaces.push(config::WorkspaceEntry {
        database: database.clone(),
        name: name.to_string(),
    });
    config.active_workspace_database = Some(database);
    if let Err(error) = config::save(&state.config_path, &config) {
        remove_database_files(&database_path);
        return Err(format!(
            "failed to write {}: {error}",
            state.config_path.display()
        ));
    }

    app.restart();
}

/// Registered local git repositories available to worktree tasks.
#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Result<Vec<config::ProjectView>, String> {
    let config = load_config(&state)?;
    Ok(config.projects.iter().map(config::project_view).collect())
}

/// Register a local git repository after resolving its work-tree root.
#[tauri::command]
pub fn add_project(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<config::ProjectView>, String> {
    let mut config = load_config(&state)?;
    let project = config::add_project(&mut config, &path)?;
    config::prune_project_worktrees(&project);
    let config = persist_config(&state, config)?;
    Ok(config.projects.iter().map(config::project_view).collect())
}

/// Remove a project from the registry without touching its files.
#[tauri::command]
pub fn remove_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<config::ProjectView>, String> {
    let mut config = load_config(&state)?;
    config::remove_project(&mut config, &project_id)?;
    let config = persist_config(&state, config)?;
    Ok(config.projects.iter().map(config::project_view).collect())
}

/// Local branches of a registered project and its current branch.
#[tauri::command]
pub fn list_project_branches(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<config::ProjectBranches, String> {
    let config = load_config(&state)?;
    let project = config
        .project(&project_id)
        .ok_or_else(|| format!("project '{project_id}' was not found"))?;
    config::project_branches(&project.path)
}
/// Send a minimal completion with the drafted provider settings.
///
/// A blank API key falls back to the key stored for `provider_id`, so a saved
/// provider can be tested without retyping its secret.
#[tauri::command]
pub async fn test_model_provider(
    state: State<'_, AppState>,
    provider_id: Option<String>,
    protocol: config::ProviderProtocol,
    base_url: String,
    model: String,
    api_key: Option<String>,
) -> Result<String, String> {
    let config = load_config(&state)?;
    let stored_key = provider_id
        .as_deref()
        .and_then(|id| config.provider(id))
        .and_then(|provider| provider.api_key.clone());
    let api_key = config::normalize_api_key(api_key.as_deref()).or(stored_key);
    let (base_url, model) = config::validate_endpoint(&base_url, &model)?;
    let resolved = config::ResolvedModel {
        kind: config::ProviderKind::Remote(protocol),
        model,
        base_url: Some(base_url),
        api_key,
    };
    let provider = config::build_provider(&resolved);
    let request = ChatCompletionRequest {
        model: resolved.model.clone(),
        messages: vec![ChatMessage::user("Reply with the single word OK.")],
        tools: Vec::new(),
        temperature: Some(0.0),
    };

    let response = tokio::time::timeout(Duration::from_secs(30), provider.chat_complete(&request))
        .await
        .map_err(|_| "The provider did not respond within 30 seconds.".to_string())?
        .map_err(|error| error.to_string())?;

    let reply = response.content.unwrap_or_default();
    let reply = reply.trim();
    if reply.is_empty() {
        Ok(format!(
            "Connected to '{}'. The provider returned an empty reply.",
            resolved.model
        ))
    } else {
        Ok(format!(
            "Connected to '{}': {}",
            resolved.model,
            summarize_reply(reply)
        ))
    }
}

/// List the model ids an endpoint reports, using its saved key when none is
/// typed.
///
/// The listing shape differs between protocols, so the request goes through
/// the protocol client rather than a shared HTTP helper.
/// One model the endpoint reported, with the window it advertises.
#[derive(Debug, Clone, Serialize)]
pub struct FetchedModel {
    pub id: String,
    /// Absent when the endpoint does not report one.
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

#[tauri::command]
pub async fn fetch_provider_models(
    state: State<'_, AppState>,
    provider_id: Option<String>,
    protocol: config::ProviderProtocol,
    base_url: String,
    api_key: Option<String>,
) -> Result<Vec<FetchedModel>, String> {
    let config_value = load_config(&state)?;
    let stored_key = provider_id
        .as_deref()
        .and_then(|id| config_value.provider(id))
        .and_then(|provider| provider.api_key.clone());
    let api_key = config::normalize_api_key(api_key.as_deref()).or(stored_key);
    let base_url = config::normalize_base_url(&base_url)?;
    let provider = config::build_remote_provider(protocol, &base_url, api_key);

    let discovered = tokio::time::timeout(Duration::from_secs(30), provider.list_models())
        .await
        .map_err(|_| "The provider did not respond within 30 seconds.".to_string())?
        .map_err(|error| error.to_string())?;

    if discovered.is_empty() {
        return Err("The endpoint returned no models.".to_string());
    }

    // Only a saved provider has somewhere to keep the metadata, and a value
    // the user typed always wins over what the endpoint claims.
    if let Some(id) = provider_id.as_deref() {
        if let Some(provider) = config_value.provider(id) {
            let merged = merge_discovered_metadata(&provider.model_metadata, &discovered);
            if merged != provider.model_metadata {
                let mut updated = config_value.clone();
                if let Some(target) = updated.providers.iter_mut().find(|entry| entry.id == id) {
                    target.model_metadata = merged;
                }
                config::save(&state.config_path, &updated).map_err(|error| {
                    format!("failed to write {}: {error}", state.config_path.display())
                })?;
            }
        }
    }

    Ok(discovered
        .into_iter()
        .map(|model| FetchedModel {
            id: model.id,
            context_window: model.context_window,
            max_output_tokens: model.max_output_tokens,
        })
        .collect())
}

/// Fill window metadata the provider does not already carry.
///
/// A model the user gave a window keeps it; only a missing entry, or one with
/// no window at all, takes the endpoint's figure. The same rule applies per
/// field, so a hand-typed output limit survives a fetch that reports none.
fn merge_discovered_metadata(
    existing: &BTreeMap<String, config::ModelMetadata>,
    discovered: &[cowbot_llm::DiscoveredModel],
) -> BTreeMap<String, config::ModelMetadata> {
    let mut merged = existing.clone();
    for model in discovered {
        if model.context_window.is_none() && model.max_output_tokens.is_none() {
            continue;
        }
        let entry = merged.entry(model.id.clone()).or_default();
        if entry.context_window.is_none() {
            entry.context_window = model.context_window;
        }
        if entry.max_output_tokens.is_none() {
            entry.max_output_tokens = model.max_output_tokens;
        }
    }
    merged
}

fn summarize_reply(reply: &str) -> String {
    const LIMIT: usize = 120;
    let mut summary: String = reply.chars().take(LIMIT).collect();
    if reply.chars().count() > LIMIT {
        summary.push('…');
    }
    summary
}

/// One argument of a composer command, flattened for the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct CommandArgInfo {
    pub placeholder: String,
    pub required: bool,
    pub kind: String,
    pub choices: Vec<String>,
}

/// A composer command, flattened for the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    pub name: String,
    pub aliases: Vec<String>,
    pub usage: String,
    pub summary: String,
    pub target: String,
    pub args: Vec<CommandArgInfo>,
}

fn command_info(spec: &CommandSpec) -> CommandInfo {
    CommandInfo {
        name: spec.name.to_string(),
        aliases: spec.aliases.iter().map(|alias| alias.to_string()).collect(),
        usage: spec.usage.to_string(),
        summary: spec.summary.to_string(),
        target: spec.target.as_str().to_string(),
        args: spec
            .args
            .iter()
            .map(|arg| CommandArgInfo {
                placeholder: arg.placeholder.to_string(),
                required: arg.required,
                kind: arg.kind.as_str().to_string(),
                choices: arg.kind.choices().iter().map(|c| c.to_string()).collect(),
            })
            .collect(),
    }
}

/// One answer to a `/btw` side question.
#[derive(Debug, Clone, Serialize)]
pub struct SideAnswerView {
    pub agent_id: String,
    pub agent_name: String,
    pub content: String,
    pub model: String,
    pub call_id: String,
}

/// Answer a `/btw` side question without changing the conversation.
///
/// Nothing is posted, no context entry is appended, and no routing turn is
/// recorded; the view returns the answer for an ephemeral panel. `agent_id`
/// picks the responder and defaults to the chat's own agent or its default
/// responder. The model comes from the conversation's settings, or from
/// `model` for an agent chat and a group channel (ADR-0041).
#[tauri::command]
pub async fn ask_side_question(
    state: State<'_, AppState>,
    channel_id: String,
    agent_id: Option<String>,
    question: String,
    model: Option<String>,
    approval_level: Option<ApprovalLevel>,
) -> Result<SideAnswerView, String> {
    let channel_id = ChannelId::new(channel_id);
    let channel = state
        .client
        .get_channel(&channel_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("channel '{}' was not found", channel_id.0))?;

    let (llm, default_model) = if channel.is_conversation() {
        let settings = state
            .client
            .conversation_settings(&channel_id)
            .await
            .map_err(|error| error.to_string())?;
        conversation_runtime(&state, &settings)
    } else {
        state.runtime_model()
    };
    let model = model
        .filter(|model| !model.trim().is_empty())
        .unwrap_or(default_model);

    let gate = local(&state)?
        .side_question_gate(AssignmentId::new(format!("side-question-{}", channel_id.0)));
    let responder = agent_id.as_deref().map(AgentId::new);
    let answer = local(&state)?
        .ask_side_question(
            &channel_id,
            responder.as_ref(),
            &question,
            &model,
            llm,
            state.tools.clone(),
            Some(gate),
            ChannelTurnOptions {
                approval_level: approval_level.unwrap_or(ApprovalLevel::Ask),
                max_concurrency: 4,
                use_model_permissions: state.uses_remote_model(),
                approval_timeout: None,
                platform_context: project_platform_context(&state),
                participant_capacity: Vec::new(),
            },
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(SideAnswerView {
        agent_id: answer.agent_id.0,
        agent_name: answer.agent_name,
        content: answer.content,
        model: answer.model,
        call_id: answer.call_id,
    })
}

/// The composer's slash command catalogue, fetched once per frontend session.
#[tauri::command]
pub fn list_commands() -> Vec<CommandInfo> {
    command_catalogue().iter().map(command_info).collect()
}

/// One composer command with its arguments filled in.
#[derive(Debug, Clone, Serialize)]
pub struct CommandInvocationView {
    pub name: String,
    pub args: Vec<String>,
    pub raw_args: String,
}

/// The outcome of reading composer input as a slash command.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandParseView {
    NotACommand,
    Invocation(CommandInvocationView),
    Unknown { name: String },
    MissingArgument { name: String, usage: String },
}

/// Read composer input as a slash command.
///
/// Called once per Enter press while the draft starts with `/`, so typing
/// never costs an IPC round trip.
#[tauri::command]
pub fn parse_command(input: String) -> CommandParseView {
    match command_parse(&input) {
        CommandParse::NotACommand => CommandParseView::NotACommand,
        CommandParse::Invocation(invocation) => {
            CommandParseView::Invocation(CommandInvocationView {
                name: invocation.name,
                args: invocation.args,
                raw_args: invocation.raw_args,
            })
        }
        CommandParse::Unknown { name } => CommandParseView::Unknown { name },
        CommandParse::MissingArgument { name, usage } => {
            CommandParseView::MissingArgument { name, usage }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::assignment::{
        AssignmentMode, AssignmentOrigin, AssignmentRequester, ReplyMode,
    };
    use cowbot_core::conversation::{ContextProjection, ConversationParticipantContext};
    use cowbot_core::memory::RecallPreview;

    fn draft(name: &str, base_url: &str, model: &str, api_key: Option<&str>) -> ProviderDraft {
        ProviderDraft {
            provider_id: None,
            protocol: config::ProviderProtocol::OpenAi,
            name: name.to_string(),
            base_url: base_url.to_string(),
            models: vec![model.to_string()],
            model: model.to_string(),
            model_metadata: BTreeMap::new(),
            api_key: api_key.map(str::to_string),
            clear_api_key: false,
        }
    }

    #[test]
    fn provider_draft_accepts_camel_case_fields() {
        let draft: ProviderDraft = serde_json::from_value(serde_json::json!({
            "providerId": "p1",
            "protocol": "anthropic",
            "name": "Claude",
            "baseUrl": "https://api.anthropic.com",
            "models": ["claude-sonnet-5-5", "claude-opus-5-5"],
            "model": "claude-sonnet-5-5",
            "modelMetadata": {
                "claude-sonnet-5-5": {
                    "context_window": 200000,
                    "max_output_tokens": 8192
                }
            },
            "apiKey": "secret",
            "clearApiKey": false
        }))
        .unwrap();

        assert_eq!(draft.provider_id.as_deref(), Some("p1"));
        assert_eq!(draft.protocol, config::ProviderProtocol::Anthropic);
        assert_eq!(draft.base_url, "https://api.anthropic.com");
        assert_eq!(
            draft.models,
            vec![
                "claude-sonnet-5-5".to_string(),
                "claude-opus-5-5".to_string()
            ]
        );
        assert_eq!(
            draft.model_metadata["claude-sonnet-5-5"].context_window,
            Some(200_000)
        );
        assert_eq!(
            draft.model_metadata["claude-sonnet-5-5"].max_output_tokens,
            Some(8_192)
        );
        assert_eq!(draft.api_key.as_deref(), Some("secret"));
        assert!(!draft.clear_api_key);
    }

    #[test]
    fn first_provider_becomes_active() {
        let mut settings = config::AppConfig::default();

        upsert_provider(
            &mut settings,
            &draft(
                "OpenAI",
                "https://api.openai.com/v1",
                "gpt-4o",
                Some("secret"),
            ),
        )
        .unwrap();

        assert_eq!(settings.providers.len(), 1);
        assert_eq!(
            settings.active_provider_id.as_deref(),
            Some(settings.providers[0].id.as_str())
        );
        assert_eq!(settings.providers[0].api_key.as_deref(), Some("secret"));
    }

    #[test]
    fn saving_a_provider_keeps_its_protocol() {
        let mut settings = config::AppConfig::default();
        let mut claude = draft(
            "Claude",
            "https://api.anthropic.com",
            "claude-sonnet-5-5",
            None,
        );
        claude.protocol = config::ProviderProtocol::Anthropic;

        upsert_provider(&mut settings, &claude).unwrap();

        assert_eq!(
            settings.providers[0].protocol,
            config::ProviderProtocol::Anthropic
        );
    }

    #[test]
    fn updating_with_a_blank_key_keeps_the_stored_key() {
        let mut settings = config::AppConfig::default();
        upsert_provider(
            &mut settings,
            &draft(
                "OpenAI",
                "https://api.openai.com/v1",
                "gpt-4o",
                Some("secret"),
            ),
        )
        .unwrap();
        let id = settings.providers[0].id.clone();

        let mut update = draft(
            "Renamed",
            "https://api.example.com/v1",
            "example-model",
            None,
        );
        update.provider_id = Some(id);
        upsert_provider(&mut settings, &update).unwrap();

        assert_eq!(settings.providers[0].name, "Renamed");
        assert_eq!(settings.providers[0].base_url, "https://api.example.com/v1");
        assert_eq!(settings.providers[0].api_key.as_deref(), Some("secret"));
    }

    #[test]
    fn saving_a_provider_keeps_every_model() {
        let mut settings = config::AppConfig::default();
        let mut openai = draft(
            "OpenAI",
            "https://api.openai.com/v1",
            "gpt-4o",
            Some("secret"),
        );
        openai.models = vec![
            "gpt-4o".to_string(),
            "gpt-4o-mini".to_string(),
            "gpt-4o".to_string(),
        ];

        upsert_provider(&mut settings, &openai).unwrap();

        assert_eq!(
            settings.providers[0].models,
            vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()]
        );
    }

    #[test]
    fn switching_the_active_model_keeps_the_stored_key() {
        let mut settings = config::AppConfig::default();
        upsert_provider(
            &mut settings,
            &draft(
                "OpenAI",
                "https://api.openai.com/v1",
                "gpt-4o",
                Some("secret"),
            ),
        )
        .unwrap();
        let id = settings.providers[0].id.clone();

        let mut update = draft("OpenAI", "https://api.openai.com/v1", "gpt-4o-mini", None);
        update.provider_id = Some(id);
        update.models = vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()];
        upsert_provider(&mut settings, &update).unwrap();

        assert_eq!(settings.providers[0].model, "gpt-4o-mini");
        assert_eq!(
            settings.providers[0].models,
            vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()]
        );
        assert_eq!(settings.providers[0].api_key.as_deref(), Some("secret"));
    }

    #[test]
    fn clearing_the_key_removes_it() {
        let mut settings = config::AppConfig::default();
        upsert_provider(
            &mut settings,
            &draft(
                "OpenAI",
                "https://api.openai.com/v1",
                "gpt-4o",
                Some("secret"),
            ),
        )
        .unwrap();
        let id = settings.providers[0].id.clone();

        let mut update = draft("OpenAI", "https://api.openai.com/v1", "gpt-4o", None);
        update.provider_id = Some(id);
        update.clear_api_key = true;
        upsert_provider(&mut settings, &update).unwrap();

        assert!(settings.providers[0].api_key.is_none());
    }

    #[test]
    fn adding_a_second_provider_keeps_the_active_one() {
        let mut settings = config::AppConfig::default();
        upsert_provider(
            &mut settings,
            &draft("First", "https://api.openai.com/v1", "gpt-4o", None),
        )
        .unwrap();
        let first_id = settings.providers[0].id.clone();

        upsert_provider(
            &mut settings,
            &draft("Second", "https://api.example.org/v1", "other-model", None),
        )
        .unwrap();

        assert_eq!(settings.providers.len(), 2);
        assert_eq!(
            settings.active_provider_id.as_deref(),
            Some(first_id.as_str())
        );
    }

    #[test]
    fn updating_an_unknown_provider_fails() {
        let mut settings = config::AppConfig::default();

        let mut update = draft("Name", "https://api.example.com/v1", "model", None);
        update.provider_id = Some("missing".to_string());

        assert!(upsert_provider(&mut settings, &update).is_err());
        assert!(settings.providers.is_empty());
    }

    #[test]
    fn exported_packages_get_the_cowpkg_extension() {
        assert_eq!(
            package_output_path("backup").unwrap(),
            PathBuf::from("backup.cowpkg")
        );
        assert_eq!(
            package_output_path("backup.COWPKG").unwrap(),
            PathBuf::from("backup.COWPKG")
        );
        assert_eq!(
            package_output_path("backup.zip").unwrap(),
            PathBuf::from("backup.cowpkg")
        );
        assert!(package_output_path("   ").is_err());
    }

    fn context_with_tokens(total: u32) -> ConversationContext {
        ConversationContext {
            channel_id: ChannelId::new("channel-1"),
            agent_id: AgentId::new("agent-1"),
            model: "test-model".to_string(),
            provider_id: Some("provider-1".to_string()),
            projection: ContextProjection {
                system_tokens: total,
                ..ContextProjection::default()
            },
            cache_last_turn: CacheUsageSummary::default(),
            cache_conversation: CacheUsageSummary::default(),
            participants: Vec::new(),
        }
    }

    #[test]
    fn context_view_maps_usage_states_and_unknown_windows() {
        let settings = cowbot_core::conversation::ConversationSettings::new(
            ChannelId::new("channel-1"),
            Some("provider-1".to_string()),
            "test-model",
        );

        let neutral = context_view_from_parts(
            context_with_tokens(5_000),
            &settings,
            &HashMap::new(),
            Some(10_000),
            Some(1_000),
        );
        assert_eq!(neutral.usable_tokens, Some(7_976));
        assert_eq!(neutral.total_tokens, 5_000);
        assert!(neutral.percent_used.unwrap() > 62.0);
        assert!(neutral.percent_used.unwrap() < 63.0);
        assert_eq!(neutral.state, ContextUsageState::Neutral);
        assert_eq!(neutral.compact_threshold, 80);
        assert!(!neutral.auto_compact);
        assert_eq!(neutral.participants.len(), 1);
        assert_eq!(neutral.participants[0].agent_id, "agent-1");
        assert!(!neutral.router_enabled);
        assert_eq!(neutral.default_responder, None);

        let warning = context_view_from_parts(
            context_with_tokens(6_000),
            &settings,
            &HashMap::new(),
            Some(10_000),
            Some(1_000),
        );
        assert_eq!(warning.state, ContextUsageState::Warning);

        let danger = context_view_from_parts(
            context_with_tokens(7_500),
            &settings,
            &HashMap::new(),
            Some(10_000),
            Some(1_000),
        );
        assert_eq!(danger.state, ContextUsageState::Danger);

        let full = context_view_from_parts(
            context_with_tokens(8_000),
            &settings,
            &HashMap::new(),
            Some(10_000),
            Some(1_000),
        );
        assert_eq!(full.state, ContextUsageState::Full);

        let unknown = context_view_from_parts(
            context_with_tokens(5_000),
            &settings,
            &HashMap::new(),
            None,
            None,
        );
        assert_eq!(unknown.state, ContextUsageState::Unknown);
        assert_eq!(unknown.percent_used, None);
        assert_eq!(unknown.usable_tokens, None);
        assert_eq!(unknown.total_tokens, 5_000);
    }

    #[test]
    fn context_view_adds_tool_schema_tokens_to_the_total() {
        let settings = cowbot_core::conversation::ConversationSettings::new(
            ChannelId::new("channel-1"),
            None,
            "test-model",
        );
        let mut tool_tokens = HashMap::new();
        tool_tokens.insert(AgentId::new("agent-1"), 250);
        let view = context_view_from_parts(
            context_with_tokens(1_000),
            &settings,
            &tool_tokens,
            Some(10_000),
            None,
        );
        assert_eq!(view.tool_tokens, 250);
        assert_eq!(view.total_tokens, 1_250);
    }

    #[test]
    fn context_view_reports_the_token_counter_source() {
        let settings = cowbot_core::conversation::ConversationSettings::new(
            ChannelId::new("channel-1"),
            None,
            "gpt-4o",
        );
        let mut context = context_with_tokens(1_000);
        context.projection.token_source = TokenSource::Tiktoken;
        context.projection.token_counter = Some("o200k_base".to_string());

        let view = context_view_from_parts(context, &settings, &HashMap::new(), Some(10_000), None);
        assert_eq!(view.token_source, TokenSource::Tiktoken);
        assert_eq!(view.token_counter.as_deref(), Some("o200k_base"));

        // An old payload without a source field defaults to an estimate.
        let estimated = context_view_from_parts(
            context_with_tokens(1_000),
            &settings,
            &HashMap::new(),
            None,
            None,
        );
        assert_eq!(estimated.token_source, TokenSource::Estimate);
        assert!(estimated.token_counter.is_none());
    }

    #[test]
    fn context_view_builds_one_entry_per_participant() {
        let settings = cowbot_core::conversation::ConversationSettings::group(
            ChannelId::new("channel-1"),
            None,
            "test-model",
            Some(AgentId::new("agent-2")),
        );
        let mut context = context_with_tokens(1_000);
        context.participants = vec![
            ConversationParticipantContext {
                agent_id: AgentId::new("agent-1"),
                agent_name: "Planner".to_string(),
                projection: ContextProjection {
                    system_tokens: 1_000,
                    ..ContextProjection::default()
                },
                memory_tokens: 50,
                cache_last_run: CacheUsageSummary::default(),
                cache_conversation: CacheUsageSummary::default(),
            },
            ConversationParticipantContext {
                agent_id: AgentId::new("agent-2"),
                agent_name: "Coder".to_string(),
                projection: ContextProjection {
                    system_tokens: 2_000,
                    ..ContextProjection::default()
                },
                memory_tokens: 0,
                cache_last_run: CacheUsageSummary::default(),
                cache_conversation: CacheUsageSummary::default(),
            },
        ];
        let mut tool_tokens = HashMap::new();
        tool_tokens.insert(AgentId::new("agent-1"), 100);
        tool_tokens.insert(AgentId::new("agent-2"), 300);

        let view =
            context_view_from_parts(context, &settings, &tool_tokens, Some(10_000), Some(1_000));

        assert_eq!(view.participants.len(), 2);
        assert_eq!(view.participants[0].agent_name, "Planner");
        assert_eq!(view.participants[0].total_tokens, 1_150);
        assert_eq!(view.participants[0].memory_tokens, 50);
        assert_eq!(view.memory_tokens, 50);
        assert_eq!(view.participants[1].agent_name, "Coder");
        assert_eq!(view.participants[1].total_tokens, 2_300);
        assert!(!view.router_enabled);
        assert_eq!(view.default_responder.as_deref(), Some("agent-2"));
        // The top-level fields mirror the first participant.
        assert_eq!(view.agent_id, "agent-1");
        assert_eq!(view.total_tokens, 1_150);
    }

    fn assignment_with(status: AssignmentStatus) -> Assignment {
        let mut assignment = Assignment::new_root(
            TaskId::new("task-1"),
            AssignmentOrigin::User,
            AssignmentRequester::System,
            AgentId::new("agent-1"),
            AssignmentMode::Work,
            ReplyMode::Resume,
            ApprovalLevel::Ask,
        );
        assignment.status = status;
        assignment
    }

    #[test]
    fn overview_status_prioritizes_active_work() {
        use AssignmentStatus::*;

        assert_eq!(overview_status(&[]), "queued");
        assert_eq!(
            overview_status(&[
                assignment_with(Queued),
                assignment_with(NeedsInput),
                assignment_with(Waiting),
            ]),
            "needs_input"
        );
        assert_eq!(
            overview_status(&[assignment_with(Queued), assignment_with(Waiting)]),
            "waiting"
        );
        assert_eq!(
            overview_status(&[assignment_with(Queued), assignment_with(Resume)]),
            "resume"
        );
        assert_eq!(
            overview_status(&[assignment_with(Queued), assignment_with(Running)]),
            "running"
        );
        assert_eq!(
            overview_status(&[assignment_with(Completed), assignment_with(Failed)]),
            "failed"
        );
        assert_eq!(
            overview_status(&[assignment_with(Cancelled), assignment_with(Cancelled)]),
            "cancelled"
        );
        assert_eq!(
            overview_status(&[assignment_with(Completed), assignment_with(Completed)]),
            "completed"
        );
    }

    #[test]
    fn overview_details_prefer_the_last_error() {
        let mut assignment = assignment_with(AssignmentStatus::Failed);
        assert_eq!(overview_detail(&assignment), None);
        assignment.outcome = Some(AssignmentOutcome::Failed {
            error: "boom".to_string(),
        });
        assert_eq!(overview_detail(&assignment), Some("boom".to_string()));
        assignment.last_error = Some("stalled".to_string());
        assert_eq!(overview_detail(&assignment), Some("stalled".to_string()));
    }

    fn preview_plan(tokens: u32) -> PromptPlan {
        PromptPlan {
            kind: PromptKind::Conversation,
            model: "gpt-4o".to_string(),
            provider_id: Some("provider-1".to_string()),
            slots: Vec::new(),
            messages: Vec::new(),
            memory: None,
            recall: RecallPreview {
                enabled: true,
                query: String::new(),
                block: None,
                estimated_tokens: 0,
                budget_tokens: 100,
                notes_budget_tokens: 100,
                notes_tokens: 0,
                candidates: Vec::new(),
            },
            tools: Vec::new(),
            projection: ContextProjection {
                system_tokens: tokens,
                ..ContextProjection::default()
            },
            auto_compact: true,
            compact_threshold: 80,
        }
    }

    fn preview_view(tokens: u32, window: Option<u32>, output: Option<u32>) -> ContextPreviewView {
        ContextPreviewView::from_plan(
            PromptKind::Conversation,
            Some("channel-1".to_string()),
            None,
            None,
            "agent-1".to_string(),
            "Planner".to_string(),
            "Design review".to_string(),
            preview_plan(tokens),
            window,
            output,
        )
    }

    #[test]
    fn context_preview_view_reports_the_over_budget_state() {
        let over = preview_view(8_000, Some(10_000), Some(1_000));
        assert_eq!(over.usable_tokens, Some(7_976));
        assert_eq!(over.request_tokens, 8_000);
        assert!(over.over_budget);
        assert_eq!(over.state, ContextUsageState::Full);
        assert_eq!(over.title, "Design review");

        let inside = preview_view(1_000, Some(10_000), Some(1_000));
        assert!(!inside.over_budget);
        assert_eq!(inside.state, ContextUsageState::Neutral);

        let unknown = preview_view(8_000, None, None);
        assert_eq!(unknown.percent_used, None);
        assert_eq!(unknown.state, ContextUsageState::Unknown);
        assert!(!unknown.over_budget);
    }

    #[test]
    fn list_commands_serializes_every_catalogue_entry() {
        let commands = list_commands();
        assert_eq!(commands.len(), command_catalogue().len());

        let json = serde_json::to_value(&commands).expect("command catalogue serializes");
        let approval = json
            .as_array()
            .expect("catalogue is a list")
            .iter()
            .find(|command| command["name"] == "approval")
            .expect("approval is catalogued");
        assert_eq!(approval["target"], "chat");
        assert_eq!(approval["args"][0]["kind"], "choice");
        assert_eq!(
            approval["args"][0]["choices"],
            serde_json::json!(cowbot_core::command::APPROVAL_CHOICES)
        );

        let compact = json
            .as_array()
            .expect("catalogue is a list")
            .iter()
            .find(|command| command["name"] == "compact")
            .expect("compact is catalogued");
        assert_eq!(compact["target"], "conversation");
        assert_eq!(compact["args"][0]["kind"], "participant");
        assert_eq!(compact["args"][0]["choices"], serde_json::json!([]));
    }

    #[test]
    fn parse_command_crosses_ipc_in_a_flat_shape() {
        assert_eq!(
            serde_json::to_value(parse_command("hello".to_string())).expect("serializes"),
            serde_json::json!({ "kind": "not_a_command" })
        );
        assert_eq!(
            serde_json::to_value(parse_command("/pin".to_string())).expect("serializes"),
            serde_json::json!({
                "kind": "invocation",
                "name": "pin",
                "args": [],
                "raw_args": "",
            })
        );
        assert_eq!(
            serde_json::to_value(parse_command("/task write it".to_string())).expect("serializes"),
            serde_json::json!({
                "kind": "invocation",
                "name": "task",
                "args": ["write", "it"],
                "raw_args": "write it",
            })
        );
        assert_eq!(
            serde_json::to_value(parse_command("/nope".to_string())).expect("serializes"),
            serde_json::json!({ "kind": "unknown", "name": "nope" })
        );
        assert_eq!(
            serde_json::to_value(parse_command("/rename".to_string())).expect("serializes"),
            serde_json::json!({
                "kind": "missing_argument",
                "name": "rename",
                "usage": "/rename <name>",
            })
        );
    }
}
