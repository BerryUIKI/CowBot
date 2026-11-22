//! IM integration routes (ADR-0056).
//!
//! What these cover is the part that belongs to the API rather than the
//! pipeline: who may create a bot, what a read is allowed to return, and that a
//! binding cannot point outside its workspace.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use cowbot_bus::EventBus;
use cowbot_core::agent::AgentProfile;
use cowbot_core::identity::{Membership, Role, Workspace, WorkspaceId};
use cowbot_server::auth::AuthService;
use cowbot_server::config::ServerConfig;
use cowbot_server::{app, AppState};
use cowbot_store::Store;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const OWNER_PASSWORD: &str = "owner-long-password";
const BOT_TOKEN: &str = "xoxb-a-real-looking-secret";

struct Fixture {
    router: Router,
    store: Store,
    auth: Arc<AuthService>,
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
    let state = AppState::new(store.clone(), auth.clone(), EventBus::default_bus());
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

async fn join(f: &Fixture, email: &str, role: Role) -> String {
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
    f.auth.issue(&user.id).await.unwrap().token
}

async fn create_channel(f: &Fixture, name: &str) -> String {
    let (status, body) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/channels", f.workspace_id),
        Some(json!({ "name": name, "topic": "" })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    body["id"].as_str().unwrap().to_string()
}

async fn create_integration(f: &Fixture, token: &str, label: &str) -> (StatusCode, Value) {
    call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/integrations", f.workspace_id),
        Some(json!({
            "kind": "slack",
            "label": label,
            "config": { "bot_token": BOT_TOKEN },
        })),
        Some(token),
    )
    .await
}

#[tokio::test]
async fn an_owner_creates_an_integration_and_reads_it_back() {
    let f = fixture().await;
    let (status, body) = create_integration(&f, &f.owner_token, "Support bot").await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    assert_eq!(body["label"], "Support bot");
    assert_eq!(body["kind"], "slack");
    assert_eq!(body["binding_count"], 0);
    assert!(body["revoked_at"].is_null());

    let (status, list) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/integrations", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {list}");
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_read_never_returns_the_configuration() {
    // The whole point of splitting `IntegrationRecord` from `NewIntegration`.
    let f = fixture().await;
    let (_, created) = create_integration(&f, &f.owner_token, "Support bot").await;
    let id = created["id"].as_str().unwrap();

    let (_, list) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/integrations", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    let serialized = list.to_string();
    assert!(
        !serialized.contains(BOT_TOKEN),
        "the list leaked the bot token: {serialized}"
    );
    assert!(
        !serialized.contains("\"config\""),
        "the list carries a config field: {serialized}"
    );
    let _ = id;
}

#[tokio::test]
async fn a_member_can_read_but_cannot_create() {
    let f = fixture().await;
    let member = join(&f, "member@example.com", Role::Member).await;

    let (status, _) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/integrations", f.workspace_id),
        None,
        Some(&member),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = create_integration(&f, &member, "Member bot").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_non_member_cannot_see_the_workspace_has_integrations() {
    let f = fixture().await;
    let outsider = cowbot_core::identity::User::new("Outsider", "out@example.com", "");
    f.store.create_user(&outsider).await.unwrap();
    let token = f.auth.issue(&outsider.id).await.unwrap().token;

    let (status, _) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/integrations", f.workspace_id),
        None,
        Some(&token),
    )
    .await;
    // A 403 would confirm the workspace exists.
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_unknown_kind_is_rejected_with_a_named_reason() {
    let f = fixture().await;
    let (status, body) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/integrations", f.workspace_id),
        Some(json!({ "kind": "irc", "label": "Old", "config": {} })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("irc"));
}

#[tokio::test]
async fn a_configuration_that_is_not_an_object_is_refused_at_the_boundary() {
    let f = fixture().await;
    let (status, _) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/integrations", f.workspace_id),
        Some(json!({ "kind": "slack", "label": "Bot", "config": "just a string" })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_binding_is_listed_and_can_be_removed() {
    let f = fixture().await;
    let (_, created) = create_integration(&f, &f.owner_token, "Support bot").await;
    let integration_id = created["id"].as_str().unwrap();
    let channel_id = create_channel(&f, "support").await;

    let (status, binding) = call(
        f.router.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        Some(json!({ "external_id": "C024", "channel_id": channel_id })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {binding}");
    assert_eq!(binding["external_id"], "C024");

    let (_, listed) = call(
        f.router.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let (status, _) = call(
        f.router.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings/C024",
            f.workspace_id
        ),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, listed) = call(
        f.router.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert!(listed.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_binding_cannot_point_at_another_workspaces_channel() {
    // A binding is a write path into a channel. Pointing it across a tenant
    // boundary would post one team's messages into another's channel.
    let f = fixture().await;
    let (_, created) = create_integration(&f, &f.owner_token, "Support bot").await;
    let integration_id = created["id"].as_str().unwrap();

    let other = Workspace::new("Other team");
    f.store.create_workspace(&other).await.unwrap();
    let foreign = cowbot_core::channel::Channel::new("theirs", "");
    f.store.create_channel(&other.id, &foreign).await.unwrap();

    let (status, body) = call(
        f.router.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        Some(json!({ "external_id": "C999", "channel_id": foreign.id.0 })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");
}

#[tokio::test]
async fn a_binding_naming_an_agent_from_another_workspace_is_refused() {
    let f = fixture().await;
    let (_, created) = create_integration(&f, &f.owner_token, "Support bot").await;
    let integration_id = created["id"].as_str().unwrap();
    let channel_id = create_channel(&f, "support").await;

    let other = Workspace::new("Other team");
    f.store.create_workspace(&other).await.unwrap();
    let alien = AgentProfile::new("Alien", "assistant", "goal", "backstory");
    f.store.upsert_agent(&other.id, &alien).await.unwrap();

    let (status, _) = call(
        f.router.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        Some(json!({
            "external_id": "C024",
            "channel_id": channel_id,
            "agent_id": alien.id.0,
        })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_binding_may_name_an_agent_from_this_workspace() {
    let f = fixture().await;
    let (_, created) = create_integration(&f, &f.owner_token, "Support bot").await;
    let integration_id = created["id"].as_str().unwrap();
    let channel_id = create_channel(&f, "support").await;

    let agent = AgentProfile::new("CowBot", "assistant", "goal", "backstory");
    f.store
        .upsert_agent(&WorkspaceId::new(f.workspace_id.clone()), &agent)
        .await
        .unwrap();

    let (status, binding) = call(
        f.router.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        Some(json!({
            "external_id": "C024",
            "channel_id": channel_id,
            "agent_id": agent.id.0,
        })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {binding}");
    assert_eq!(binding["agent_id"], agent.id.0);
}

#[tokio::test]
async fn a_revoked_integration_stays_listed_and_keeps_its_bindings_readable() {
    let f = fixture().await;
    let (_, created) = create_integration(&f, &f.owner_token, "Support bot").await;
    let integration_id = created["id"].as_str().unwrap();
    let channel_id = create_channel(&f, "support").await;
    let _ = call(
        f.router.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        Some(json!({ "external_id": "C024", "channel_id": channel_id })),
        Some(&f.owner_token),
    )
    .await;

    let (status, _) = call(
        f.router.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}",
            f.workspace_id
        ),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, list) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/integrations", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    let entry = &list.as_array().unwrap()[0];
    assert!(!entry["revoked_at"].is_null());
    assert_eq!(entry["binding_count"], 1);

    let (status, bindings) = call(
        f.router.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bindings.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn an_integration_from_another_workspace_reads_as_missing() {
    let f = fixture().await;
    let (_, created) = create_integration(&f, &f.owner_token, "Support bot").await;
    let integration_id = created["id"].as_str().unwrap();

    // A second workspace the owner also administers.
    let (_, second) = call(
        f.router.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({ "name": "Second team" })),
        Some(&f.owner_token),
    )
    .await;
    let second_id = second["id"].as_str().unwrap();

    let (status, _) = call(
        f.router.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{second_id}/integrations/{integration_id}"),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn binding_and_revoking_are_audited() {
    let f = fixture().await;
    let (_, created) = create_integration(&f, &f.owner_token, "Support bot").await;
    let integration_id = created["id"].as_str().unwrap();
    let channel_id = create_channel(&f, "support").await;
    let _ = call(
        f.router.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        Some(json!({ "external_id": "C024", "channel_id": channel_id })),
        Some(&f.owner_token),
    )
    .await;
    let _ = call(
        f.router.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}",
            f.workspace_id
        ),
        None,
        Some(&f.owner_token),
    )
    .await;

    let (_, audit) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/audit?limit=50", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    let actions: Vec<&str> = audit
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["action"].as_str())
        .collect();
    assert!(actions.contains(&"integration.created"), "{actions:?}");
    assert!(actions.contains(&"integration.bound"), "{actions:?}");
    assert!(actions.contains(&"integration.revoked"), "{actions:?}");
}

#[tokio::test]
async fn a_platform_conversation_id_is_stored_verbatim() {
    // Slack thread ids contain dots and Feishu ids contain underscores, so a
    // binding must keep the id exactly as the platform gave it.
    let f = fixture().await;
    let (_, created) = create_integration(&f, &f.owner_token, "Support bot").await;
    let integration_id = created["id"].as_str().unwrap();
    let channel_id = create_channel(&f, "support").await;
    let external = "1712345678.000100";

    let (status, _) = call(
        f.router.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        Some(json!({ "external_id": external, "channel_id": channel_id })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let binding = f
        .store
        .integration_binding(
            &cowbot_core::integration::IntegrationId::new(integration_id),
            external,
        )
        .await
        .unwrap();
    assert!(binding.is_some(), "the id was not stored verbatim");
}

#[tokio::test]
async fn a_mattermost_bind_reports_what_the_adapter_says_about_reaching_it() {
    // The bind is the moment the operator can still act: a 201 with no verdict
    // means they bind, see success, and hear nothing until the next restart.
    let f = fixture().await;
    let (_, created) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/integrations", f.workspace_id),
        Some(json!({
            "kind": "mattermost",
            "label": "Support bot",
            // Port 1 refuses connections, so the adapter cannot confirm reach.
            "config": { "base_url": "http://127.0.0.1:1", "bot_token": "abc" },
        })),
        Some(&f.owner_token),
    )
    .await;
    let integration_id = created["id"].as_str().unwrap();
    let channel_id = create_channel(&f, "support").await;

    let (status, binding) = call(
        f.router.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        Some(json!({ "external_id": "mm-channel-1", "channel_id": channel_id })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {binding}");
    // An adapter that cannot reach its server says `unknown`, never `reachable`.
    assert_eq!(binding["health"], "unknown", "body: {binding}");
    assert!(binding["health_detail"].is_string(), "body: {binding}");
}

#[tokio::test]
async fn a_listed_binding_omits_the_verdict_rather_than_claiming_one() {
    // Checking every binding on every list would turn a cheap read into N round
    // trips to the platform; the startup notice is what covers that case.
    let f = fixture().await;
    let (_, created) = create_integration(&f, &f.owner_token, "Support bot").await;
    let integration_id = created["id"].as_str().unwrap();
    let channel_id = create_channel(&f, "support").await;
    let _ = call(
        f.router.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        Some(json!({ "external_id": "C024", "channel_id": channel_id })),
        Some(&f.owner_token),
    )
    .await;

    let (_, listed) = call(
        f.router.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        None,
        Some(&f.owner_token),
    )
    .await;
    let entry = &listed.as_array().unwrap()[0];
    assert!(entry.get("health").is_none(), "body: {entry}");
    assert!(entry.get("health_detail").is_none(), "body: {entry}");
}

#[tokio::test]
async fn a_kind_with_no_adapter_binds_without_a_verdict() {
    // Slack has no adapter in this build; the bind must still succeed, and must
    // not invent a health it never checked.
    let f = fixture().await;
    let (_, created) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/integrations", f.workspace_id),
        Some(json!({ "kind": "slack", "label": "Later", "config": {} })),
        Some(&f.owner_token),
    )
    .await;
    let integration_id = created["id"].as_str().unwrap();
    let channel_id = create_channel(&f, "support").await;

    let (status, binding) = call(
        f.router.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{}/integrations/{integration_id}/bindings",
            f.workspace_id
        ),
        Some(json!({ "external_id": "C024", "channel_id": channel_id })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {binding}");
    assert!(binding.get("health").is_none(), "body: {binding}");
}
