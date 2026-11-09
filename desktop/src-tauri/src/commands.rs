use cowbot_agent::{AgentInstance, Crew, StandardRoles};
use cowbot_client::{AgentUpdate, Client, NewTask};
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::{Approval, ApprovalId, ApprovalLevel};
use cowbot_core::assignment::{Assignment, AssignmentEvent, AssignmentId};
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::run::{Run, RunId, ToolCallRecord};
use cowbot_core::task::{Task, TaskId};
use cowbot_core::template::TemplateDocument;
use cowbot_core::tool::ToolDefinition;
use cowbot_llm::{ChatCompletionRequest, ChatMessage};
use cowbot_service::ChannelTurnOptions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
    let message = state
        .client
        .post_message(&channel_id, &content)
        .await
        .map_err(|error| error.to_string())?;

    // Agents answer in the background so the composer stays responsive; the
    // timeline refreshes from MessagePosted events and the turn-finished event.
    let client = state.client.clone();
    let (llm, model) = state.runtime_model();
    let tools = state.tools.clone();
    let channel = channel_id.clone();
    let posted = message.clone();
    let reply_id = message.id.clone();
    let running = state.replies.clone();
    let approval_level = approval_level.unwrap_or(ApprovalLevel::Ask);
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
                },
            )
            .await;
        running.lock().remove(&reply_id);

        let payload = match outcome {
            Ok(report) if report.is_empty() => None,
            Ok(report) => Some(ChannelReplyFinished {
                message_id: reply_id,
                reply_count: report.agents.len(),
                error: None,
            }),
            Err(error) => Some(ChannelReplyFinished {
                message_id: reply_id,
                reply_count: 0,
                error: Some(error.to_string()),
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

#[tauri::command]
pub async fn create_task(
    state: State<'_, AppState>,
    description: String,
    expected_output: String,
    assignee: String,
    channel_id: Option<String>,
    approval_level: ApprovalLevel,
) -> Result<Task, String> {
    let mut task = NewTask::new(description, expected_output, AgentId::new(assignee));
    if let Some(channel_id) = channel_id {
        task = task.in_channel(ChannelId::new(channel_id));
    }
    task = task.with_approval_level(approval_level);

    state
        .client
        .create_task(&task)
        .await
        .map_err(|error| error.to_string())
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

    let running = state.running.clone();
    if !running.lock().insert(task_id.clone()) {
        return Err(format!("task '{task_id}' is already running"));
    }

    let client = state.client.clone();
    let (llm, model) = state.runtime_model();
    let tools = state.tools.clone();
    let run_id = task_id.clone();

    tauri::async_runtime::spawn(async move {
        let outcome = async {
            let mut crew = build_standard_crew(&task.description, &llm, &tools, &model);
            client.run_crew(&mut crew, approval_level).await
        }
        .await;

        running.lock().remove(&run_id);

        let payload = match outcome {
            Ok(_) => RunFinished {
                task_id: run_id,
                ok: true,
                error: None,
            },
            Err(error) => RunFinished {
                task_id: run_id,
                ok: false,
                error: Some(error.to_string()),
            },
        };
        let _ = app.emit("cowbot-run-finished", payload);
    });

    Ok(())
}

/// Build the Planner/Coder/Reviewer pipeline used by the CLI, with the
/// submitted task's description as the objective.
fn build_standard_crew(
    objective: &str,
    llm: &std::sync::Arc<dyn cowbot_llm::LlmProvider>,
    tools: &cowbot_tools::ToolRegistry,
    model: &str,
) -> Crew {
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
        format!("Analyze objective and formulate architecture plan: {objective}"),
        "A clear, step-by-step implementation and verification plan.",
    )
    .assign_to(planner_id);
    let plan_id = plan_task.id.clone();

    let code_task = Task::new(
        "Execute code implementation and verify functionality in the sandbox.",
        "Working source code and sandbox execution results.",
    )
    .assign_to(coder_id)
    .with_context_task(plan_id);

    let review_task = Task::new(
        "Review implementation, tests, and diffs for correctness and edge cases.",
        "Comprehensive review findings and final approval status.",
    )
    .assign_to(reviewer_id)
    .with_context_task(code_task.id.clone());

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
) -> Result<Approval, String> {
    state
        .client
        .decide_approval(&ApprovalId::new(approval_id), approved)
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
}
