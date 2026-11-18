mod commands;
mod config;
mod migration;
mod startup;
mod workspace_tools;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use cowbot_client::{Client, LocalClient, RemoteClient};
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
    /// The server client when this device opened a server workspace
    /// (ADR-0053), so the settings view can read the account's workspaces and
    /// report what changed.
    pub server_client: Option<Arc<RemoteClient>>,
    /// Where this device's data comes from.
    pub backend: config::BackendMode,
    pub tools: ToolRegistry,
    /// Active model provider; replaced when the user switches providers.
    pub model: RwLock<ModelRuntime>,
    /// Path of the local `config.json` holding model providers.
    pub config_path: PathBuf,
    /// Directory holding workspace databases, `config.json`, and `cowbot.db`.
    pub data_dir: PathBuf,
    /// Folder chosen for workspace databases; the app data directory unless the
    /// first-run guide picked another one.
    pub database_dir: PathBuf,
    /// Absolute path of the open workspace database.
    pub database_path: PathBuf,
    /// Workspace database name relative to `database_dir`.
    pub active_database: String,
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
    let builder = tauri::Builder::default();

    // Keep one CowBot per machine: a second launch focuses the open window
    // instead of opening a copy over the same workspace database.
    //
    // Only a release build claims that lock. The plugin derives its lock name
    // from the bundle identifier alone, and a development build keeps its
    // workspace under `app_data_dir()/dev` instead of the installed app's data
    // directory. Sharing the lock would make the installed app exit and focus
    // the development window whenever `tauri dev` is running, which looks like
    // the app opening with an empty workspace.
    #[cfg(not(debug_assertions))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }));

    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::get_startup_status,
            commands::resolve_startup,
            commands::quit_app,
            commands::set_database_directory,
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
            commands::get_ui_locale,
            commands::set_ui_locale,
            commands::get_show_agent_bot_tags,
            commands::set_show_agent_bot_tags,
            commands::restart_app,
            commands::list_workspaces,
            commands::switch_workspace,
            commands::create_workspace,
            commands::get_server_settings,
            commands::probe_server,
            commands::connect_server,
            commands::list_server_workspaces,
            commands::switch_server_workspace,
            commands::use_local_backend,
            commands::disconnect_server,
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
        .build(tauri::generate_context!())
        .expect("error while building CowBot");

    // Resolve paths and inspect the database before the event loop starts. The
    // window then shows a startup screen that asks before a one-way upgrade;
    // `resolve_startup` finishes initialization once the user decides.
    initialize(&app);

    app.run(|_, _| {});
}

/// Resolve the paths startup needs and decide whether the database can open.
fn initialize(app: &tauri::App) {
    match prepare(app) {
        Ok(state) => {
            let ready = state.status().is_ready();
            app.manage(state);
            if ready {
                if let Err(error) = tauri::async_runtime::block_on(finish(app.handle())) {
                    mark_failed(app.handle(), &error);
                }
            }
        }
        Err(error) => {
            app.manage(startup::StartupState::failed(error));
        }
    }
}

/// Resolve the data directory, configuration, and workspace database path.
fn prepare(app: &tauri::App) -> Result<startup::StartupState, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let data_dir = config::resolve_data_dir(&app_data_dir);
    std::fs::create_dir_all(&data_dir)
        .map_err(|error| format!("failed to create {}: {error}", data_dir.display()))?;
    if data_dir == app_data_dir {
        migration::migrate_legacy_database(&data_dir);
    }

    let config_path = config::config_path(&data_dir);
    let config = match config::load(&config_path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!(
                "warning: failed to read {}: {error}; using built-in defaults",
                config_path.display()
            );
            config::AppConfig::default()
        }
    };

    let database_dir = config::resolve_database_dir(&data_dir, &config);
    std::fs::create_dir_all(&database_dir)
        .map_err(|error| format!("failed to create {}: {error}", database_dir.display()))?;

    let requested_database = config
        .active_workspace_database
        .clone()
        .unwrap_or_else(|| config::DEFAULT_DATABASE.to_string());
    let (database, db_path) =
        match config::resolve_database_path(&database_dir, &requested_database) {
            Ok(path) if path.exists() || requested_database == config::DEFAULT_DATABASE => {
                (requested_database, path)
            }
            _ => (
                config::DEFAULT_DATABASE.to_string(),
                database_dir.join(config::DEFAULT_DATABASE),
            ),
        };

    let status = tauri::async_runtime::block_on(startup::inspect(&db_path, &database_dir));
    Ok(startup::StartupState::new(
        data_dir,
        database_dir,
        database,
        db_path,
        config_path,
        status,
    ))
}

/// Open the client for this device's backend and install the application
/// state.
///
/// Idempotent: a second call after the state is installed does nothing.
/// A local workspace opens its database here; a server workspace resumes its
/// saved session. The startup screen inspects a database before this runs, and
/// in server mode there is no local database to inspect, so a failure to reach
/// the server is reported on the startup screen rather than guessed at.
pub(crate) async fn finish(app: &tauri::AppHandle) -> Result<(), String> {
    if app.try_state::<AppState>().is_some() {
        return Ok(());
    }

    let (data_dir, database_dir, database, db_path, config_path) = {
        let state = app.state::<startup::StartupState>();
        (
            state.data_dir.clone(),
            state.database_dir.clone(),
            state.database.clone(),
            state.db_path.clone(),
            state.config_path.clone(),
        )
    };

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

    // Worktrees belong to the local runtime, so they are reconciled before the
    // backend is chosen; a server workspace leaves this machine's checkouts
    // exactly as they are.
    for project in &config.projects {
        config::prune_project_worktrees(project);
    }

    if let Some(connection) = config.active_server().cloned() {
        let resume = RemoteClient::resume(
            &connection.base_url,
            &connection.token,
            Some(&connection.workspace_id),
        )
        .await
        .map_err(|error| format!("the server workspace could not be opened: {error}"))?;
        let client = Arc::new(resume);
        let session = client.session().await.map_err(|error| error.to_string())?;
        // The saved session may have been renamed on the server.
        if let Some(saved) = config.server.as_mut() {
            saved.workspace_name = session.workspace.name.clone();
            saved.email = session.user.email.clone();
        }
        if config_writable {
            config::save(&config_path, &config).map_err(|error| error.to_string())?;
        }

        let tools = AppState::rooted_tools(
            &data_dir,
            config_path.clone(),
            client.clone() as Arc<dyn Client>,
        );
        install_events(app, client.clone() as Arc<dyn Client>);
        app.manage(AppState {
            client: client.clone() as Arc<dyn Client>,
            local_client: None,
            server_client: Some(client),
            backend: config::BackendMode::Server,
            tools,
            model: RwLock::new(ModelRuntime {
                llm: config::build_provider(&resolved),
                model: resolved.model,
                remote: matches!(resolved.kind, config::ProviderKind::Remote(_)),
            }),
            config_path,
            data_dir,
            database_dir,
            database_path: db_path,
            active_database: database,
            running: Arc::new(Mutex::new(HashSet::new())),
            replies: Arc::new(Mutex::new(HashSet::new())),
        });
        return Ok(());
    }

    let client = LocalClient::open(&db_path)
        .await
        .map_err(|error| error.to_string())?;
    let client = Arc::new(client);
    let session = client.session().await.map_err(|error| error.to_string())?;

    // A fresh workspace starts with the built-in primary assistant so the user
    // can talk to it without visiting Settings → Team first. A workspace that
    // already has agents is left exactly as it is.
    if let Err(error) = client.ensure_main_agent().await {
        eprintln!("warning: failed to create the default main agent: {error}");
    }

    // Worktrees whose folders disappeared are reported as missing, and each
    // registered project drops stale git worktree entries.
    if let Err(error) = client.reconcile_worktrees().await {
        eprintln!("warning: failed to reconcile worktrees: {error}");
    }

    // Keep the switcher registry in sync with the database in use and drop
    // entries whose files no longer exist.
    config.workspaces.retain(|entry| {
        entry.database == database
            || config::resolve_database_path(&database_dir, &entry.database)
                .map(|path| path.exists())
                .unwrap_or(false)
    });
    match config
        .workspaces
        .iter_mut()
        .find(|entry| entry.database == database)
    {
        Some(entry) => entry.name = session.workspace.name.clone(),
        None => config.workspaces.push(config::WorkspaceEntry {
            database: database.clone(),
            name: session.workspace.name.clone(),
        }),
    }
    config.active_workspace_database = Some(database.clone());
    if config_writable {
        config::save(&config_path, &config).map_err(|error| error.to_string())?;
    }

    let tools = AppState::rooted_tools(&data_dir, config_path.clone(), client.clone());

    install_events(app, client.clone() as Arc<dyn Client>);
    app.manage(AppState {
        client: client.clone() as Arc<dyn Client>,
        local_client: Some(client.clone()),
        server_client: None,
        backend: config::BackendMode::Local,
        tools,
        model: RwLock::new(ModelRuntime {
            llm: config::build_provider(&resolved),
            model: resolved.model,
            remote: matches!(resolved.kind, config::ProviderKind::Remote(_)),
        }),
        config_path,
        data_dir,
        database_dir,
        database_path: db_path,
        active_database: database,
        running: Arc::new(Mutex::new(HashSet::new())),
        replies: Arc::new(Mutex::new(HashSet::new())),
    });
    Ok(())
}

/// Forward a client's live events to the window.
///
/// Both transports spawn their own forwarding task in `subscribe`, so this
/// runs inside the runtime context and lasts as long as the client does.
fn install_events(app: &tauri::AppHandle, client: Arc<dyn Client>) {
    let mut events = client.subscribe();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            let _ = handle.emit("cowbot-event", &event);
        }
    });
}

/// Record a startup failure so the window can report it.
pub(crate) fn mark_failed(app: &tauri::AppHandle, error: &str) {
    let state = app.state::<startup::StartupState>();
    let status = state.failure(error.to_string());
    state.set_status(status);
}
