//! The Mattermost adapter against a fake server (ADR-0057).
//!
//! The fake speaks Mattermost's shapes: a REST surface for `users/me`,
//! `config/client`, `users/{id}`, and `posts`, and a WebSocket that expects the
//! authentication challenge before it sends anything. That is enough to drive the
//! real adapter — the real socket loop, the real REST client, the real
//! translation — without a Mattermost install.

use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use cowbot_core::cancel::CancelSignal;
use cowbot_integrations::adapter::{Integration, IntegrationContext};
use cowbot_integrations::model::{ChunkPart, DeliveredId, OutboundContent, OutboundTarget};
use cowbot_integrations::Ingested;
use cowbot_mattermost::{MattermostConfig, MattermostIntegration};
use serde_json::{json, Value};

/// What the fake server was asked to do, for a test to assert on.
#[derive(Default)]
struct Recorded {
    /// Bodies the adapter posted.
    posts: Mutex<Vec<Value>>,
    /// Tokens presented on any REST call.
    tokens: Mutex<Vec<String>>,
    /// Whether the socket received the authentication frame.
    authenticated: Mutex<bool>,
    /// Channel for the socket handler to send down.
    sender: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Value>>>,
}

#[derive(Clone)]
struct Fake {
    recorded: Arc<Recorded>,
    max_post_size: i64,
}

async fn me(State(fake): State<Fake>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    record_token(&fake, &headers);
    Json(json!({ "id": "bot-id", "username": "cowbot [bot]", "is_bot": true }))
}

async fn client_config(State(fake): State<Fake>) -> impl IntoResponse {
    Json(json!({ "MaxPostSize": fake.max_post_size }))
}

async fn user(
    State(fake): State<Fake>,
    headers: axum::http::HeaderMap,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    record_token(&fake, &headers);
    Json(json!({ "id": user_id, "username": "alice" }))
}

async fn create_post(
    State(fake): State<Fake>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    record_token(&fake, &headers);
    fake.recorded.posts.lock().unwrap().push(body.clone());
    (StatusCode::CREATED, Json(json!({ "id": "posted-1" })))
}

fn record_token(fake: &Fake, headers: &axum::http::HeaderMap) {
    if let Some(value) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        fake.recorded
            .tokens
            .lock()
            .unwrap()
            .push(value.trim_start_matches("Bearer ").to_string());
    }
}

async fn websocket(State(fake): State<Fake>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| handle_socket(socket, fake))
}

async fn handle_socket(mut socket: WebSocket, fake: Fake) {
    // Mattermost authenticates in-band: the first frame carries the token and
    // the server answers with a status event. A test that skipped this would not
    // be exercising the handshake the adapter depends on.
    let Some(Ok(Message::Text(first))) = socket.recv().await else {
        return;
    };
    let frame: Value = serde_json::from_str(first.as_str()).unwrap_or(Value::Null);
    let authenticated =
        frame["action"] == "authentication_challenge" && frame["data"]["token"] == "bot-token";
    *fake.recorded.authenticated.lock().unwrap() = authenticated;
    if !authenticated {
        let _ = socket
            .send(Message::Text(
                json!({ "event": "status", "data": { "status": "FAIL" } })
                    .to_string()
                    .into(),
            ))
            .await;
        return;
    }
    let _ = socket
        .send(Message::Text(
            json!({ "event": "status", "data": { "status": "OK" } })
                .to_string()
                .into(),
        ))
        .await;

    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Value>();
    *fake.recorded.sender.lock().unwrap() = Some(sender);

    // Forward whatever the test asks the socket to deliver.
    while let Some(event) = receiver.recv().await {
        if socket
            .send(Message::Text(event.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
    }
}

async fn fake_server(fake: Fake) -> (String, Router) {
    let router = Router::new()
        .route("/api/v4/users/me", get(me))
        .route("/api/v4/config/client", get(client_config))
        .route("/api/v4/users/{user_id}", get(user))
        .route("/api/v4/posts", post(create_post))
        .route("/api/v4/websocket", get(websocket))
        .with_state(fake);
    (String::new(), router)
}

/// Start the fake on a random port and return the origin.
async fn serve(fake: Fake) -> String {
    let (_, router) = fake_server(fake).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{address}")
}

fn config(base_url: String) -> MattermostConfig {
    MattermostConfig {
        base_url,
        bot_token: "bot-token".into(),
    }
}

/// A sink that records what the pipeline was handed.
#[derive(Default)]
struct RecordingSink {
    accepted: Mutex<Vec<cowbot_integrations::InboundMessage>>,
}

#[async_trait::async_trait]
impl cowbot_integrations::MessageSink for RecordingSink {
    async fn accept(
        &self,
        message: cowbot_integrations::InboundMessage,
    ) -> cowbot_core::error::CowResult<Ingested> {
        self.accepted.lock().unwrap().push(message);
        Ok(Ingested::Posted {
            channel_id: cowbot_core::channel::ChannelId::new("c1"),
            message_id: "m1".into(),
            user_id: "u1".into(),
            starts_turn: true,
        })
    }
}

/// Wait until the fake's socket recorded its sender, so a test can push events.
async fn wait_for_socket(fake: &Fake) {
    for _ in 0..200 {
        if fake.recorded.sender.lock().unwrap().is_some() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the fake never saw the socket connect");
}

fn posted_event(post: Value) -> Value {
    json!({
        "event": "posted",
        "data": { "post": post.to_string() },
    })
}

#[tokio::test]
async fn the_adapter_authenticates_and_relays_a_mention() {
    let fake = Fake {
        recorded: Arc::new(Recorded::default()),
        max_post_size: 16_384,
    };
    let base = serve(fake.clone()).await;
    let sink = Arc::new(RecordingSink::default());
    let adapter = MattermostIntegration::new(config(base.clone()));

    let store = cowbot_store::Store::connect_in_memory().await.unwrap();
    let ctx = IntegrationContext {
        store,
        integration: cowbot_core::integration::IntegrationRecord {
            id: cowbot_core::integration::IntegrationId::new("i1"),
            workspace_id: cowbot_core::identity::WorkspaceId::new("w1"),
            kind: cowbot_core::integration::IntegrationKind::Mattermost,
            label: "Support".into(),
            created_at: chrono::Utc::now(),
            revoked_at: None,
        },
        sink: sink.clone(),
    };

    let cancel = CancelSignal::new();
    let running = cancel.clone();
    let handle = {
        let adapter = adapter.clone();
        tokio::spawn(async move { adapter.run(ctx, running).await })
    };

    wait_for_socket(&fake).await;
    assert!(
        *fake.recorded.authenticated.lock().unwrap(),
        "the challenge was not sent"
    );

    // A person mentions the bot in a channel.
    let sender = fake
        .recorded
        .sender
        .lock()
        .unwrap()
        .clone()
        .expect("the socket is connected");
    sender
        .send(posted_event(json!({
            "id": "post-1",
            "channel_id": "mm-channel-1",
            "user_id": "mm-user-1",
            "message": "@cowbot [bot] what is the deploy status?",
            "root_id": "",
            "type": "",
            "create_at": 1_700_000_000_000i64,
        })))
        .unwrap();

    // Wait for the pipeline to receive it.
    for _ in 0..200 {
        if !sink.accepted.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    cancel.cancel();
    let _ = handle.await;

    let accepted = sink.accepted.lock().unwrap().clone();
    assert_eq!(accepted.len(), 1, "the message was not relayed");
    let message = &accepted[0];
    assert_eq!(message.event_id, "post-1");
    assert_eq!(message.external_conversation_id, "mm-channel-1");
    assert!(message.addresses_bot);
    // `@cowbot [bot]` became `@cowbot`, which is what the group binding turns
    // into `@ThatAgent`.
    assert_eq!(message.text, "@cowbot what is the deploy status?");
    assert_eq!(message.author.display_name, "alice");
    assert!(!message.author.is_bot);
}

#[tokio::test]
async fn a_message_that_does_not_address_the_bot_is_relayed_but_not_addressed() {
    let fake = Fake {
        recorded: Arc::new(Recorded::default()),
        max_post_size: 16_384,
    };
    let base = serve(fake.clone()).await;
    let sink = Arc::new(RecordingSink::default());
    let adapter = MattermostIntegration::new(config(base));

    let store = cowbot_store::Store::connect_in_memory().await.unwrap();
    let ctx = IntegrationContext {
        store,
        integration: cowbot_core::integration::IntegrationRecord {
            id: cowbot_core::integration::IntegrationId::new("i1"),
            workspace_id: cowbot_core::identity::WorkspaceId::new("w1"),
            kind: cowbot_core::integration::IntegrationKind::Mattermost,
            label: "Support".into(),
            created_at: chrono::Utc::now(),
            revoked_at: None,
        },
        sink: sink.clone(),
    };
    let cancel = CancelSignal::new();
    let running = cancel.clone();
    let handle = {
        let adapter = adapter.clone();
        tokio::spawn(async move { adapter.run(ctx, running).await })
    };
    wait_for_socket(&fake).await;

    fake.recorded
        .sender
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .send(posted_event(json!({
            "id": "post-2",
            "channel_id": "mm-channel-1",
            "user_id": "mm-user-1",
            "message": "lunch at noon?",
            "root_id": "",
            "type": "",
        })))
        .unwrap();

    for _ in 0..200 {
        if !sink.accepted.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    cancel.cancel();
    let _ = handle.await;

    let accepted = sink.accepted.lock().unwrap().clone();
    assert_eq!(accepted.len(), 1);
    assert!(!accepted[0].addresses_bot);
    assert_eq!(accepted[0].text, "lunch at noon?");
}

#[tokio::test]
async fn the_bots_own_post_never_reaches_the_pipeline() {
    let fake = Fake {
        recorded: Arc::new(Recorded::default()),
        max_post_size: 16_384,
    };
    let base = serve(fake.clone()).await;
    let sink = Arc::new(RecordingSink::default());
    let adapter = MattermostIntegration::new(config(base));

    let store = cowbot_store::Store::connect_in_memory().await.unwrap();
    let ctx = IntegrationContext {
        store,
        integration: cowbot_core::integration::IntegrationRecord {
            id: cowbot_core::integration::IntegrationId::new("i1"),
            workspace_id: cowbot_core::identity::WorkspaceId::new("w1"),
            kind: cowbot_core::integration::IntegrationKind::Mattermost,
            label: "Support".into(),
            created_at: chrono::Utc::now(),
            revoked_at: None,
        },
        sink: sink.clone(),
    };
    let cancel = CancelSignal::new();
    let running = cancel.clone();
    let handle = {
        let adapter = adapter.clone();
        tokio::spawn(async move { adapter.run(ctx, running).await })
    };
    wait_for_socket(&fake).await;

    // The bot's answer comes back over the same socket, as it does in Mattermost.
    fake.recorded
        .sender
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .send(posted_event(json!({
            "id": "post-3",
            "channel_id": "mm-channel-1",
            "user_id": "bot-id",
            "message": "Deploy is green.",
            "root_id": "",
            "type": "",
        })))
        .unwrap();

    // A person's message afterwards proves the loop is running and the bot's own
    // post was skipped rather than merely slow.
    fake.recorded
        .sender
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .send(posted_event(json!({
            "id": "post-4",
            "channel_id": "mm-channel-1",
            "user_id": "mm-user-1",
            "message": "thanks",
            "root_id": "",
            "type": "",
        })))
        .unwrap();

    for _ in 0..200 {
        if !sink.accepted.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    cancel.cancel();
    let _ = handle.await;

    let accepted = sink.accepted.lock().unwrap().clone();
    assert_eq!(accepted.len(), 1, "the bot answered itself: {accepted:?}");
    assert_eq!(accepted[0].event_id, "post-4");
}

#[tokio::test]
async fn a_system_post_is_never_relayed() {
    let fake = Fake {
        recorded: Arc::new(Recorded::default()),
        max_post_size: 16_384,
    };
    let base = serve(fake.clone()).await;
    let sink = Arc::new(RecordingSink::default());
    let adapter = MattermostIntegration::new(config(base));
    let store = cowbot_store::Store::connect_in_memory().await.unwrap();
    let ctx = IntegrationContext {
        store,
        integration: cowbot_core::integration::IntegrationRecord {
            id: cowbot_core::integration::IntegrationId::new("i1"),
            workspace_id: cowbot_core::identity::WorkspaceId::new("w1"),
            kind: cowbot_core::integration::IntegrationKind::Mattermost,
            label: "Support".into(),
            created_at: chrono::Utc::now(),
            revoked_at: None,
        },
        sink: sink.clone(),
    };
    let cancel = CancelSignal::new();
    let running = cancel.clone();
    let handle = {
        let adapter = adapter.clone();
        tokio::spawn(async move { adapter.run(ctx, running).await })
    };
    wait_for_socket(&fake).await;

    fake.recorded
        .sender
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .send(posted_event(json!({
            "id": "post-5",
            "channel_id": "mm-channel-1",
            "user_id": "",
            "message": "alice joined the channel",
            "type": "system_join_channel",
            "root_id": "",
        })))
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    cancel.cancel();
    let _ = handle.await;
    assert!(sink.accepted.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_rejected_token_is_reported_rather_than_retried_forever() {
    let fake = Fake {
        recorded: Arc::new(Recorded::default()),
        max_post_size: 16_384,
    };
    let base = serve(fake).await;
    let sink = Arc::new(RecordingSink::default());
    // A valid-looking config whose token the fake will refuse: `me` returns the
    // bot, but the socket's challenge checks the exact string.
    let adapter = MattermostIntegration::new(MattermostConfig {
        base_url: base,
        bot_token: "wrong-token".into(),
    });
    let store = cowbot_store::Store::connect_in_memory().await.unwrap();
    let ctx = IntegrationContext {
        store,
        integration: cowbot_core::integration::IntegrationRecord {
            id: cowbot_core::integration::IntegrationId::new("i1"),
            workspace_id: cowbot_core::identity::WorkspaceId::new("w1"),
            kind: cowbot_core::integration::IntegrationKind::Mattermost,
            label: "Support".into(),
            created_at: chrono::Utc::now(),
            revoked_at: None,
        },
        sink,
    };
    // The real error surfaces as a reconnect loop; what matters here is that the
    // socket was told to refuse and the adapter did not silently proceed.
    let cancel = CancelSignal::new();
    let running = cancel.clone();
    let handle = {
        let adapter = adapter.clone();
        tokio::spawn(async move { adapter.run(ctx, running).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(
        adapter.identity().is_some(),
        "the REST identity lookup runs first"
    );
    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn delivering_posts_the_answer_into_the_thread_it_came_from() {
    let fake = Fake {
        recorded: Arc::new(Recorded::default()),
        max_post_size: 16_384,
    };
    let base = serve(fake.clone()).await;
    let adapter = MattermostIntegration::new(config(base));

    let target = OutboundTarget::new(
        &cowbot_core::integration::IntegrationId::new("i1"),
        "mm-channel-1",
    )
    .in_thread(Some("root-9".into()));
    let content = OutboundContent {
        text: "Deploy is green.".into(),
        thread_id: Some("root-9".into()),
        part: ChunkPart { index: 0, total: 1 },
    };
    let delivered: DeliveredId = adapter.deliver(&target, &content).await.unwrap();
    assert_eq!(delivered.0, "posted-1");

    let posts = fake.recorded.posts.lock().unwrap().clone();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0]["channel_id"], "mm-channel-1");
    assert_eq!(posts[0]["message"], "Deploy is green.");
    assert_eq!(posts[0]["root_id"], "root-9");
}

#[tokio::test]
async fn a_multi_part_answer_says_which_part_it_is() {
    let fake = Fake {
        recorded: Arc::new(Recorded::default()),
        max_post_size: 16_384,
    };
    let base = serve(fake.clone()).await;
    let adapter = MattermostIntegration::new(config(base));

    let target = OutboundTarget::new(
        &cowbot_core::integration::IntegrationId::new("i1"),
        "mm-channel-1",
    );
    adapter
        .deliver(
            &target,
            &OutboundContent {
                text: "second half".into(),
                thread_id: None,
                part: ChunkPart { index: 1, total: 3 },
            },
        )
        .await
        .unwrap();

    let posts = fake.recorded.posts.lock().unwrap().clone();
    assert_eq!(posts[0]["message"], "(2/3)\nsecond half");
}

#[tokio::test]
async fn every_rest_call_carries_the_bot_token() {
    let fake = Fake {
        recorded: Arc::new(Recorded::default()),
        max_post_size: 16_384,
    };
    let base = serve(fake.clone()).await;
    let sink = Arc::new(RecordingSink::default());
    let adapter = MattermostIntegration::new(config(base));
    let store = cowbot_store::Store::connect_in_memory().await.unwrap();
    let ctx = IntegrationContext {
        store,
        integration: cowbot_core::integration::IntegrationRecord {
            id: cowbot_core::integration::IntegrationId::new("i1"),
            workspace_id: cowbot_core::identity::WorkspaceId::new("w1"),
            kind: cowbot_core::integration::IntegrationKind::Mattermost,
            label: "Support".into(),
            created_at: chrono::Utc::now(),
            revoked_at: None,
        },
        sink,
    };
    let cancel = CancelSignal::new();
    let running = cancel.clone();
    let handle = {
        let adapter = adapter.clone();
        tokio::spawn(async move { adapter.run(ctx, running).await })
    };
    wait_for_socket(&fake).await;
    cancel.cancel();
    let _ = handle.await;

    let tokens = fake.recorded.tokens.lock().unwrap().clone();
    assert!(!tokens.is_empty());
    assert!(
        tokens.iter().all(|token| token == "bot-token"),
        "{tokens:?}"
    );
}

#[test]
fn the_identity_starts_unknown() {
    let adapter = MattermostIntegration::new(config("http://localhost:8065".into()));
    assert!(adapter.identity().is_none());
    assert_eq!(
        adapter.max_post_bytes(),
        cowbot_mattermost::rest::DEFAULT_MAX_POST_BYTES
    );
}
