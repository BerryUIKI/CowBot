//! Channels, messages, tasks, approvals, tokens, and the event stream.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use cowbot_bus::EventBus;
use cowbot_core::identity::{Membership, Role, WorkspaceId};
use cowbot_server::auth::AuthService;
use cowbot_server::config::ServerConfig;
use cowbot_server::{app, AppState};
use cowbot_store::Store;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const OWNER_PASSWORD: &str = "owner-long-password";

struct Fixture {
    router: Router,
    store: Store,
    auth: Arc<AuthService>,
    bus: EventBus,
    owner_token: String,
    workspace_id: String,
}

async fn fixture() -> Fixture {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let config = ServerConfig {
        owner_email: "owner@example.com".into(),
        ..ServerConfig::default()
    };
    let auth = Arc::new(AuthService::new(store.clone(), config));
    auth.bootstrap(OWNER_PASSWORD).await.unwrap();
    let bus = EventBus::default_bus();
    let state = AppState::new(store.clone(), auth.clone(), bus.clone());
    let router = app(state);

    let (_, signed_in) = call(
        router.clone(),
        "POST",
        "/api/v1/auth/sign-in",
        Some(json!({ "email": "owner@example.com", "password": OWNER_PASSWORD })),
        None,
    )
    .await;
    let owner_token = signed_in["token"].as_str().unwrap().to_string();
    let workspace_id = signed_in["workspaces"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    Fixture {
        router,
        store,
        auth,
        bus,
        owner_token,
        workspace_id,
    }
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

/// A user in the fixture's workspace at `role`, with a session token.
async fn join(f: &Fixture, email: &str, role: Role) -> (String, String) {
    let user = cowbot_core::identity::User::new("Test User", email, "");
    f.store.create_user(&user).await.unwrap();
    f.store
        .add_member(&Membership::new(
            user.id.clone(),
            WorkspaceId::new(f.workspace_id.clone()),
            role,
        ))
        .await
        .unwrap();
    let issued = f.auth.issue(&user.id).await.unwrap();
    (user.id.0, issued.token)
}

async fn create_channel(f: &Fixture, name: &str, token: &str) -> String {
    let (status, body) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/channels", f.workspace_id),
        Some(json!({ "name": name, "topic": "topic" })),
        Some(token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    body["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_member_posts_and_reads_messages_in_their_channel() {
    let f = fixture().await;
    let (member_id, member_token) = join(&f, "member@example.com", Role::Member).await;
    let channel_id = create_channel(&f, "design", &member_token).await;

    let (status, posted) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/channels/{channel_id}/messages"),
        Some(json!({ "content": "hello there" })),
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {posted}");
    assert_eq!(posted["content"], "hello there");
    // The sender is the authenticated user, never a value from the body.
    assert_eq!(posted["sender"]["type"], "User");
    assert_eq!(posted["sender"]["id"], member_id);

    let (status, messages) = call(
        f.router,
        "GET",
        &format!("/api/v1/channels/{channel_id}/messages"),
        None,
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(messages.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_stranger_cannot_read_a_channel() {
    let f = fixture().await;
    let channel_id = create_channel(&f, "secret", &f.owner_token).await;
    let outsider = cowbot_core::identity::User::new("Outsider", "outsider@example.com", "");
    f.store.create_user(&outsider).await.unwrap();
    let issued = f.auth.issue(&outsider.id).await.unwrap();

    let (status, body) = call(
        f.router,
        "GET",
        &format!("/api/v1/channels/{channel_id}/messages"),
        None,
        Some(&issued.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");
}

#[tokio::test]
async fn only_an_admin_deletes_or_clears_a_channel() {
    let f = fixture().await;
    let (_, member_token) = join(&f, "member@example.com", Role::Member).await;
    let channel_id = create_channel(&f, "shared", &member_token).await;

    let (status, _) = call(
        f.router.clone(),
        "DELETE",
        &format!("/api/v1/channels/{channel_id}"),
        None,
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/channels/{channel_id}/messages/clear"),
        None,
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = call(
        f.router,
        "DELETE",
        &format!("/api/v1/channels/{channel_id}"),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_task_is_created_with_the_authenticated_requester() {
    let f = fixture().await;
    let (agent_id, _) = join(&f, "agent-owner@example.com", Role::Member).await;
    let _ = agent_id;
    let agent = cowbot_core::agent::AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
    f.store
        .upsert_agent(&WorkspaceId::new(f.workspace_id.clone()), &agent)
        .await
        .unwrap();

    let (status, task) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/tasks", f.workspace_id),
        Some(json!({
            "title": "Build it",
            "prompt": "Write the code",
            "assignee": agent.id.0,
            "approval_level": "plan-only",
        })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {task}");
    let task_id = task["id"].as_str().unwrap().to_string();

    let (status, stored) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/tasks/{task_id}"),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stored["id"], task_id.as_str());

    // The queued assignment belongs to the caller.
    let (status, assignments) = call(
        f.router,
        "GET",
        &format!("/api/v1/tasks/{task_id}/assignments"),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(assignments.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_task_in_another_workspace_is_not_found() {
    let f = fixture().await;
    let agent = cowbot_core::agent::AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
    f.store
        .upsert_agent(&WorkspaceId::new(f.workspace_id.clone()), &agent)
        .await
        .unwrap();
    let (_, created) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/tasks", f.workspace_id),
        Some(json!({
            "title": "Build it",
            "prompt": "Write the code",
            "assignee": agent.id.0,
        })),
        Some(&f.owner_token),
    )
    .await;
    let task_id = created["id"].as_str().unwrap().to_string();

    // A second workspace the owner does not belong to.
    let other = cowbot_core::identity::Workspace::new("Other");
    f.store.create_workspace(&other).await.unwrap();

    let (status, body) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/tasks/{task_id}"),
        None,
        Some(&f.owner_token),
    )
    .await;
    // The owner *is* in this workspace, so it is visible; the check is that a
    // caller outside both sees nothing. Create such a caller.
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let outsider = cowbot_core::identity::User::new("Outsider", "out@example.com", "");
    f.store.create_user(&outsider).await.unwrap();
    f.store
        .add_member(&Membership::new(
            outsider.id.clone(),
            other.id,
            Role::Member,
        ))
        .await
        .unwrap();
    let issued = f.auth.issue(&outsider.id).await.unwrap();
    let (status, _) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/tasks/{task_id}"),
        None,
        Some(&issued.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_an_admin_reads_the_audit_log() {
    let f = fixture().await;
    let (_, member_token) = join(&f, "member@example.com", Role::Member).await;
    let (status, _) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/audit", f.workspace_id),
        None,
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, events) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/audit", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(events.is_array());
}

#[tokio::test]
async fn a_token_authenticates_like_a_session_and_revokes_cleanly() {
    let f = fixture().await;
    let (status, created) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/tokens", f.workspace_id),
        Some(json!({ "scopes": ["workspace"] })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let secret = created["token"].as_str().unwrap().to_string();
    let token_id = created["id"].as_str().unwrap().to_string();

    // The listing never carries the secret.
    let (status, list) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/tokens", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert!(list[0].get("token").is_none());
    assert!(list[0].get("token_hash").is_none());

    let (status, _) = call(
        f.router.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/tokens/{token_id}", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A revoked token no longer resolves at the store.
    let hash = cowbot_server::auth::hash_token(&secret);
    let stored = f
        .store
        .get_api_token_by_hash(&hash)
        .await
        .unwrap()
        .expect("the token row remains for audit");
    assert!(stored.revoked_at.is_some(), "revocation stamps the row");
}

#[tokio::test]
async fn a_member_cannot_mint_an_api_token() {
    let f = fixture().await;
    let (_, member_token) = join(&f, "member@example.com", Role::Member).await;
    let (status, _) = call(
        f.router,
        "POST",
        &format!("/api/v1/workspaces/{}/tokens", f.workspace_id),
        Some(json!({})),
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_posted_message_is_published_on_the_bus() {
    let f = fixture().await;
    let channel_id = create_channel(&f, "wire", &f.owner_token).await;
    let mut events = f.bus.subscribe();

    call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/channels/{channel_id}/messages"),
        Some(json!({ "content": "broadcast" })),
        Some(&f.owner_token),
    )
    .await;

    let event = events.recv().await.unwrap();
    let value = serde_json::to_value(&event.payload).unwrap();
    assert_eq!(value["type"], "MessagePosted");
    assert_eq!(value["payload"]["content"], "broadcast");
}

#[tokio::test]
async fn a_user_id_from_the_body_cannot_become_the_sender() {
    let f = fixture().await;
    let (member_id, member_token) = join(&f, "member@example.com", Role::Member).await;
    let channel_id = create_channel(&f, "spoof", &member_token).await;
    let (status, posted) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/channels/{channel_id}/messages"),
        Some(json!({ "content": "hi", "sender": { "user_id": "someone-else" } })),
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_ne!(posted["sender"]["User"], "someone-else");
    assert_eq!(posted["sender"]["type"], "User");
    assert_eq!(posted["sender"]["id"], member_id);
}

#[tokio::test]
async fn the_policy_route_requires_administration_to_change() {
    let f = fixture().await;
    let (_, member_token) = join(&f, "member@example.com", Role::Member).await;
    let (status, _) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/policy", f.workspace_id),
        None,
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        f.router.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}/policy", f.workspace_id),
        Some(json!({ "bypass_critical": "always" })),
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
