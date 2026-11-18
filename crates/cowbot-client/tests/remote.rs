//! The remote client against a real server over a socket.
//!
//! These tests start `cowbot-server` on an ephemeral port with an in-memory
//! database, then drive it through `RemoteClient`, so the HTTP shapes, the
//! error mapping, and the role enforcement are exercised together.

use std::sync::Arc;

use cowbot_bus::EventBus;
use cowbot_client::{Client, RemoteClient};
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::channel::ChannelId;
use cowbot_core::identity::WorkspaceId;
use cowbot_server::auth::AuthService;
use cowbot_server::config::ServerConfig;
use cowbot_server::AppState;
use cowbot_store::Store;

const PASSWORD: &str = "remote-client-password";

struct Server {
    base: String,
    store: Store,
    state: AppState,
    workspace_id: String,
    agent_id: AgentId,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Server {
    /// Stop serving without giving up the database or the bus.
    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }

    /// Serve the same state again on the same address.
    ///
    /// The replacement must answer on the same port, because that is the only
    /// address the already-subscribed client knows. A just-closed listener can
    /// hold the port for a moment on Windows, so the bind is retried briefly.
    async fn start_again(&mut self) {
        let address = self.base_address();
        let mut listener = None;
        for _ in 0..100 {
            match tokio::net::TcpListener::bind(&address).await {
                Ok(bound) => {
                    listener = Some(bound);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            }
        }
        let listener = listener.expect("rebind");
        let state = self.state.clone();
        let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        self.shutdown = Some(shutdown);
        tokio::spawn(async move {
            let _ = cowbot_server::router::serve_with_shutdown(listener, state, async move {
                let _ = shutdown_rx.await;
            })
            .await;
        });
        // Give the replacement a moment to be listening.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    fn base_address(&self) -> String {
        self.base
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .to_string()
    }
}

async fn start_server() -> Server {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let config = ServerConfig {
        owner_email: "owner@example.com".into(),
        ..ServerConfig::default()
    };
    let auth = Arc::new(AuthService::new(store.clone(), config));
    auth.bootstrap(PASSWORD).await.unwrap();
    store.list_users().await.unwrap();
    let workspace = store.list_workspaces().await.unwrap().remove(0);
    let agent = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
    store.upsert_agent(&workspace.id, &agent).await.unwrap();

    let state = AppState::new(store.clone(), auth, EventBus::default_bus());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let served = state.clone();
    tokio::spawn(async move {
        let _ = cowbot_server::router::serve_with_shutdown(listener, served, async move {
            let _ = shutdown_rx.await;
        })
        .await;
    });

    Server {
        base: format!("http://{address}"),
        store,
        state,
        workspace_id: workspace.id.0,
        agent_id: agent.id,
        shutdown: Some(shutdown),
    }
}

async fn client(server: &Server) -> RemoteClient {
    RemoteClient::sign_in(
        &server.base,
        "owner@example.com",
        PASSWORD,
        Some(&server.workspace_id),
    )
    .await
    .expect("sign in")
}

#[tokio::test]
async fn a_remote_client_signs_in_and_reads_the_session() {
    let server = start_server().await;
    let client = client(&server).await;
    let session = client.session().await.unwrap();
    assert_eq!(session.user.email, "owner@example.com");
    assert_eq!(session.workspace.id.0, server.workspace_id);
}

#[tokio::test]
async fn a_wrong_password_is_a_clear_config_error() {
    let server = start_server().await;
    let error = RemoteClient::sign_in(
        &server.base,
        "owner@example.com",
        "not-the-password",
        Some(&server.workspace_id),
    )
    .await
    .unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("not correct") || message.contains("not found"),
        "got {message}"
    );
}

#[tokio::test]
async fn channels_and_messages_round_trip_over_http() {
    let server = start_server().await;
    let client = client(&server).await;

    let channel = client.create_channel("design", "UI work").await.unwrap();
    let channels = client.list_channels().await.unwrap();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].id, channel.id);

    let posted = client.post_message(&channel.id, "hello").await.unwrap();
    assert_eq!(posted.content, "hello");
    let messages = client.list_messages(&channel.id, 50).await.unwrap();
    assert_eq!(messages.len(), 1);

    let renamed = client
        .rename_channel(&channel.id, "design-2", "UI work")
        .await
        .unwrap();
    assert_eq!(renamed.name, "design-2");

    let pinned = client.set_channel_pinned(&channel.id, true).await.unwrap();
    assert!(pinned.pinned);
}

#[tokio::test]
async fn an_unknown_channel_reads_as_none_instead_of_erroring() {
    let server = start_server().await;
    let client = client(&server).await;
    assert!(client
        .get_channel(&ChannelId::new("nope"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn tasks_flow_through_create_and_list() {
    let server = start_server().await;
    let client = client(&server).await;
    let task = client
        .create_task(&cowbot_client::NewTask::new(
            "Write the parser",
            "A parser",
            server.agent_id.clone(),
        ))
        .await
        .unwrap();
    let tasks = client.list_tasks().await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, task.id);
    let fetched = client.get_task(&task.id).await.unwrap().expect("the task");
    assert_eq!(fetched.id, task.id);
    assert!(client
        .get_task(&cowbot_core::task::TaskId::new("nope"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn the_agent_route_lists_workspace_agents() {
    let server = start_server().await;
    let client = client(&server).await;
    let agents = client.list_agents().await.unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].id, server.agent_id);
}

#[tokio::test]
async fn a_local_only_operation_reports_server_mode_clearly() {
    let server = start_server().await;
    let client = client(&server).await;
    let error = client
        .request_task_worktree(
            &cowbot_core::task::TaskId::new("t"),
            "p",
            "/tmp/repo",
            "main",
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("server mode"), "got {error}");
    let error = client
        .cancel_turn(&cowbot_core::conversation::TurnId::random())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("server mode"), "got {error}");
    let _ = WorkspaceId::new(server.workspace_id);
    let _ = &server.store;
}

#[tokio::test]
async fn a_stored_token_resumes_the_session_without_a_password() {
    let server = start_server().await;
    let client = client(&server).await;
    let token = client.token().to_string();

    let resumed = RemoteClient::resume(&server.base, &token, Some(client.workspace_id()))
        .await
        .expect("resume");
    let session = resumed.session().await.unwrap();
    assert_eq!(session.user.email, "owner@example.com");
    assert_eq!(session.workspace.id.0, server.workspace_id);
    assert_eq!(resumed.base_url(), server.base);
}

#[tokio::test]
async fn a_token_the_server_forgot_resumes_as_a_sign_in_error() {
    let server = start_server().await;
    let error = RemoteClient::resume(&server.base, "not-a-real-token", Some(&server.workspace_id))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("sign in again"), "got {error}");
}

#[tokio::test]
async fn the_account_workspace_list_comes_from_the_live_session() {
    let server = start_server().await;
    let client = client(&server).await;
    let workspaces = client.account_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, server.workspace_id);
}

#[tokio::test]
async fn the_event_stream_keeps_trying_until_the_server_returns() {
    let mut server = start_server().await;
    let client = client(&server).await;

    // Subscribe while nothing is listening: the first connect fails, and the
    // client must retry until the server comes back rather than giving up.
    server.stop();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut events = client.subscribe();

    server.start_again().await;

    client.create_channel("after the outage", "").await.unwrap();
    let delivered = recv_until_within(&mut events, "ChannelCreated", 45).await;
    assert!(
        delivered.is_some(),
        "events should flow once the server is reachable again"
    );
}

/// One event of the named payload type, or `None` after a bounded wait.
async fn recv_until_within(
    events: &mut cowbot_client::EventStream,
    wanted: &str,
    seconds: u64,
) -> Option<()> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let Ok(Some(event)) = tokio::time::timeout(remaining, events.recv()).await else {
            return None;
        };
        let name = serde_json::to_value(&event.payload).ok()?;
        if name.get("type").and_then(|value| value.as_str()) == Some(wanted) {
            return Some(());
        }
    }
}
