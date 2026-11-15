//! Memory, notes, conversation, template, snapshot, and package routes
//! (Phase 2 milestone 2.5).
//!
//! The point of these tests is the tenant boundary: each surface is reachable
//! only for a workspace the caller belongs to, and a channel, template, or
//! snapshot id from another workspace reads as missing.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use cowbot_core::agent::AgentProfile;
use cowbot_core::identity::{Membership, Role, User, Workspace, WorkspaceId};
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
    let state = AppState::new(
        store.clone(),
        auth.clone(),
        cowbot_bus::EventBus::default_bus(),
    );
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

/// A second workspace the fixture's owner is not a member of, with one agent.
async fn other_workspace(f: &Fixture) -> (Workspace, AgentProfile) {
    let user = User::new("Other", "other@example.com", "");
    f.store.create_user(&user).await.unwrap();
    let workspace = Workspace::new("Other Team");
    f.store.create_workspace(&workspace).await.unwrap();
    f.store
        .add_member(&Membership::new(
            user.id.clone(),
            workspace.id.clone(),
            Role::Owner,
        ))
        .await
        .unwrap();
    let agent = AgentProfile::new("Secret", "Spy", "exfiltrate", "hidden");
    f.store.upsert_agent(&workspace.id, &agent).await.unwrap();
    (workspace, agent)
}

async fn seed_agent(f: &Fixture) -> AgentProfile {
    let agent = AgentProfile::new("Writer", "Technical Writer", "Write", "Clear");
    f.store
        .upsert_agent(&WorkspaceId::new(f.workspace_id.clone()), &agent)
        .await
        .unwrap();
    agent
}

#[tokio::test]
async fn a_member_saves_and_reads_the_workspace_note() {
    let f = fixture().await;
    let (status, saved) = call(
        f.router.clone(),
        "PUT",
        &format!("/api/v1/workspaces/{}/memory/notes", f.workspace_id),
        Some(json!({ "content": "The release branch is cut from dev" })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {saved}");
    assert_eq!(
        saved["note"]["content"],
        "The release branch is cut from dev"
    );

    let (status, notes) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/memory/notes", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {notes}");
    assert_eq!(
        notes["workspace"]["content"],
        "The release branch is cut from dev"
    );
    assert!(notes["conversation"].is_null());
}

#[tokio::test]
async fn a_member_adds_lists_and_archives_a_memory_entry() {
    let f = fixture().await;
    let agent = seed_agent(&f).await;
    let entries = format!("/api/v1/workspaces/{}/memory/entries", f.workspace_id);

    let (status, added) = call(
        f.router.clone(),
        "POST",
        &entries,
        Some(json!({
            "content": "Ship on Fridays",
            "kind": "decision",
            "agent_id": agent.id.0,
            "pinned": true,
        })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {added}");
    let entry_id = added["entry"]["id"].as_str().unwrap().to_string();
    assert_eq!(added["entry"]["status"], "active");

    let (status, listed) = call(
        f.router.clone(),
        "GET",
        &entries,
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {listed}");
    assert_eq!(listed.as_array().unwrap().len(), 1);

    let (status, archived) = call(
        f.router.clone(),
        "POST",
        &format!("{entries}/{entry_id}/archive"),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {archived}");
    assert_eq!(archived["status"], "archived");
}

#[tokio::test]
async fn a_memory_entry_cannot_be_scoped_to_another_workspaces_agent() {
    let f = fixture().await;
    let (_other, victim) = other_workspace(&f).await;
    let (status, body) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/memory/entries", f.workspace_id),
        Some(json!({ "content": "leak", "agent_id": victim.id.0 })),
        Some(&f.owner_token),
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND,
        "a foreign agent must not be accepted: {status} {body}"
    );
}

#[tokio::test]
async fn a_template_is_invisible_to_another_workspace() {
    let f = fixture().await;
    let document = json!({
        "id": "team-alpha",
        "kind": "team",
        "name": "Alpha Team",
        "description": "A team",
        "version": "1.0.0",
        "agents": [{
            "name": "Alpha",
            "role": "Builder",
            "goal": "Build",
            "backstory": "Careful"
        }],
        "tasks": []
    });
    let (status, saved) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/templates", f.workspace_id),
        Some(json!({ "document": document })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {saved}");

    let (status, listed) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/templates", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {listed}");
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["id"] == "team-alpha"));

    // The other workspace's listing has the built-ins but not this one.
    let (other, _agent) = other_workspace(&f).await;
    let (status, listed) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/templates", other.id.0),
        None,
        Some(&f.owner_token),
    )
    .await;
    // The fixture's owner is not a member of `other`, so it is refused
    // outright rather than listed empty.
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {listed}");

    let foreign = f.store.list_templates(&other.id, None).await.unwrap();
    assert!(foreign.is_empty(), "the other workspace owns no templates");
}

#[tokio::test]
async fn a_conversation_cannot_be_read_through_another_workspace() {
    let f = fixture().await;
    let agent = seed_agent(&f).await;
    let (status, created) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/conversations", f.workspace_id),
        Some(json!({
            "title": "Draft",
            "agent_ids": [agent.id.0],
            "model": "mock-model",
            "provider_id": "provider-1",
        })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    let channel_id = created["id"].as_str().unwrap().to_string();

    let (status, context) = call(
        f.router.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/conversations/{channel_id}/context",
            f.workspace_id
        ),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {context}");
    assert_eq!(context["participants"].as_array().unwrap().len(), 1);

    // The same channel id under another workspace reads as missing.
    let (other, _agent) = other_workspace(&f).await;
    let (status, body) = call(
        f.router.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/conversations/{channel_id}/context",
            other.id.0
        ),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");
}

#[tokio::test]
async fn a_snapshot_settings_change_requires_administration() {
    let f = fixture().await;
    let settings = format!("/api/v1/workspaces/{}/snapshots/settings", f.workspace_id);

    let (status, body) = call(
        f.router.clone(),
        "PATCH",
        &settings,
        Some(json!({ "enabled": false, "retention_days": 3 })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["retention_days"], 3);

    // A member cannot change it.
    let member = User::new("Member", "member@example.com", "");
    f.store.create_user(&member).await.unwrap();
    f.store
        .add_member(&Membership::new(
            member.id.clone(),
            WorkspaceId::new(f.workspace_id.clone()),
            Role::Member,
        ))
        .await
        .unwrap();
    let issued = f.auth.issue(&member.id).await.unwrap();
    let (status, body) = call(
        f.router.clone(),
        "PATCH",
        &settings,
        Some(json!({ "enabled": true })),
        Some(&issued.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
}

#[tokio::test]
async fn exporting_a_workspace_returns_a_package() {
    let f = fixture().await;
    seed_agent(&f).await;
    let request = Request::builder()
        .method("GET")
        .uri(format!(
            "/api/v1/workspaces/{}/packages/export",
            f.workspace_id
        ))
        .header("authorization", format!("Bearer {}", f.owner_token))
        .body(Body::empty())
        .unwrap();
    let response = f.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let manifest = cowbot_package::inspect(&bytes).expect("a valid package");
    assert_eq!(manifest.format, cowbot_package::PACKAGE_FORMAT);
}

#[tokio::test]
async fn importing_a_package_creates_a_workspace_the_caller_owns() {
    // Build a package from a second database so its ids are unique here.
    let source = Store::connect_in_memory().await.unwrap();
    source.migrate().await.unwrap();
    let context = cowbot_service::bootstrap_local(&source).await.unwrap();
    let agent = AgentProfile::new("Imported", "Builder", "Build", "Careful");
    source
        .upsert_agent(&context.workspace.id, &agent)
        .await
        .unwrap();
    let bytes = cowbot_package::export_workspace(&source, &context.workspace.id)
        .await
        .unwrap();

    let f = fixture().await;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/packages/import")
        .header("authorization", format!("Bearer {}", f.owner_token))
        .body(Body::from(bytes))
        .unwrap();
    let response = f.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let imported = WorkspaceId::new(body["workspace_id"].as_str().unwrap());
    assert_eq!(body["agents"], 1);

    let agents = f.store.list_agents(&imported).await.unwrap();
    assert_eq!(agents.len(), 1);
    let members = f.store.list_members(&imported).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].role, Role::Owner);
}

#[tokio::test]
async fn a_damaged_package_is_rejected_without_creating_a_workspace() {
    let f = fixture().await;
    let workspaces_before = f.store.list_workspaces().await.unwrap().len();
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/packages/import")
        .header("authorization", format!("Bearer {}", f.owner_token))
        .body(Body::from("not an archive"))
        .unwrap();
    let response = f.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        f.store.list_workspaces().await.unwrap().len(),
        workspaces_before
    );
}
