//! The inbound webhook surface (ADR-0062).
//!
//! What these cover is the route's own behaviour, not any platform's: it accepts
//! a request with no CowBot session, refuses one for an integration that does not
//! exist or was revoked, and refuses one whose adapter never opted into an
//! inbound surface. The per-platform verification is tested in the platform
//! crates, where the signature scheme lives.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use cowbot_bus::EventBus;
use cowbot_core::identity::Workspace;
use cowbot_core::integration::{IntegrationKind, NewIntegration};
use cowbot_server::auth::AuthService;
use cowbot_server::config::ServerConfig;
use cowbot_server::webhooks::WebhookReceiver;
use cowbot_server::{app, AppState};
use cowbot_store::Store;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn router(store: &Store) -> Router {
    let config = ServerConfig {
        owner_email: "owner@example.com".into(),
        ..ServerConfig::default()
    };
    let auth = Arc::new(AuthService::new(store.clone(), config));
    auth.bootstrap("owner-long-password").await.unwrap();
    let receiver = WebhookReceiver::new(
        store.clone(),
        EventBus::default_bus(),
        Arc::new(cowbot_integrations::sink::RecordingTurns::new()),
        None,
    );
    app(AppState::new(store.clone(), auth, EventBus::default_bus())
        .with_webhooks(Arc::new(receiver)))
}

async fn post(router: Router, path: &str, body: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A workspace with one integration row of `kind`.
async fn workspace_with(kind: IntegrationKind) -> (Store, String) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let workspace = Workspace::new("Team");
    store.create_workspace(&workspace).await.unwrap();
    let new = NewIntegration::new(&workspace.id, kind, "Bot", r#"{"bot_token":"x"}"#);
    store.create_integration(&new).await.unwrap();
    (store, new.id.0)
}

#[tokio::test]
async fn the_endpoint_is_reachable_without_a_cowbot_session() {
    // A platform has no session; the refusal must come from the integration
    // lookup rather than from authentication, or no webhook could ever arrive.
    let (store, _) = workspace_with(IntegrationKind::Telegram).await;
    let (status, body) = post(
        router(&store).await,
        "/api/v1/integrations/missing/webhook",
        "{}",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("no integration accepts webhooks"), "{body}");
}

#[tokio::test]
async fn a_revoked_integration_is_a_404() {
    // A retired bot keeps its row so an operator can read what it was bound to;
    // it must not keep accepting traffic, and a caller learns nothing from the
    // difference.
    let (store, id) = workspace_with(IntegrationKind::Telegram).await;
    store
        .revoke_integration(&cowbot_core::integration::IntegrationId::new(id.clone()))
        .await
        .unwrap();
    let (status, _) = post(
        router(&store).await,
        &format!("/api/v1/integrations/{id}/webhook"),
        "{}",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_adapter_that_dials_out_is_not_given_an_inbound_surface() {
    // Mattermost and Telegram (in its default transport) open a socket or poll.
    // A request to a URL that was never meant to exist reads as missing rather
    // than as a verification failure, because there is nothing to verify.
    let (store, id) = workspace_with(IntegrationKind::Mattermost).await;
    let (status, _) = post(
        router(&store).await,
        &format!("/api/v1/integrations/{id}/webhook"),
        "{}",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_deployment_with_no_receiver_answers_missing_rather_than_failing() {
    // No model provider means no receiver, which is the same "nothing is
    // running" answer startup gives. A 500 would tell the platform to retry.
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let config = ServerConfig {
        owner_email: "owner@example.com".into(),
        ..ServerConfig::default()
    };
    let auth = Arc::new(AuthService::new(store.clone(), config));
    auth.bootstrap("owner-long-password").await.unwrap();
    let bare = app(AppState::new(store, auth, EventBus::default_bus()));

    let (status, _) = post(bare, "/api/v1/integrations/anything/webhook", "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
