//! Feishu's webhook transport, end to end (ADR-0062).
//!
//! The crypto has unit tests of its own; what these prove is the ordering — that
//! a delivery is decrypted before it is verified, that the handshake is answered
//! even when encrypted, and that a body which fails either check never reaches
//! the pipeline.

use std::sync::Arc;

use cowbot_core::error::CowResult;
use cowbot_core::integration::{IntegrationKind, IntegrationRecord};
use cowbot_integrations::adapter::{InboundRequest, Integration, IntegrationContext, MessageSink};
use cowbot_integrations::model::{Ingested, Skipped};
use cowbot_store::Store;

/// A sink that records nothing; these tests are about whether it is reached.
struct CountingSink {
    accepted: std::sync::atomic::AtomicUsize,
}

impl CountingSink {
    fn new() -> Self {
        Self {
            accepted: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn count(&self) -> usize {
        self.accepted.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl MessageSink for CountingSink {
    async fn accept(&self, _message: cowbot_integrations::InboundMessage) -> CowResult<Ingested> {
        self.accepted
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Ingested::Skipped(Skipped::Unbound))
    }
}

const ENCRYPT_KEY: &str = "the-encrypt-key";
const VERIFICATION_TOKEN: &str = "the-verification-token";

async fn adapter_with(transport: &str) -> cowbot_feishu::FeishuIntegration {
    let config = format!(
        r#"{{"app_id":"cli_1","app_secret":"s","transport":"{transport}",
             "verification_token":"{VERIFICATION_TOKEN}","encrypt_key":"{ENCRYPT_KEY}"}}"#
    );
    let adapter = cowbot_feishu::FeishuIntegration::new(
        cowbot_feishu::FeishuConfig::from_json(&config).unwrap(),
    );
    adapter.set_bot(cowbot_feishu::api::BotInfo {
        open_id: "ou_bot".into(),
        app_name: "CowBot".into(),
    });
    adapter
}

async fn context(sink: Arc<CountingSink>, transport: &str) -> IntegrationContext {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let _ = transport;
    IntegrationContext {
        store,
        bus: cowbot_bus::EventBus::default_bus(),
        integration: IntegrationRecord {
            id: cowbot_core::integration::IntegrationId::new("i1"),
            workspace_id: cowbot_core::identity::WorkspaceId::new("w1"),
            kind: IntegrationKind::Feishu,
            label: "Support".into(),
            created_at: chrono::Utc::now(),
            revoked_at: None,
        },
        sink,
        attachments_dir: None,
    }
}

/// The headers Feishu sends, signed for the body it is sending.
fn headers(body: &[u8], card: bool) -> Vec<(String, String)> {
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let nonce = "n0nce";
    let signature = if card {
        cowbot_feishu::crypto::sign_card(VERIFICATION_TOKEN, &timestamp, nonce, body)
    } else {
        cowbot_feishu::crypto::sign_event(ENCRYPT_KEY, &timestamp, nonce, body)
    };
    vec![
        ("x-lark-request-timestamp".into(), timestamp),
        ("x-lark-request-nonce".into(), nonce.to_string()),
        ("x-lark-signature".into(), signature),
    ]
}

fn request<'a>(headers: &'a [(String, String)], body: &'a [u8]) -> InboundRequest<'a> {
    InboundRequest { headers, body }
}

#[tokio::test]
async fn a_signed_plaintext_event_reaches_the_pipeline() {
    // An app with no Encrypt Key receives plaintext; the signature is then the
    // only authentication, and it has to be enough.
    let config = format!(
        r#"{{"app_id":"cli_1","app_secret":"s","transport":"webhook",
             "verification_token":"{VERIFICATION_TOKEN}"}}"#
    );
    let adapter = cowbot_feishu::FeishuIntegration::new(
        cowbot_feishu::FeishuConfig::from_json(&config).unwrap(),
    );
    adapter.set_bot(cowbot_feishu::api::BotInfo {
        open_id: "ou_bot".into(),
        app_name: "CowBot".into(),
    });
    let sink = Arc::new(CountingSink::new());
    let ctx = context(sink.clone(), "webhook").await;

    // Signed with the Encrypt Key, which this app does not have: an empty key.
    // The real shape: `sender_type` sits beside `sender_id`, not inside it.
    // `header.app_id` is required: the adapter refuses a delivery for any app but
    // its own, which is a second check behind the signature rather than a
    // substitute for it.
    let body =
        br#"{"schema":"2.0","header":{"app_id":"cli_1","event_type":"im.message.receive_v1"},
        "event":{"sender":{"sender_type":"user","sender_id":{"open_id":"ou_ada"}},
        "message":{"message_id":"om_1","chat_id":"oc_1","chat_type":"p2p",
        "message_type":"text","content":"{\"text\":\"hi\"}"}}}"#;
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let signature = cowbot_feishu::crypto::sign_event("", &timestamp, "n", body);
    let headers = vec![
        ("x-lark-request-timestamp".to_string(), timestamp),
        ("x-lark-request-nonce".to_string(), "n".to_string()),
        ("x-lark-signature".to_string(), signature),
    ];

    let reply = adapter
        .webhook(&ctx, request(&headers, body))
        .await
        .unwrap();
    assert_eq!(reply, cowbot_integrations::WebhookReply::Ack);
    // The message reached the sink; what the sink does with it is the pipeline's
    // business and needs a bound channel this fixture does not have.
    assert_eq!(sink.count(), 1, "the delivery never reached the pipeline");
}

#[tokio::test]
async fn an_encrypted_handshake_is_answered_in_plaintext() {
    // Feishu sends the challenge inside the encrypted envelope while an operator
    // saves the URL, and expects it echoed in plaintext — before the app is
    // otherwise reachable. Answering it requires decrypting first, which is why
    // the decrypt runs before the signature check.
    let adapter = adapter_with("webhook").await;
    let sink = Arc::new(CountingSink::new());
    let ctx = context(sink, "webhook").await;

    let challenge = r#"{"challenge":"abc123","token":"t","type":"url_verification"}"#;
    let sealed = sealed_envelope(challenge);
    let headers = headers(sealed.as_bytes(), false);

    let reply = adapter
        .webhook(&ctx, request(&headers, sealed.as_bytes()))
        .await
        .unwrap();
    let cowbot_integrations::WebhookReply::Challenge(body) = reply else {
        panic!("expected the challenge echoed, got {reply:?}");
    };
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["challenge"], "abc123");
}

#[tokio::test]
async fn a_tampered_envelope_is_refused_before_the_pipeline_sees_it() {
    let adapter = adapter_with("webhook").await;
    let sink = Arc::new(CountingSink::new());
    let ctx = context(sink.clone(), "webhook").await;

    let sealed = sealed_envelope(r#"{"type":"event_callback"}"#);
    // Signed for one body, delivered as another.
    let headers = headers(br#"{"something":"else"}"#, false);
    let error = adapter
        .webhook(&ctx, request(&headers, sealed.as_bytes()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Feishu refused"), "{error}");
    assert_eq!(sink.count(), 0, "a refused delivery reached the pipeline");
}

#[tokio::test]
async fn a_wrong_signature_is_refused() {
    let adapter = adapter_with("webhook").await;
    let sink = Arc::new(CountingSink::new());
    let ctx = context(sink.clone(), "webhook").await;

    let sealed = sealed_envelope(r#"{"type":"event_callback"}"#);
    let mut headers = headers(sealed.as_bytes(), false);
    headers[2].1 = "0".repeat(64);
    let error = adapter
        .webhook(&ctx, request(&headers, sealed.as_bytes()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("refused"), "{error}");
    assert_eq!(sink.count(), 0);
}

#[tokio::test]
async fn an_unencrypted_delivery_is_refused_when_a_key_is_configured() {
    // An app with an Encrypt Key that receives plaintext is misconfigured. The
    // signature would still pass for an attacker who knows the key is unused, so
    // this is refused rather than read.
    let adapter = adapter_with("webhook").await;
    let sink = Arc::new(CountingSink::new());
    let ctx = context(sink.clone(), "webhook").await;

    let body = br#"{"type":"event_callback"}"#;
    let headers = headers(body, false);
    let error = adapter
        .webhook(&ctx, request(&headers, body))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not encrypted"), "{error}");
    assert_eq!(sink.count(), 0);
}

#[tokio::test]
async fn a_long_connection_adapter_does_not_accept_webhooks() {
    // The flag the server route reads. A row on the default transport must not
    // expose an inbound surface at all.
    let adapter = adapter_with("long_connection").await;
    assert!(!adapter.accepts_webhook());
    let webhook = adapter_with("webhook").await;
    assert!(webhook.accepts_webhook());
}

/// Encrypt a body the way Feishu's webhook transport does, for a test.
fn sealed_envelope(payload: &str) -> String {
    // The test-only encryptor lives behind `cfg(test)` in the crate, so a test
    // outside it builds the envelope the way the adapter reads one: the payload
    // is what matters, and the key is the configured one.
    let sealed = cowbot_feishu::crypto::seal_for_test(ENCRYPT_KEY, payload);
    serde_json::json!({ "encrypt": sealed }).to_string()
}
