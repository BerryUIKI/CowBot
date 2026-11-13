mod commands;
mod config;
mod migration;
mod workspace_tools;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use cowbot_client::{Client, LocalClient};
use cowbot_llm::LlmProvider;
use cowbot_tools::{
    CodeExecutionTool, ReadFileTool, SubprocessSandbox, ToolRegistry, WriteFileTool,
};
use parking_lot::{Mutex, RwLock};
use tauri::{Emitter, Manager};

/// Model configuration currently in effect.
pub struct ModelRuntime {
    pub llm: Arc<dyn LlmProvider>,
    pub model: String,
    pub remote: bool,
}

/// Shared state for Tauri commands.
pub struct AppState {
    /// The active transport: local SQLite or a remote server.
    pub client: Arc<dyn Client>,
    /// The same client when it is local, for file, worktree, and crew
    /// operations that only the in-process runtime can do.
    pub local_client: Option<Arc<LocalClient>>,
    pub tools: ToolRegistry,
    /// Active model provider; replaced when the user switches providers.
    pub model: RwLock<ModelRuntime>,
    /// Path of the local `config.json` holding model providers.
    pub config_path: PathBuf,
    /// Directory holding workspace databases, `config.json`, and `cowbot.db`.
    pub data_dir: PathBuf,
    /// Task ids with a crew currently executing.
    pub running: Arc<Mutex<HashSet<String>>>,
    /// Message ids with a channel turn currently executing.
    pub replies: Arc<Mutex<HashSet<String>>>,
}

impl AppState {
    /// Snapshot the provider handle and model name for one crew run.
    pub fn runtime_model(&self) -> (Arc<dyn LlmProvider>, String) {
        let runtime = self.model.read();
        (runtime.llm.clone(), runtime.model.clone())
    }

    /// Whether the active model is a real remote provider.
    pub fn uses_remote_model(&self) -> bool {
        self.model.read().remote
    }

    /// Rebuild the LLM client after the provider configuration changed.
    pub fn apply_config(&self, config: &config::AppConfig) {
        let resolved = config::resolve_active(config);
        let llm = config::build_provider(&resolved);
        let remote = matches!(resolved.kind, config::ProviderKind::Remote(_));
        let mut runtime = self.model.write();
        runtime.model = resolved.model;
        runtime.llm = llm;
        runtime.remote = remote;
    }

    fn build_tools(
        workspace_dir: &std::path::Path,
        config_path: PathBuf,
        client: Arc<dyn Client>,
    ) -> ToolRegistry {
        Self::rooted_tools(workspace_dir, config_path, client)
    }

    /// Build a registry whose file, code, and working directory tools are
    /// rooted at `root`, plus the client-backed workspace tools.
    ///
    /// Worktree runs use this so agents edit the checkout instead of the
    /// app-data workspace; channel turns and non-worktree runs keep the shared
    /// registry.
    pub fn rooted_tools(
        root: &std::path::Path,
        config_path: PathBuf,
        client: Arc<dyn Client>,
    ) -> ToolRegistry {
        let sandbox = Arc::new(SubprocessSandbox::new(Some(root.to_path_buf())));
        let mut tools = Self::workspace_tools(config_path, client);
        tools.register(Arc::new(CodeExecutionTool::new(sandbox)));
        tools.register(Arc::new(ReadFileTool::new(Some(root.to_path_buf()))));
        tools.register(Arc::new(WriteFileTool::new(Some(root.to_path_buf()))));
        tools
    }

    fn workspace_tools(config_path: PathBuf, client: Arc<dyn Client>) -> ToolRegistry {
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(workspace_tools::ListAgentsTool::new(
            client.clone(),
        )));
        tools.register(Arc::new(workspace_tools::ListChannelsTool::new(
            client.clone(),
        )));
        tools.register(Arc::new(workspace_tools::ListTemplatesTool::new(
            client.clone(),
        )));
        tools.register(Arc::new(workspace_tools::CreateChannelTool::new(
            client.clone(),
        )));
        tools.register(Arc::new(workspace_tools::CreateTaskTool::new(
            client.clone(),
            config_path,
        )));
        tools.register(Arc::new(workspace_tools::DeleteChannelTool::new(
            client.clone(),
        )));
        tools.register(Arc::new(workspace_tools::InstantiateTeamTool::new(client)));
        tools
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            migration::migrate_legacy_database(&data_dir);

            let config_path = config::config_path(&data_dir);
            let loaded = config::load(&config_path);
            let config_writable = loaded.is_ok();
            let mut config = match loaded {
                Ok(config) => config,
                Err(error) => {
                    eprintln!(
                        "warning: failed to read {}: {error}; using built-in defaults",
                        config_path.display()
                    );
                    config::AppConfig::default()
                }
            };
            let resolved = config::resolve_active(&config);

            let requested_database = config
                .active_workspace_database
                .clone()
                .unwrap_or_else(|| config::DEFAULT_DATABASE.to_string());
            let (active_database, db_path) =
                match config::resolve_database_path(&data_dir, &requested_database) {
                    Ok(path) if path.exists() || requested_database == config::DEFAULT_DATABASE => {
                        (requested_database, path)
                    }
                    _ => (
                        config::DEFAULT_DATABASE.to_string(),
                        data_dir.join(config::DEFAULT_DATABASE),
                    ),
                };

            let client = Arc::new(tauri::async_runtime::block_on(LocalClient::open(&db_path))?);
            let session = tauri::async_runtime::block_on(client.session())?;

            // Worktrees whose folders disappeared are reported as missing, and
            // each registered project drops stale git worktree entries.
            for project in &config.projects {
                config::prune_project_worktrees(project);
            }
            if let Err(error) = tauri::async_runtime::block_on(client.reconcile_worktrees()) {
                eprintln!("warning: failed to reconcile worktrees: {error}");
            }

            // Keep the switcher registry in sync with the database in use and
            // drop entries whose files no longer exist.
            config.workspaces.retain(|entry| {
                entry.database == active_database
                    || config::resolve_database_path(&data_dir, &entry.database)
                        .map(|path| path.exists())
                        .unwrap_or(false)
            });
            match config
                .workspaces
                .iter_mut()
                .find(|entry| entry.database == active_database)
            {
                Some(entry) => entry.name = session.workspace.name.clone(),
                None => config.workspaces.push(config::WorkspaceEntry {
                    database: active_database.clone(),
                    name: session.workspace.name.clone(),
                }),
            }
            config.active_workspace_database = Some(active_database);
            if config_writable {
                config::save(&config_path, &config)?;
            }

            // Local mode spawns the forwarding task, so subscribe must run
            // inside the runtime context.
            let mut events = tauri::async_runtime::block_on(async { client.subscribe() });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                while let Some(event) = events.recv().await {
                    let _ = handle.emit("cowbot-event", &event);
                }
            });

            let tools = AppState::build_tools(&data_dir, config_path.clone(), client.clone());
            app.manage(AppState {
                local_client: Some(client.clone()),
                client: client as Arc<dyn Client>,
                tools,
                model: RwLock::new(ModelRuntime {
                    llm: config::build_provider(&resolved),
                    model: resolved.model,
                    remote: matches!(resolved.kind, config::ProviderKind::Remote(_)),
                }),
                config_path,
                data_dir,
                running: Arc::new(Mutex::new(HashSet::new())),
                replies: Arc::new(Mutex::new(HashSet::new())),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_session,
            commands::update_user_profile,
            commands::list_channels,
            commands::ensure_agent_channels,
            commands::create_channel,
            commands::rename_channel,
            commands::delete_channel,
            commands::set_channel_pinned,
            commands::clear_channel_messages,
            commands::list_messages,
            commands::post_message,
            commands::create_conversation,
            commands::list_conversation_summaries,
            commands::list_conversation_participants,
            commands::list_conversation_turns,
            commands::get_conversation_context,
            commands::get_conversation_context_preview,
            commands::get_task_run_context_preview,
            commands::list_run_prompt_snapshots,
            commands::list_recent_prompt_snapshots,
            commands::get_llm_prompt_snapshot,
            commands::get_prompt_snapshot_settings,
            commands::update_prompt_snapshot_settings,
            commands::purge_prompt_snapshots,
            commands::update_conversation_settings,
            commands::compact_conversation,
            commands::clear_conversation_context,
            commands::get_memory_notes,
            commands::save_memory_note,
            commands::delete_memory_note,
            commands::get_memory_settings,
            commands::update_memory_settings,
            commands::list_memory_entries,
            commands::add_memory_entry,
            commands::update_memory_entry,
            commands::approve_memory_entry,
            commands::dismiss_memory_entry,
            commands::archive_memory_entry,
            commands::delete_memory_entry,
            commands::preview_memory_recall,
            commands::list_agents,
            commands::update_agent,
            commands::list_available_tools,
            commands::list_templates,
            commands::instantiate_agents,
            commands::create_task,
            commands::run_task,
            commands::list_tasks,
            commands::list_run_tasks,
            commands::get_task_worktree,
            commands::task_worktree_diff,
            commands::discard_task_worktree,
            commands::list_task_overview,
            commands::cancel_task,
            commands::cancel_turn,
            commands::archive_task,
            commands::unarchive_task,
            commands::approve_task_plan,
            commands::list_assignments,
            commands::list_assignment_events,
            commands::list_runs,
            commands::list_tool_calls,
            commands::pending_approvals,
            commands::decide_approval,
            commands::session_decisions,
            commands::clear_session_decisions,
            commands::list_permission_rules,
            commands::create_permission_rule,
            commands::set_permission_rule_enabled,
            commands::delete_permission_rule,
            commands::get_workspace_policy,
            commands::set_bypass_critical,
            commands::list_audit_events,
            commands::get_model_settings,
            commands::save_model_provider,
            commands::delete_model_provider,
            commands::activate_model_provider,
            commands::set_active_model,
            commands::test_model_provider,
            commands::fetch_provider_models,
            commands::get_onboarding_state,
            commands::complete_onboarding,
            commands::get_app_info,
            commands::set_auto_check_updates,
            commands::restart_app,
            commands::list_workspaces,
            commands::switch_workspace,
            commands::create_workspace,
            commands::list_projects,
            commands::add_project,
            commands::remove_project,
            commands::list_project_branches,
            commands::export_workspace,
            commands::import_workspace,
            commands::list_commands,
            commands::parse_command,
            commands::ask_side_question,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CowBot");
}
