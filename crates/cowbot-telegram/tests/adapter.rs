//! The Telegram adapter against a fake Bot API (ADR-0058).
//!
//! The fake answers the same envelope Telegram does — `{"ok":true,…}` or
//! `{"ok":false,…}` — so the test drives the real poll loop, the real HTTP
//! client, the real offset handling and the real rendering, with no Telegram
//! account.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use cowbot_core::cancel::CancelSignal;
use cowbot_integrations::adapter::{Integration, IntegrationContext};
use cowbot_integrations::model::{ChunkPart, OutboundContent, OutboundTarget};
use cowbot_integrations::Ingested;
use cowbot_telegram::{TelegramConfig, TelegramIntegration};
use serde_json::{json, Value};

/// What the fake was asked to do.
#[derive(Default)]
struct Recorded {
    /// Bodies of every `sendMessage`, in order.
    sent: Mutex<Vec<Value>>,
    /// Every `offset` a `getUpdates` presented.
    offsets: Mutex<Vec<i64>>,
    /// The `message_thread_id` of each `sendMessage`.
    threads: Mutex<Vec<Option<i64>>>,
}

#[derive(Clone)]
struct Fake {
    recorded: Arc<Recorded>,
    /// Batches to hand out, one per `getUpdates`, in order. Empty afterwards.
    batches: Arc<Mutex<std::collections::VecDeque<Vec<Value>>>>,
    /// What `getChat` answers with.
    chat_error: Option<(u16, String)>,
    chat: Value,
    /// The `status` `getChatMember` reports.
    member_status: String,
    /// Status code for `sendMessage` with `parse_mode`, to exercise the
    /// plain-text fallback.
    reject_html: bool,
}

impl Fake {
    fn new() -> Self {
        Self {
            recorded: Arc::new(Recorded::default()),
            batches: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            chat_error: None,
            chat: json!({ "id": 7, "type": "private", "first_name": "Ada" }),
            member_status: "administrator".into(),
            reject_html: false,
        }
    }

    fn with_batches(self, batches: Vec<Vec<Value>>) -> Self {
        {
            let mut queue = self.batches.lock().unwrap();
            for batch in batches {
                queue.push_back(batch);
            }
        }
        self
    }
}

#[derive(Clone)]
struct State_ {
    fake: Arc<Fake>,
}

async fn get_me() -> Json<Value> {
    Json(json!({
        "ok": true,
        "result": { "id": 42, "is_bot": true, "first_name": "CowBot", "username": "cowbot_help_bot" }
    }))
}

async fn get_updates(State(state): State<State_>, Json(body): Json<Value>) -> Json<Value> {
    let fake = &state.fake;
    fake.recorded
        .offsets
        .lock()
        .unwrap()
        .push(body["offset"].as_i64().unwrap_or(0));
    // Exactly two kinds, and both are handled. Asking for a kind the adapter
    // drops is a real cost on a busy bot; *not* asking for `callback_query` is
    // the quietest way to build a button that does nothing (ADR-0060).
    assert_eq!(
        body["allowed_updates"],
        json!(["message", "callback_query"]),
        "the poll must ask for the update kinds the adapter handles, and no others"
    );
    let batch = fake.batches.lock().unwrap().pop_front().unwrap_or_default();
    Json(json!({ "ok": true, "result": batch }))
}

async fn send_message(State(state): State<State_>, Json(body): Json<Value>) -> Json<Value> {
    if state.fake.reject_html && body["parse_mode"].is_string() {
        return Json(json!({
            "ok": false,
            "error_code": 400,
            "description": "Bad Request: can't parse entities: Unclosed tag"
        }));
    }
    state.fake.recorded.sent.lock().unwrap().push(body.clone());
    state
        .fake
        .recorded
        .threads
        .lock()
        .unwrap()
        .push(body["message_thread_id"].as_i64());
    Json(
        json!({ "ok": true, "result": { "message_id": 900 + body["chat_id"].as_str().unwrap_or("0").len() as i64 } }),
    )
}

async fn get_chat(State(state): State<State_>, _body: Json<Value>) -> Json<Value> {
    match &state.fake.chat_error {
        Some((code, description)) => Json(json!({
            "ok": false,
            "error_code": code,
            "description": description,
        })),
        None => Json(json!({ "ok": true, "result": state.fake.chat })),
    }
}

async fn get_chat_member(State(state): State<State_>, _body: Json<Value>) -> Json<Value> {
    Json(json!({ "ok": true, "result": { "status": state.fake.member_status } }))
}

async fn serve(fake: Fake) -> String {
    let state = State_ {
        fake: Arc::new(fake),
    };
    let router = Router::new()
        .route("/bot{token}/getMe", post(get_me))
        .route("/bot{token}/getUpdates", post(get_updates))
        .route("/bot{token}/sendMessage", post(send_message))
        .route("/bot{token}/getChat", post(get_chat))
        .route("/bot{token}/getChatMember", post(get_chat_member))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{address}")
}

fn config(base: &str) -> TelegramConfig {
    TelegramConfig::from_json(&format!(
        r#"{{"bot_token":"42:secret","api_base":"{base}"}}"#
    ))
    .unwrap()
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

fn context(store: cowbot_store::Store, sink: Arc<RecordingSink>) -> IntegrationContext {
    IntegrationContext {
        store,
        bus: cowbot_bus::EventBus::default_bus(),
        integration: cowbot_core::integration::IntegrationRecord {
            id: cowbot_core::integration::IntegrationId::new("i1"),
            workspace_id: cowbot_core::identity::WorkspaceId::new("w1"),
            kind: cowbot_core::integration::IntegrationKind::Telegram,
            label: "Support".into(),
            created_at: chrono::Utc::now(),
            revoked_at: None,
        },
        sink,
        attachments_dir: None,
    }
}

fn direct_message(update_id: i64, text: &str) -> Value {
    json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id,
            "date": 1_700_000_000i64,
            "chat": { "id": 7, "type": "private", "first_name": "Ada", "username": "ada" },
            "from": { "id": 7, "is_bot": false, "first_name": "Ada", "username": "ada" },
            "text": text,
            "entities": [],
        }
    })
}

/// Wait until the sink has accepted `count` messages.
async fn wait_for(sink: &RecordingSink, count: usize) {
    for _ in 0..300 {
        if sink.accepted.lock().unwrap().len() >= count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!(
        "expected {count} messages, saw {}",
        sink.accepted.lock().unwrap().len()
    );
}

#[tokio::test]
async fn a_private_message_reaches_the_pipeline_and_the_offset_follows_it() {
    let fake = Fake::new().with_batches(vec![vec![direct_message(500, "what is the status?")]]);
    let base = serve(fake).await;
    let sink = Arc::new(RecordingSink::default());
    let adapter = TelegramIntegration::new(config(&base));
    let store = cowbot_store::Store::connect_in_memory().await.unwrap();

    let cancel = CancelSignal::new();
    let running = cancel.clone();
    let handle = {
        let adapter = adapter.clone();
        let ctx = context(store, sink.clone());
        tokio::spawn(async move { adapter.run(ctx, running).await })
    };

    wait_for(&sink, 1).await;
    cancel.cancel();
    let _ = handle.await;

    let accepted = sink.accepted.lock().unwrap().clone();
    assert_eq!(accepted.len(), 1);
    assert_eq!(
        accepted[0].event_id, "500",
        "the update id is the dedupe key"
    );
    assert_eq!(accepted[0].external_conversation_id, "7");
    assert!(accepted[0].addresses_bot, "a private chat needs no mention");
    assert_eq!(accepted[0].author.display_name, "@ada");
}

#[tokio::test]
async fn the_next_poll_confirms_the_update_it_handled() {
    // `offset = update_id + 1` is the only acknowledgement Telegram has. Getting
    // it wrong replays every batch for ever; getting it right is what makes the
    // dedupe table a backstop rather than the mechanism.
    let fake = Fake::new().with_batches(vec![
        vec![direct_message(500, "first")],
        vec![direct_message(501, "second")],
    ]);
    let base = serve(fake.clone()).await;
    let sink = Arc::new(RecordingSink::default());
    let adapter = TelegramIntegration::new(config(&base));
    let store = cowbot_store::Store::connect_in_memory().await.unwrap();

    let cancel = CancelSignal::new();
    let running = cancel.clone();
    let handle = {
        let adapter = adapter.clone();
        let ctx = context(store, sink.clone());
        tokio::spawn(async move { adapter.run(ctx, running).await })
    };

    wait_for(&sink, 2).await;
    cancel.cancel();
    let _ = handle.await;

    let offsets = fake.recorded.offsets.lock().unwrap().clone();
    assert_eq!(
        &offsets[..3.min(offsets.len())],
        &[0, 501, 502][..3.min(offsets.len())],
        "offsets were {offsets:?}"
    );
}

#[tokio::test]
async fn a_webhook_conflict_is_reported_rather_than_retried_for_ever() {
    // A 409 never fixes itself: a webhook is registered, or another process
    // polls the same token. A retry loop would spin silently.
    let state = State_ {
        fake: Arc::new(Fake::new()),
    };
    let router = Router::new()
        .route("/bot{token}/getMe", post(get_me))
        .route(
            "/bot{token}/getUpdates",
            post(|| async {
                Json(json!({
                    "ok": false,
                    "error_code": 409,
                    "description": "Conflict: can't use getUpdates method while webhook is active"
                }))
            }),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let adapter = TelegramIntegration::new(config(&format!("http://{address}")));
    let store = cowbot_store::Store::connect_in_memory().await.unwrap();
    let ctx = context(store, Arc::new(RecordingSink::default()));
    let cancel = CancelSignal::new();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), adapter.run(ctx, cancel))
        .await
        .expect("a conflict must not be retried");

    let error = result.expect_err("a conflict is an error");
    let message = error.to_string();
    assert!(message.contains("webhook"), "{message}");
    assert!(message.contains("deleteWebhook"), "{message}");
}

#[tokio::test]
async fn a_rejected_token_is_reported_rather_than_retried() {
    let router = Router::new().route(
        "/bot{token}/getMe",
        post(|| async {
            Json(json!({
                "ok": false,
                "error_code": 401,
                "description": "Unauthorized"
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let adapter = TelegramIntegration::new(config(&format!("http://{address}")));
    let store = cowbot_store::Store::connect_in_memory().await.unwrap();
    let ctx = context(store, Arc::new(RecordingSink::default()));
    let result = adapter.run(ctx, CancelSignal::new()).await;

    let message = result.expect_err("a bad token is an error").to_string();
    assert!(message.contains("refused the bot token"), "{message}");
    // The token is in the URL; an error must not carry it.
    assert!(!message.contains("secret"), "{message}");
}

#[tokio::test]
async fn delivering_posts_the_answer_with_telegram_markup() {
    let fake = Fake::new();
    let base = serve(fake.clone()).await;
    let adapter = TelegramIntegration::new(config(&base));

    let target = OutboundTarget::new(&cowbot_core::integration::IntegrationId::new("i1"), "7")
        .in_thread(Some("88".into()));
    let content = OutboundContent {
        text: "Deploy is **green**: run `cargo test`.".into(),
        thread_id: Some("88".into()),
        part: ChunkPart { index: 0, total: 1 },
    };
    let delivered = adapter.deliver(&target, &content).await.unwrap();

    let sent = fake.recorded.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["chat_id"], "7");
    assert_eq!(sent[0]["parse_mode"], "HTML");
    assert_eq!(
        sent[0]["text"],
        "Deploy is <b>green</b>: run <code>cargo test</code>."
    );
    // The forum topic is preserved, or the answer lands in the wrong topic.
    assert_eq!(sent[0]["message_thread_id"], 88);
    assert!(!delivered.0.is_empty());
}

#[tokio::test]
async fn markup_telegram_cannot_parse_falls_back_to_plain_text() {
    // An answer lost to a formatting quirk is worse than an unformatted one.
    let mut fake = Fake::new();
    fake.reject_html = true;
    let base = serve(fake.clone()).await;
    let adapter = TelegramIntegration::new(config(&base));

    let target = OutboundTarget::new(&cowbot_core::integration::IntegrationId::new("i1"), "7");
    let content = OutboundContent {
        text: "a < b".into(),
        thread_id: None,
        part: ChunkPart { index: 0, total: 1 },
    };
    adapter.deliver(&target, &content).await.unwrap();

    let sent = fake.recorded.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "the fallback did not post");
    assert!(
        sent[0]["parse_mode"].is_null(),
        "the fallback still used markup"
    );
    assert_eq!(sent[0]["text"], "a < b");
}

#[tokio::test]
async fn a_multi_part_answer_says_which_part_it_is() {
    let fake = Fake::new();
    let base = serve(fake.clone()).await;
    let adapter = TelegramIntegration::new(config(&base));

    let target = OutboundTarget::new(&cowbot_core::integration::IntegrationId::new("i1"), "7");
    let content = OutboundContent {
        text: "second half".into(),
        thread_id: None,
        part: ChunkPart { index: 1, total: 3 },
    };
    adapter.deliver(&target, &content).await.unwrap();

    let sent = fake.recorded.sent.lock().unwrap().clone();
    assert_eq!(sent[0]["text"], "(2/3)\nsecond half");
}

#[tokio::test]
async fn a_person_holding_the_token_is_refused_as_a_bot() {
    // A human account's token would make the adapter answer as that person.
    let router = Router::new().route(
        "/bot{token}/getMe",
        post(|| async {
            Json(json!({
                "ok": true,
                "result": { "id": 9, "is_bot": false, "first_name": "Ada", "username": "ada" }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let adapter = TelegramIntegration::new(config(&format!("http://{address}")));
    let store = cowbot_store::Store::connect_in_memory().await.unwrap();
    let ctx = context(store, Arc::new(RecordingSink::default()));
    let error = adapter.run(ctx, CancelSignal::new()).await.unwrap_err();
    assert!(error.to_string().contains("person, not a bot"), "{error}");
}

#[tokio::test]
async fn a_chat_the_bot_cannot_reach_is_unreachable_with_a_fixable_reason() {
    let mut fake = Fake::new();
    fake.chat_error = Some((400, "Bad Request: chat not found".into()));
    let base = serve(fake).await;
    let adapter = TelegramIntegration::new(config(&base));
    adapter.set_bot(cowbot_telegram::update::BotHandle::new(
        "42",
        "cowbot_help_bot",
    ));

    let health = adapter.check_binding("7").await;
    let reason = health.problem().expect("an unreachable binding says why");
    // The reason names the fix, not the symptom: a private chat needs the person
    // to message the bot first, which no binding row can say.
    assert!(reason.contains("message the bot once"), "{reason}");
}

#[tokio::test]
async fn a_reachable_private_chat_is_confirmed() {
    let fake = Fake::new();
    let base = serve(fake).await;
    let adapter = TelegramIntegration::new(config(&base));
    adapter.set_bot(cowbot_telegram::update::BotHandle::new(
        "42",
        "cowbot_help_bot",
    ));
    assert_eq!(
        adapter.check_binding("7").await,
        cowbot_integrations::BindingHealth::Reachable
    );
}

#[tokio::test]
async fn a_channel_without_admin_rights_is_unreachable_because_posting_is_refused() {
    // A channel is the one chat kind where being present is not enough: a bot
    // that is a plain member can read it and cannot post.
    let mut fake = Fake::new();
    fake.chat = json!({ "id": -1009876, "type": "channel", "title": "Announcements" });
    fake.member_status = "member".into();
    let base = serve(fake).await;
    let adapter = TelegramIntegration::new(config(&base));
    adapter.set_bot(cowbot_telegram::update::BotHandle::new(
        "42",
        "cowbot_help_bot",
    ));

    let health = adapter.check_binding("-1009876").await;
    let reason = health
        .problem()
        .expect("a binding that cannot post says why");
    assert!(reason.contains("administrator"), "{reason}");
    assert!(reason.contains("Announcements"), "{reason}");
}

#[tokio::test]
async fn a_server_fault_is_unknown_rather_than_unreachable() {
    // A 500 is not an answer about the binding; reporting it as unreachable
    // would send an operator to fix something that is fine.
    let mut fake = Fake::new();
    fake.chat_error = Some((500, "Internal Server Error".into()));
    let base = serve(fake).await;
    let adapter = TelegramIntegration::new(config(&base));
    adapter.set_bot(cowbot_telegram::update::BotHandle::new(
        "42",
        "cowbot_help_bot",
    ));

    let health = adapter.check_binding("7").await;
    assert!(!health.is_reachable());
    assert!(matches!(
        health,
        cowbot_integrations::BindingHealth::Unknown(_)
    ));
}
