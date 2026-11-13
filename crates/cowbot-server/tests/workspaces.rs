//! Workspace membership, role enforcement, and invitations.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
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
    owner_id: String,
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
    let owner_id = signed_in["user"]["id"].as_str().unwrap().to_string();
    let workspace_id = signed_in["workspaces"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    Fixture {
        router,
        store,
        auth,
        owner_token,
        owner_id,
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

/// Create a second account and issue it a session, returning (id, token).
///
/// The account is not yet a member of any workspace.
async fn add_user(fixture: &Fixture, email: &str) -> (String, String) {
    let user = cowbot_core::identity::User::new("Test User", email, "");
    fixture.store.create_user(&user).await.unwrap();
    let issued = fixture.auth.issue(&user.id).await.unwrap();
    (user.id.0, issued.token)
}

/// Put an existing account into the fixture's workspace at `role`.
async fn join(fixture: &Fixture, user_id: &str, role: cowbot_core::identity::Role) {
    fixture
        .store
        .add_member(&cowbot_core::identity::Membership::new(
            cowbot_core::identity::UserId::new(user_id),
            cowbot_core::identity::WorkspaceId::new(fixture.workspace_id.clone()),
            role,
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_member_lists_the_workspace_members_but_a_stranger_cannot() {
    let f = fixture().await;
    let (status, body) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/members", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body[0]["user"]["id"], f.owner_id);
    assert_eq!(body[0]["role"], "owner");

    let (_, outsider) = add_user(&f, "stranger@example.com").await;
    let (status, body) = call(
        f.router,
        "GET",
        &format!("/api/v1/workspaces/{}/members", f.workspace_id),
        None,
        Some(&outsider),
    )
    .await;
    // A non-member gets 404, not 403, so membership cannot be probed.
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn creating_a_workspace_makes_the_creator_its_owner() {
    let f = fixture().await;
    let (status, created) = call(
        f.router.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({ "name": "Second Team" })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {created}");
    assert_eq!(created["role"], "owner");

    let (status, mine) = call(
        f.router,
        "GET",
        "/api/v1/workspaces",
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mine.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn an_admin_manages_members_but_cannot_escalate() {
    let f = fixture().await;
    let (admin_id, admin_token) = add_user(&f, "admin@example.com").await;
    let (member_id, member_token) = add_user(&f, "member@example.com").await;
    join(&f, &admin_id, cowbot_core::identity::Role::Admin).await;
    join(&f, &member_id, cowbot_core::identity::Role::Member).await;
    for (id, role) in [(&admin_id, "admin"), (&member_id, "member")] {
        let (status, _) = call(
            f.router.clone(),
            "PATCH",
            &format!("/api/v1/workspaces/{}/members/{id}", f.workspace_id),
            Some(json!({ "role": role })),
            Some(&f.owner_token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "seeding {role} failed");
    }

    // An admin may promote a member to admin? No — peer promotion is refused.
    let (status, body) = call(
        f.router.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}/members/{member_id}", f.workspace_id),
        Some(json!({ "role": "admin" })),
        Some(&admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");

    // An admin may still manage a member (demote back).
    let (status, _) = call(
        f.router.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}/members/{member_id}", f.workspace_id),
        Some(json!({ "role": "member" })),
        Some(&admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // An admin cannot touch the owner.
    let (status, _) = call(
        f.router.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            f.workspace_id, f.owner_id
        ),
        Some(json!({ "role": "member" })),
        Some(&admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A member manages nobody.
    let (status, _) = call(
        f.router,
        "PATCH",
        &format!("/api/v1/workspaces/{}/members/{member_id}", f.workspace_id),
        Some(json!({ "role": "admin" })),
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_last_owner_cannot_be_demoted_or_removed() {
    let f = fixture().await;
    let (status, body) = call(
        f.router.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            f.workspace_id, f.owner_id
        ),
        Some(json!({ "role": "member" })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("at least one owner"));

    let (status, _) = call(
        f.router,
        "DELETE",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            f.workspace_id, f.owner_id
        ),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn with_a_second_owner_the_first_can_step_down() {
    let f = fixture().await;
    let (second_id, _) = add_user(&f, "second@example.com").await;
    join(&f, &second_id, cowbot_core::identity::Role::Member).await;
    let (status, _) = call(
        f.router.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}/members/{second_id}", f.workspace_id),
        Some(json!({ "role": "owner" })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = call(
        f.router.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            f.workspace_id, f.owner_id
        ),
        Some(json!({ "role": "member" })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn an_invite_joins_the_workspace_once_and_then_is_dead() {
    let f = fixture().await;
    let (status, invite) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/invites", f.workspace_id),
        Some(json!({ "role": "member" })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {invite}");
    let token = invite["token"].as_str().unwrap().to_string();
    assert_eq!(invite["role"], "member");

    let (_, newcomer) = add_user(&f, "newcomer@example.com").await;
    let (status, joined) = call(
        f.router.clone(),
        "POST",
        "/api/v1/invites/accept",
        Some(json!({ "token": token })),
        Some(&newcomer),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {joined}");
    assert_eq!(joined["role"], "member");
    assert_eq!(joined["id"], f.workspace_id);

    // Accepting again fails: the invite is single-use.
    let (status, _) = call(
        f.router.clone(),
        "POST",
        "/api/v1/invites/accept",
        Some(json!({ "token": token })),
        Some(&newcomer),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The newcomer is now a member of the workspace.
    let (status, members) = call(
        f.router,
        "GET",
        &format!("/api/v1/workspaces/{}/members", f.workspace_id),
        None,
        Some(&newcomer),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(members.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn an_invite_cannot_grant_a_role_stronger_than_the_actor_holds() {
    let f = fixture().await;
    let (admin_id, admin_token) = add_user(&f, "admin@example.com").await;
    join(&f, &admin_id, cowbot_core::identity::Role::Admin).await;

    let (status, _) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/invites", f.workspace_id),
        Some(json!({ "role": "owner" })),
        Some(&admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // An admin can invite a member, and can review the list.
    let (status, _) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/invites", f.workspace_id),
        Some(json!({ "role": "member" })),
        Some(&admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, list) = call(
        f.router.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/invites", f.workspace_id),
        None,
        Some(&admin_token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    // The token hash never appears in the listing.
    assert!(list[0].get("token").is_none());
    assert!(list[0].get("token_hash").is_none());
}

#[tokio::test]
async fn a_member_cannot_invite_or_rename() {
    let f = fixture().await;
    let (member_id, member_token) = add_user(&f, "member@example.com").await;
    join(&f, &member_id, cowbot_core::identity::Role::Member).await;

    let (status, _) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/invites", f.workspace_id),
        Some(json!({ "role": "member" })),
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = call(
        f.router,
        "PATCH",
        &format!("/api/v1/workspaces/{}", f.workspace_id),
        Some(json!({ "name": "Renamed" })),
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_expired_invite_is_refused_and_dropped() {
    let f = fixture().await;
    let (status, invite) = call(
        f.router.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/invites", f.workspace_id),
        Some(json!({ "role": "member" })),
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let invite_id = invite["id"].as_str().unwrap().to_string();
    let token = invite["token"].as_str().unwrap().to_string();

    // Rewind the expiry, so the accept path sees it expire without waiting.
    f.store
        .set_invite_expiry(
            &invite_id,
            chrono::DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        )
        .await
        .unwrap();

    let (_, newcomer) = add_user(&f, "late@example.com").await;
    let (status, body) = call(
        f.router.clone(),
        "POST",
        "/api/v1/invites/accept",
        Some(json!({ "token": token })),
        Some(&newcomer),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(body["message"].as_str().unwrap().contains("expired"));

    // It is gone from the listing.
    let (_, list) = call(
        f.router,
        "GET",
        &format!("/api/v1/workspaces/{}/invites", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn membership_changes_are_audited() {
    let f = fixture().await;
    let (member_id, _) = add_user(&f, "member@example.com").await;
    join(&f, &member_id, cowbot_core::identity::Role::Member).await;
    call(
        f.router.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}/members/{member_id}", f.workspace_id),
        Some(json!({ "role": "admin" })),
        Some(&f.owner_token),
    )
    .await;
    let events = f
        .store
        .list_audit_events(
            &cowbot_core::identity::WorkspaceId::new(f.workspace_id.clone()),
            50,
        )
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.action == "member.role_changed"),
        "expected a role-change audit event, got {:?}",
        events.iter().map(|e| e.action.clone()).collect::<Vec<_>>()
    );
    let event = events
        .iter()
        .find(|event| event.action == "member.role_changed")
        .unwrap();
    assert_eq!(event.actor, f.owner_id);
    assert_eq!(event.source.as_deref(), Some("human"));
}

#[tokio::test]
async fn removing_a_member_keeps_their_account() {
    let f = fixture().await;
    let (member_id, member_token) = add_user(&f, "member@example.com").await;
    join(&f, &member_id, cowbot_core::identity::Role::Member).await;
    let (status, _) = call(
        f.router.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/members/{member_id}", f.workspace_id),
        None,
        Some(&f.owner_token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The account still signs in; it just no longer sees the workspace.
    let (status, _) = call(
        f.router,
        "GET",
        &format!("/api/v1/workspaces/{}/members", f.workspace_id),
        None,
        Some(&member_token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
