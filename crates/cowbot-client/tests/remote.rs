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
async fn an_owner_invites_someone_who_redeems_their_way_into_the_workspace() {
    let server = start_server().await;
    let owner = client(&server).await;

    // The owner is the only member, and knows what it may hand out.
    let session = owner.session().await.unwrap();
    assert_eq!(session.role, cowbot_core::identity::Role::Owner);
    assert_eq!(
        session.grantable_roles,
        vec![
            cowbot_core::identity::Role::Owner,
            cowbot_core::identity::Role::Admin,
            cowbot_core::identity::Role::Member,
        ]
    );
    assert_eq!(owner.list_members().await.unwrap().len(), 1);

    let grant = owner
        .create_invite(cowbot_core::identity::Role::Member)
        .await
        .unwrap();
    assert_eq!(grant.role, cowbot_core::identity::Role::Member);
    assert!(!grant.token.is_empty());

    // A client that has never signed in: no account, no session, just the
    // token the owner handed over. This is the whole point of the route.
    let newcomer = RemoteClient::redeem(
        &server.base,
        &grant.token,
        "Newcomer",
        "newcomer@example.com",
        "newcomer-long-password",
    )
    .await
    .expect("redeem");
    let newcomer_session = newcomer.session().await.unwrap();
    assert_eq!(newcomer_session.user.email, "newcomer@example.com");
    assert_eq!(newcomer_session.workspace.id.0, server.workspace_id);
    assert_eq!(newcomer_session.role, cowbot_core::identity::Role::Member);
    assert!(
        newcomer_session.grantable_roles.is_empty(),
        "a member hands out nothing"
    );

    // The account is real: it can read the workspace, which needs membership.
    assert!(newcomer.list_channels().await.is_ok());
    assert_eq!(owner.list_members().await.unwrap().len(), 2);

    // The invitation is spent, so it cannot mint a second account.
    let error = RemoteClient::redeem(
        &server.base,
        &grant.token,
        "Second",
        "second@example.com",
        "second-long-password",
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not valid"), "got {error}");
}

#[tokio::test]
async fn a_member_cannot_invite_and_the_owner_can_promote_then_remove_them() {
    let server = start_server().await;
    let owner = client(&server).await;
    let grant = owner
        .create_invite(cowbot_core::identity::Role::Member)
        .await
        .unwrap();
    let newcomer = RemoteClient::redeem(
        &server.base,
        &grant.token,
        "Newcomer",
        "newcomer@example.com",
        "newcomer-long-password",
    )
    .await
    .expect("redeem");
    let newcomer_id = newcomer.session().await.unwrap().user.id;

    // A plain member may not invite: the policy is the server's, and the
    // client surfaces its refusal rather than pre-empting it.
    let error = newcomer
        .create_invite(cowbot_core::identity::Role::Member)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("inviting"), "got {error}");

    // The owner promotes them, and the promoted client sees the new role on its
    // next read rather than keeping the role it signed in with.
    let promoted = owner
        .set_member_role(&newcomer_id, cowbot_core::identity::Role::Admin)
        .await
        .unwrap();
    assert_eq!(promoted.role, cowbot_core::identity::Role::Admin);
    let listed = owner.list_members().await.unwrap();
    let member = listed
        .iter()
        .find(|member| member.user_id == newcomer_id)
        .expect("the newcomer is listed");
    assert_eq!(member.role, cowbot_core::identity::Role::Admin);

    // Removing them keeps their account but ends their access to the
    // workspace: the next read is a 404, not a 403, so they cannot probe it.
    owner.remove_member(&newcomer_id).await.unwrap();
    assert_eq!(owner.list_members().await.unwrap().len(), 1);
    let error = newcomer.list_channels().await.unwrap_err();
    assert!(
        error.to_string().contains("not found") || error.to_string().contains("not available"),
        "got {error}"
    );
}

#[tokio::test]
async fn a_revoked_invitation_cannot_be_redeemed() {
    let server = start_server().await;
    let owner = client(&server).await;
    let grant = owner
        .create_invite(cowbot_core::identity::Role::Member)
        .await
        .unwrap();
    assert_eq!(owner.list_invites().await.unwrap().len(), 1);

    owner.revoke_invite(&grant.id).await.unwrap();
    assert!(owner.list_invites().await.unwrap().is_empty());

    let error = RemoteClient::redeem(
        &server.base,
        &grant.token,
        "Late",
        "late@example.com",
        "late-long-password",
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("not valid"), "got {error}");
}

#[tokio::test]
async fn a_signed_in_account_can_accept_an_invitation_to_a_second_workspace() {
    let server = start_server().await;
    let owner = client(&server).await;

    // A second account that already exists and already has a workspace of its
    // own: this is the path for someone the desktop knows, which redeem is not.
    let other_workspace = cowbot_core::identity::Workspace::new("Second Team");
    server
        .store
        .create_workspace(&other_workspace)
        .await
        .unwrap();
    let other = cowbot_core::identity::User::new("Other", "other@example.com", "");
    server.store.create_user(&other).await.unwrap();
    server
        .store
        .add_member(&cowbot_core::identity::Membership::new(
            other.id.clone(),
            other_workspace.id.clone(),
            cowbot_core::identity::Role::Owner,
        ))
        .await
        .unwrap();
    let issued = server.state.auth.issue(&other.id).await.unwrap();
    let other_client = RemoteClient::resume(&server.base, &issued.token, None)
        .await
        .expect("resume");

    let grant = owner
        .create_invite(cowbot_core::identity::Role::Member)
        .await
        .unwrap();
    let joined = other_client.accept_invite(&grant.token).await.unwrap();
    assert_eq!(joined.id, server.workspace_id);
    assert_eq!(joined.role, cowbot_core::identity::Role::Member);

    // The account now sees both workspaces.
    let workspaces = other_client.account_workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 2);
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

/// A stub that answers `/health` with the version it was given and everything
/// else with a fixed response.
///
/// The real server only ever reports its own version, so the mismatch path
/// cannot be reached against it; a stub is the only way to prove the check
/// happens before the first authenticated request.
struct Stub {
    base: String,
    /// Counts of `/health` hits and fallback hits, read once the stub stops.
    hits: tokio::task::JoinHandle<(usize, usize)>,
    stop: tokio::sync::oneshot::Sender<()>,
}

impl Stub {
    /// Stop the stub and read how many requests each route saw.
    async fn finish(self) -> (usize, usize) {
        let _ = self.stop.send(());
        self.hits.await.expect("the stub should not panic")
    }
}

async fn stub_server(api_version: u32) -> Stub {
    use axum::routing::get;
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let health_hits = std::sync::Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let health_counter = health_hits.clone();
    let app = Router::new()
        .route(
            "/health",
            get(move || {
                let health_counter = health_counter.clone();
                async move {
                    health_counter.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({ "status": "ok", "api_version": api_version }))
                }
            }),
        )
        .fallback(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::OK
            }
        });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = stop_rx.await;
            })
            .await;
        (
            health_hits.load(Ordering::SeqCst),
            hits.load(Ordering::SeqCst),
        )
    });
    Stub {
        base: format!("http://{address}"),
        hits: server,
        stop,
    }
}

#[tokio::test]
async fn a_server_speaking_another_api_version_is_refused_before_signing_in() {
    // The stub's fallback would answer the sign-in request if it were sent, so
    // the fallback count proves the version check stopped it.
    let stub = stub_server(99).await;
    let base = stub.base.clone();
    let error = RemoteClient::sign_in(&base, "owner@example.com", PASSWORD, None)
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("version 99"), "got {message}");
    assert!(
        message.contains("speaks server API version 1"),
        "the message should name the version this build speaks: {message}"
    );
    let (health_hits, other_hits) = stub.finish().await;
    assert_eq!(health_hits, 1, "the version check should run once");
    assert_eq!(other_hits, 0, "no other request may reach the server");
}

#[tokio::test]
async fn a_matching_api_version_lets_the_first_request_through() {
    let stub = stub_server(1).await;
    let base = stub.base.clone();
    // The stub has no sign-in route, so the request fails; what matters is
    // that it was attempted rather than blocked by the version check.
    let error = RemoteClient::sign_in(&base, "owner@example.com", PASSWORD, None)
        .await
        .unwrap_err();
    let (health_hits, other_hits) = stub.finish().await;
    assert!(health_hits >= 1, "the client should have checked /health");
    assert_eq!(other_hits, 1, "the sign-in request should have been sent");
    assert!(
        error.to_string().contains("unexpected sign-in response"),
        "got {error}"
    );
}

#[tokio::test]
async fn resuming_a_session_checks_the_api_version_too() {
    let stub = stub_server(7).await;
    let base = stub.base.clone();
    let error = RemoteClient::resume(&base, "some-token", None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("version 7"), "got {error}");
    let (_, other_hits) = stub.finish().await;
    assert_eq!(other_hits, 0, "the session request must not be sent");
}

/// A stub whose channel route fails a fixed number of times before answering.
///
/// Counts keep `/health`, `/me`, and sign-in answering immediately so the test
/// reaches the route it is about; only the read under test is disturbed.
struct FlakyStub {
    base: String,
    /// Channel reads and channel writes that reached the stub, in that order.
    hits: tokio::task::JoinHandle<(usize, usize)>,
    stop: tokio::sync::oneshot::Sender<()>,
}

impl FlakyStub {
    async fn finish(self) -> (usize, usize) {
        let _ = self.stop.send(());
        self.hits.await.expect("the stub should not panic")
    }
}

/// A stub for exercising read retries.
///
/// `failures` channel reads answer `503` before the stub answers `200` with an
/// empty list, and every write records how often it was called so a test can
/// prove writes are not replayed.
async fn flaky_stub(failures: usize) -> FlakyStub {
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let channel_hits = std::sync::Arc::new(AtomicUsize::new(0));
    let write_hits = std::sync::Arc::new(AtomicUsize::new(0));
    let channels = channel_hits.clone();
    let channel_writes = write_hits.clone();
    let app = Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({ "status": "ok", "api_version": 1 })) }),
        )
        .route(
            "/api/v1/auth/sign-in",
            post(|| async {
                Json(serde_json::json!({
                    "token": "stub-token",
                    "expires_at": "2099-01-01T00:00:00Z",
                    "user": { "id": "user-1", "name": "Owner", "email": "owner@example.com" },
                    "workspaces": [{
                        "id": "ws-1", "name": "Test", "created_at": "2026-01-01T00:00:00Z",
                        "role": "owner",
                        "grantable_roles": ["owner", "admin", "member"]
                    }]
                }))
            }),
        )
        .route(
            "/api/v1/me",
            get(|| async {
                Json(serde_json::json!({
                    "session_expires_at": "2099-01-01T00:00:00Z",
                    "user": { "id": "user-1", "name": "Owner", "email": "owner@example.com" },
                    "workspaces": [{
                        "id": "ws-1", "name": "Test", "created_at": "2026-01-01T00:00:00Z",
                        "role": "owner",
                        "grantable_roles": ["owner", "admin", "member"]
                    }]
                }))
            }),
        )
        .route(
            "/api/v1/workspaces/{id}/channels",
            get(move || {
                let channels = channels.clone();
                async move {
                    let attempt = channels.fetch_add(1, Ordering::SeqCst) + 1;
                    if attempt <= failures {
                        (
                            axum::http::StatusCode::SERVICE_UNAVAILABLE,
                            Json(serde_json::json!({ "code": "unavailable", "message": "try later" })),
                        )
                    } else {
                        (
                            axum::http::StatusCode::OK,
                            Json(serde_json::json!([])),
                        )
                    }
                }
            })
            .post(move || {
                let channel_writes = channel_writes.clone();
                async move {
                    channel_writes.fetch_add(1, Ordering::SeqCst);
                    (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        Json(serde_json::json!({ "code": "unavailable", "message": "try later" })),
                    )
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let reader_counter = channel_hits.clone();
    let writer_counter = write_hits.clone();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = stop_rx.await;
            })
            .await;
        (
            reader_counter.load(Ordering::SeqCst),
            writer_counter.load(Ordering::SeqCst),
        )
    });
    FlakyStub {
        base: format!("http://{address}"),
        hits: server,
        stop,
    }
}

#[tokio::test]
async fn a_read_that_fails_transiently_is_tried_again() {
    let stub = flaky_stub(2).await;
    let base = stub.base.clone();
    let client = RemoteClient::sign_in(&base, "owner@example.com", "password", Some("ws-1"))
        .await
        .expect("sign in");
    let channels = client
        .list_channels()
        .await
        .expect("the third read answers");
    assert!(channels.is_empty());
    let (reads, _) = stub.finish().await;
    assert_eq!(reads, 3, "three attempts should reach the stub");
}

#[tokio::test]
async fn a_read_that_keeps_failing_reports_the_error() {
    // More failures than attempts, so the client gives up and reports it.
    let stub = flaky_stub(5).await;
    let base = stub.base.clone();
    let client = RemoteClient::sign_in(&base, "owner@example.com", "password", Some("ws-1"))
        .await
        .expect("sign in");
    let error = client.list_channels().await.unwrap_err();
    assert!(
        error.to_string().contains("try later"),
        "the server's message should survive: {error}"
    );
    let (reads, _) = stub.finish().await;
    assert_eq!(reads, 3, "retries stop after three attempts");
}

#[tokio::test]
async fn a_write_is_never_replayed() {
    let stub = flaky_stub(0).await;
    let base = stub.base.clone();
    let client = RemoteClient::sign_in(&base, "owner@example.com", "password", Some("ws-1"))
        .await
        .expect("sign in");
    // Creating a channel is a POST; the stub fails it and the client must stop
    // there rather than risk a second create.
    let error = client.create_channel("ops", "").await.unwrap_err();
    assert!(error.to_string().contains("try later"), "got {error}");
    let (_, writes) = stub.finish().await;
    assert_eq!(writes, 1, "a write must reach the server exactly once");
}
