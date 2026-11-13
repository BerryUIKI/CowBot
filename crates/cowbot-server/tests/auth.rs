//! The server's account surface: sign-in, sessions, and password changes.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use cowbot_server::auth::AuthService;
use cowbot_server::config::ServerConfig;
use cowbot_server::{app, ApiError, AppState};
use cowbot_store::Store;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const PASSWORD: &str = "correct-horse-battery";

async fn test_app() -> (Router, Store) {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let config = ServerConfig {
        owner_email: "owner@example.com".into(),
        ..ServerConfig::default()
    };
    let auth = Arc::new(AuthService::new(store.clone(), config));
    auth.bootstrap(PASSWORD).await.unwrap();
    let state = AppState::new(store.clone(), auth);
    (app(state), store)
}

async fn call(
    router: Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

#[tokio::test]
async fn health_reports_the_api_version_without_a_token() {
    let (router, _) = test_app().await;
    let (status, body) = call(router, "GET", "/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["api_version"], 1);
}

#[tokio::test]
async fn signing_in_returns_a_token_and_the_workspaces() {
    let (router, _) = test_app().await;
    let (status, body) = call(
        router,
        "POST",
        "/api/v1/auth/sign-in",
        Some(json!({ "email": "owner@example.com", "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body["token"]
        .as_str()
        .is_some_and(|token| token.len() >= 64));
    assert_eq!(body["user"]["email"], "owner@example.com");
    assert_eq!(body["workspaces"][0]["role"], "owner");
    // The password hash never crosses the wire.
    assert!(body["user"].get("password_hash").is_none());
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_email_are_indistinguishable() {
    let (router, _) = test_app().await;
    let (wrong_status, wrong_body) = call(
        router.clone(),
        "POST",
        "/api/v1/auth/sign-in",
        Some(json!({ "email": "owner@example.com", "password": "not-the-password" })),
        None,
    )
    .await;
    let (unknown_status, unknown_body) = call(
        router,
        "POST",
        "/api/v1/auth/sign-in",
        Some(json!({ "email": "nobody@example.com", "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(wrong_status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong_body, unknown_body);
}

#[tokio::test]
async fn the_session_token_authenticates_me() {
    let (router, _) = test_app().await;
    let (_, signed_in) = call(
        router.clone(),
        "POST",
        "/api/v1/auth/sign-in",
        Some(json!({ "email": "owner@example.com", "password": PASSWORD })),
        None,
    )
    .await;
    let token = signed_in["token"].as_str().unwrap().to_string();

    let (status, me) = call(router.clone(), "GET", "/api/v1/me", None, Some(&token)).await;
    assert_eq!(status, StatusCode::OK, "body: {me}");
    assert_eq!(me["user"]["email"], "owner@example.com");
    assert!(me["session_expires_at"].is_string());

    // Signing out drops it.
    let (status, _) = call(
        router.clone(),
        "POST",
        "/api/v1/auth/sign-out",
        None,
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(router, "GET", "/api/v1/me", None, Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_missing_or_garbage_token_is_unauthorized() {
    let (router, _) = test_app().await;
    let (status, body) = call(router.clone(), "GET", "/api/v1/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthorized");

    let (status, _) = call(router, "GET", "/api/v1/me", None, Some("not-a-session")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn changing_the_password_drops_every_other_session() {
    let (router, _) = test_app().await;
    let (_, first) = call(
        router.clone(),
        "POST",
        "/api/v1/auth/sign-in",
        Some(json!({ "email": "owner@example.com", "password": PASSWORD })),
        None,
    )
    .await;
    let first_token = first["token"].as_str().unwrap().to_string();
    let (_, second) = call(
        router.clone(),
        "POST",
        "/api/v1/auth/sign-in",
        Some(json!({ "email": "owner@example.com", "password": PASSWORD })),
        None,
    )
    .await;
    let second_token = second["token"].as_str().unwrap().to_string();

    let (status, changed) = call(
        router.clone(),
        "POST",
        "/api/v1/auth/password",
        Some(json!({
            "current_password": PASSWORD,
            "new_password": "a-much-better-secret",
        })),
        Some(&second_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {changed}");
    let reissued = changed["token"].as_str().unwrap().to_string();
    assert_ne!(reissued, second_token);

    // Both old tokens are dead; the re-issued one works.
    for stale in [&first_token, &second_token] {
        let (status, _) = call(router.clone(), "GET", "/api/v1/me", None, Some(stale)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "stale token still worked");
    }
    let (status, _) = call(router.clone(), "GET", "/api/v1/me", None, Some(&reissued)).await;
    assert_eq!(status, StatusCode::OK);

    // The old password no longer signs in; the new one does.
    let (status, _) = call(
        router.clone(),
        "POST",
        "/api/v1/auth/sign-in",
        Some(json!({ "email": "owner@example.com", "password": PASSWORD })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(
        router,
        "POST",
        "/api/v1/auth/sign-in",
        Some(json!({ "email": "owner@example.com", "password": "a-much-better-secret" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_short_password_is_refused_at_bootstrap_and_at_change() {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let config = ServerConfig::default();
    let auth = AuthService::new(store.clone(), config.clone());
    assert!(auth.bootstrap("short").await.is_err());

    let auth = Arc::new(AuthService::new(store.clone(), config));
    auth.bootstrap(PASSWORD).await.unwrap();
    let state = AppState::new(store, auth);
    let router = app(state);
    let (_, signed_in) = call(
        router.clone(),
        "POST",
        "/api/v1/auth/sign-in",
        Some(json!({ "email": "owner@cowbot.local", "password": PASSWORD })),
        None,
    )
    .await;
    let token = signed_in["token"].as_str().unwrap().to_string();
    let (status, body) = call(
        router,
        "POST",
        "/api/v1/auth/password",
        Some(json!({ "current_password": PASSWORD, "new_password": "short" })),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("at least"));
}

#[tokio::test]
async fn bootstrap_leaves_an_existing_database_alone() {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let config = ServerConfig::default();
    let auth = AuthService::new(store.clone(), config);
    assert!(auth.bootstrap(PASSWORD).await.unwrap().is_some());
    assert!(auth.bootstrap(PASSWORD).await.unwrap().is_none());
    assert_eq!(store.list_users().await.unwrap().len(), 1);
}

#[tokio::test]
async fn repeated_failures_rate_limit_the_source() {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let config = ServerConfig {
        login_attempts: 3,
        ..ServerConfig::default()
    };
    let auth = AuthService::new(store.clone(), config);
    auth.bootstrap(PASSWORD).await.unwrap();

    for _ in 0..3 {
        assert!(auth
            .sign_in("owner@cowbot.local", "wrong-password", "10.0.0.1")
            .await
            .is_err());
    }
    // The correct password is refused while the source is limited.
    let limited = auth
        .sign_in("owner@cowbot.local", PASSWORD, "10.0.0.1")
        .await
        .unwrap_err();
    assert!(limited.to_string().contains("too many sign-in attempts"));

    // A different source is unaffected.
    assert!(auth
        .sign_in("owner@cowbot.local", PASSWORD, "10.0.0.2")
        .await
        .is_ok());
}

#[tokio::test]
async fn an_api_error_serializes_its_code_and_message() {
    let error = ApiError::not_found("nothing here");
    assert_eq!(error.status, StatusCode::NOT_FOUND);
    assert_eq!(error.code, "not_found");
}
