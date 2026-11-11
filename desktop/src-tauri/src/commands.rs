use cowbot_agent::{AgentInstance, Crew, StandardRoles};
use cowbot_client::{
    AgentUpdate, Client, ConversationSummary, MemoryFilter, MemoryNotes, MemorySave,
    MemorySettingsUpdate, MemoryUpdate, NewConversation, NewMemory, NewTask, NoteSave,
    RecalledMemory,
};
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::{Approval, ApprovalId, ApprovalLevel, BypassCritical, WorkspacePolicy};
use cowbot_core::assignment::{
    Assignment, AssignmentEvent, AssignmentId, AssignmentOutcome, AssignmentStatus,
};
use cowbot_core::audit::AuditEvent;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage};
use cowbot_core::conversation::{
    context_usage_state, usable_window, CacheUsageSummary, ChannelParticipant, CompactionOutcome,
    ContextProjection, ContextUsageState, ConversationContext, ConversationSettingsUpdate,
    ConversationTurn,
};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::memory::{MemoryEntry, MemoryKind, MemorySettings, MemoryStatus};
use cowbot_core::permission::{PermissionRule, RuleDecision};
use cowbot_core::run::{Run, RunId, ToolCallRecord};
use cowbot_core::task::{Task, TaskId};
use cowbot_core::template::TemplateDocument;
use cowbot_core::tool::ToolDefinition;
use cowbot_core::usage::{estimate_json_tokens, estimate_tokens};
use cowbot_core::worktree::{TaskWorktree, TaskWorktreeDiff, WorktreeStatus};
use cowbot_llm::{ChatCompletionRequest, ChatMessage, LlmProvider};
use cowbot_service::{ChannelTurnOptions, ParticipantCapacity};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

use crate::{config, AppState};

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
#[tauri::command]
pub async fn ensure_agent_channels(state: State<'_, AppState>) -> Result<Vec<Channel>, String> {
    state
        .client
        .ensure_agent_channels()
        .await
        .map_err(|error| error.to_string())
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

    // A single-agent conversation keeps the M2 pre-send capacity block. A
    // group is not pre-blocked: the service checks each selected branch after
    // routing. The view supplies the per-participant windows for that check.
    let mut participant_capacity = Vec::new();
    let (llm, model) = if channel.is_conversation() {
        let settings = state
            .client
            .conversation_settings(&channel_id)
            .await
            .map_err(|error| error.to_string())?;
        let runtime = conversation_runtime(&state, &settings);
        let mut view = conversation_context_view(&state, &channel_id).await?;

        if view.participants.len() <= 1 {
            let pending = estimate_tokens(content.trim());
            if let Some(usable) = view.usable_tokens {
                let projected = view.total_tokens.saturating_add(pending);
                let threshold_reached = settings.auto_compact
                    && u64::from(projected) * 100
                        >= u64::from(usable) * u64::from(settings.compact_threshold);
                if threshold_reached {
                    state
                        .client
                        .compact_conversation(&channel_id, runtime.0.clone(), &runtime.1)
                        .await
                        .map_err(|error| error.to_string())?;
                    view = conversation_context_view(&state, &channel_id).await?;
                }
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
    let client = state.client.clone();
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
pub async fn list_agents(state: State<'_, AppState>) -> Result<Vec<AgentProfile>, String> {
    state
        .client
        .list_agents()
        .await
        .map_err(|error| error.to_string())
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
        state
            .client
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

    // A worktree task is prepared before spawning so a git failure rejects
    // the run and the task stays pending.
    let worktree = match state
        .client
        .task_worktree(&task.id)
        .await
        .map_err(|error| error.to_string())?
    {
        Some(record) => match record.status {
            WorktreeStatus::Requested | WorktreeStatus::Ready => Some(
                state
                    .client
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

    let client = state.client.clone();
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
    state
        .client
        .task_worktree(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn task_worktree_diff(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<TaskWorktreeDiff, String> {
    state
        .client
        .worktree_diff(&TaskId::new(task_id))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn discard_task_worktree(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<TaskWorktree, String> {
    state
        .client
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

/// One summary entry per run group for sidebar cards and the background panel.
#[tauri::command]
pub async fn list_task_overview(
    state: State<'_, AppState>,
) -> Result<Vec<TaskOverviewEntry>, String> {
    let tasks = state
        .client
        .list_tasks()
        .await
        .map_err(|error| error.to_string())?;
    let worktree_by_task: HashMap<String, TaskWorktree> = state
        .client
        .list_worktrees()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|worktree| (worktree.task_id.0.clone(), worktree))
        .collect();

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
        let assignments = state
            .client
            .list_run_assignments(&TaskId::new(run_id.clone()))
            .await
            .map_err(|error| error.to_string())?;
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
        let latest_tool_call = state
            .client
            .latest_tool_call_for_task(&TaskId::new(run_id.clone()))
            .await
            .map_err(|error| error.to_string())?;
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
    pub system_tokens: u32,
    pub tool_tokens: u32,
    pub history_tokens: u32,
    pub latest_tokens: u32,
    pub total_tokens: u32,
    pub active_entries: u32,
    pub excluded_entries: u32,
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
    pub default_responder: Option<String>,
    pub cache_last_turn: CacheUsageSummary,
    pub cache_conversation: CacheUsageSummary,
    /// One entry per participant; the top-level fields mirror the first.
    pub participants: Vec<ConversationParticipantView>,
}

/// Fixed provider/model of a conversation, falling back to the active runtime
/// when the recorded provider was removed from the desktop configuration.
fn conversation_runtime(
    state: &AppState,
    settings: &cowbot_core::conversation::ConversationSettings,
) -> (Arc<dyn LlmProvider>, String) {
    if let Some(provider_id) = settings.provider_id.as_deref() {
        if let Ok(config) = load_config(state) {
            if let Some(provider) = config.provider(provider_id) {
                return (
                    config::build_remote_provider(
                        provider.protocol,
                        &provider.base_url,
                        provider.api_key.clone(),
                    ),
                    settings.model.clone(),
                );
            }
        }
    }
    let (llm, _) = state.runtime_model();
    (llm, settings.model.clone())
}

/// Context window and output limit metadata recorded for the conversation model.
fn conversation_window(
    state: &AppState,
    settings: &cowbot_core::conversation::ConversationSettings,
) -> (Option<u32>, Option<u32>) {
    let Ok(config) = load_config(state) else {
        return (None, None);
    };
    let provider = settings
        .provider_id
        .as_deref()
        .and_then(|id| config.provider(id))
        .or_else(|| config.active_provider());
    provider
        .and_then(|provider| provider.model_metadata.get(&settings.model))
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
    Ok(context_view_from_parts(
        context,
        &settings,
        &tool_tokens,
        context_window,
        max_output_tokens,
    ))
}

/// Tool-schema tokens each conversation participant pays in every request.
fn participant_tool_tokens(
    state: &AppState,
    context: &ConversationContext,
    agents: &[AgentProfile],
) -> HashMap<AgentId, u32> {
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
            let tokens = serde_json::to_value(state.tools.definitions_for(&agent.tools))
                .map(|value| estimate_json_tokens(&value))
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
        system_tokens: first.system_tokens,
        tool_tokens: first.tool_tokens,
        history_tokens: first.history_tokens,
        latest_tokens: first.latest_tokens,
        total_tokens: first.total_tokens,
        active_entries: first.active_entries,
        excluded_entries: first.excluded_entries,
        memory_tokens: first.memory_tokens,
        context_window,
        max_output_tokens,
        usable_tokens,
        percent_used: first.percent_used,
        state: first.state,
        auto_compact: settings.auto_compact,
        compact_threshold: settings.compact_threshold,
        router_enabled: settings.router_enabled,
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
    let total_tokens = projection.total_tokens().saturating_add(tool_tokens);
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
pub async fn update_conversation_settings(
    state: State<'_, AppState>,
    channel_id: String,
    auto_compact: Option<bool>,
    compact_threshold: Option<u32>,
    router_enabled: Option<bool>,
    default_responder: Option<String>,
) -> Result<ConversationContextView, String> {
    let channel_id = ChannelId::new(channel_id);
    state
        .client
        .update_conversation_settings(
            &channel_id,
            &ConversationSettingsUpdate {
                auto_compact,
                compact_threshold,
                router_enabled,
                default_responder: default_responder.map(AgentId::new),
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
        Some(agent_id) => state
            .client
            .compact_conversation_agent(&channel_id, &AgentId::new(agent_id), llm, &model)
            .await
            .map_err(|error| error.to_string()),
        None => state
            .client
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

/// Apply a create or update to an in-memory configuration.
///
/// A blank API key keeps the stored one; `clear_api_key` removes it. The first
/// provider added to an empty configuration becomes active.
fn upsert_provider(config: &mut config::AppConfig, draft: &ProviderDraft) -> Result<(), String> {
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
                config.active_provider_id = Some(id);
            }
        }
    }

    Ok(())
}

/// Create a provider when `provider_id` is absent, otherwise update it.
#[tauri::command]
pub fn save_model_provider(
    state: State<'_, AppState>,
    draft: ProviderDraft,
) -> Result<ModelSettings, String> {
    let mut config = load_config(&state)?;
    upsert_provider(&mut config, &draft)?;

    let config = persist_config(&state, config)?;
    Ok(model_settings(&state, &config))
}

/// Select one of a provider's known models without resaving the provider.
#[tauri::command]
pub fn set_active_model(
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
    config.providers[index].model = model;

    let config = persist_config(&state, config)?;
    Ok(model_settings(&state, &config))
}

#[tauri::command]
pub fn delete_model_provider(
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
    Ok(model_settings(&state, &config))
}

/// Select the provider used for crew runs; `None` selects the offline mock.
#[tauri::command]
pub fn activate_model_provider(
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
    Ok(model_settings(&state, &config))
}

/// First-run onboarding state for the desktop.
#[derive(Debug, Clone, Serialize)]
pub struct OnboardingState {
    pub completed: bool,
}

fn onboarding_state(config: &config::AppConfig) -> OnboardingState {
    OnboardingState {
        completed: config.onboarding_completed,
    }
}

#[tauri::command]
pub fn get_onboarding_state(state: State<'_, AppState>) -> Result<OnboardingState, String> {
    let config = load_config(&state)?;
    Ok(onboarding_state(&config))
}

/// Mark the guided first-run setup as completed or skipped.
#[tauri::command]
pub fn complete_onboarding(state: State<'_, AppState>) -> Result<OnboardingState, String> {
    let mut config = load_config(&state)?;
    config.onboarding_completed = true;
    let config = persist_config(&state, config)?;
    Ok(onboarding_state(&config))
}

/// Build version and update-check preference shown in the About settings section.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub auto_check_updates: bool,
}

#[tauri::command]
pub fn get_app_info(app: AppHandle, state: State<'_, AppState>) -> Result<AppInfo, String> {
    let auto_check_updates = load_config(&state)?.auto_check_updates;
    Ok(AppInfo {
        version: app.package_info().version.to_string(),
        auto_check_updates,
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
            available: config::resolve_database_path(&state.data_dir, &entry.database)
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
    let path = config::resolve_database_path(&state.data_dir, &database)?;
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
    let bytes = state
        .client
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
    let database_path = config::resolve_database_path(&state.data_dir, &database)?;
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
    let database_path = config::resolve_database_path(&state.data_dir, &database)?;
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
#[tauri::command]
pub async fn fetch_provider_models(
    state: State<'_, AppState>,
    provider_id: Option<String>,
    protocol: config::ProviderProtocol,
    base_url: String,
    api_key: Option<String>,
) -> Result<Vec<String>, String> {
    let config = load_config(&state)?;
    let stored_key = provider_id
        .as_deref()
        .and_then(|id| config.provider(id))
        .and_then(|provider| provider.api_key.clone());
    let api_key = config::normalize_api_key(api_key.as_deref()).or(stored_key);
    let base_url = config::normalize_base_url(&base_url)?;
    let provider = config::build_remote_provider(protocol, &base_url, api_key);

    let models = tokio::time::timeout(Duration::from_secs(30), provider.list_models())
        .await
        .map_err(|_| "The provider did not respond within 30 seconds.".to_string())?
        .map_err(|error| error.to_string())?;

    if models.is_empty() {
        return Err("The endpoint returned no models.".to_string());
    }
    Ok(models)
}

fn summarize_reply(reply: &str) -> String {
    const LIMIT: usize = 120;
    let mut summary: String = reply.chars().take(LIMIT).collect();
    if reply.chars().count() > LIMIT {
        summary.push('…');
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::assignment::{
        AssignmentMode, AssignmentOrigin, AssignmentRequester, ReplyMode,
    };
    use cowbot_core::conversation::{ContextProjection, ConversationParticipantContext};

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
        assert_eq!(view.participants[0].total_tokens, 1_100);
        assert_eq!(view.participants[0].memory_tokens, 50);
        assert_eq!(view.memory_tokens, 50);
        assert_eq!(view.participants[1].agent_name, "Coder");
        assert_eq!(view.participants[1].total_tokens, 2_300);
        assert!(!view.router_enabled);
        assert_eq!(view.default_responder.as_deref(), Some("agent-2"));
        // The top-level fields mirror the first participant.
        assert_eq!(view.agent_id, "agent-1");
        assert_eq!(view.total_tokens, 1_100);
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
}
