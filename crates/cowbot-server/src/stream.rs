//! The live event stream over a WebSocket (ADR-0045).
//!
//! A remote client subscribes once and receives the same `EventPayload` values
//! the local desktop gets from the bus, so the desktop's event handler works
//! unchanged in both modes. The socket authenticates with the same bearer token
//! a request carries, passed as the `token` query parameter because a browser
//! WebSocket handshake cannot set an `Authorization` header.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use cowbot_core::event::Event;
use cowbot_core::identity::WorkspaceId;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::error::ApiError;
use crate::router::AppState;
use crate::workspaces::require_member;

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    pub token: String,
}

/// Upgrade to the workspace event stream.
pub async fn stream(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    Query(query): Query<StreamQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = WorkspaceId::new(workspace_id);
    let user = state
        .auth
        .authenticate(&query.token)
        .await?
        .ok_or_else(|| ApiError::unauthorized("the session is expired or unknown"))?;
    require_member(&state, &user, &workspace_id).await?;
    let events = state.bus.subscribe();
    Ok(ws.on_upgrade(move |socket| pump(socket, events, state.clone(), workspace_id)))
}

/// Forward bus events to the socket.
///
/// The bus is process-wide and most payloads do not name a workspace, so the
/// socket is filtered by the channels and tasks that belong to the workspace it
/// subscribed to. The sets are loaded at subscribe time and refreshed when the
/// workspace gains a channel; a task created after subscribe belongs to a
/// workspace the client already sees, so a small delay is a correctness
/// problem only for brand-new channels, which the client also receives through
/// its own create response.
async fn pump(
    mut socket: WebSocket,
    mut events: tokio::sync::broadcast::Receiver<Event>,
    state: AppState,
    workspace_id: WorkspaceId,
) {
    let mut scope = match load_scope(&state, &workspace_id).await {
        Ok(scope) => scope,
        Err(_) => return,
    };
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    // A close, error, or no message ends the loop; a text
                    // message from the client is ignored (the stream is
                    // one-way).
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        if !scope.admit(&state, &event).await {
                            continue;
                        }
                        let Ok(text) = serde_json::to_string(&event) else {
                            continue;
                        };
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        // The subscriber fell behind; tell it to reload rather
                        // than silently skipping events.
                        let notice = serde_json::json!({ "type": "Resync", "skipped": skipped });
                        if socket
                            .send(Message::Text(notice.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                        scope = match load_scope(&state, &workspace_id).await {
                            Ok(scope) => scope,
                            Err(_) => break,
                        };
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }
}

/// The channels and tasks a workspace owns, for filtering the stream.
struct Scope {
    workspace_id: WorkspaceId,
    channels: std::collections::HashSet<String>,
    tasks: std::collections::HashSet<String>,
}

async fn load_scope(state: &AppState, workspace_id: &WorkspaceId) -> Result<Scope, ApiError> {
    let channels = state
        .store
        .list_channels(workspace_id)
        .await?
        .into_iter()
        .map(|channel| channel.id.0)
        .collect();
    let tasks = state
        .store
        .list_tasks(workspace_id)
        .await?
        .into_iter()
        .map(|task| task.id.0)
        .collect();
    Ok(Scope {
        workspace_id: workspace_id.clone(),
        channels,
        tasks,
    })
}

impl Scope {
    /// Whether an event belongs in this workspace's stream.
    ///
    /// Channel and task identifiers are checked against the workspace's sets.
    /// An event that names neither is dropped: a payload that cannot be placed
    /// is safer omitted in a shared deployment than leaked.
    async fn admit(&mut self, state: &AppState, event: &Event) -> bool {
        let Ok(encoded) = serde_json::to_value(&event.payload) else {
            return false;
        };
        // `EventPayload` is `{ "type": ..., "payload": { ... } }`, so the
        // identifiers live one level down.
        let value = encoded.get("payload").cloned().unwrap_or(encoded);
        // A channel event is placed by the channel's workspace, read live so a
        // channel created after subscribe is not mistaken for another tenant's.
        if let Some(id) = value.get("channel_id").and_then(|id| id.as_str()) {
            return self.channel_admitted(state, id).await;
        }
        if let Some(channel) = value.get("channel") {
            if let Some(id) = channel.get("id").and_then(|id| id.as_str()) {
                return self.channel_admitted(state, id).await;
            }
        }
        // A task event is placed by the task's workspace, read live for the same
        // reason: a task created after subscribe must still reach the socket.
        if let Some(id) = value.get("task_id").and_then(|id| id.as_str()) {
            return self.task_admitted(state, id).await;
        }
        if let Some(task) = value.get("task") {
            if let Some(id) = task.get("id").and_then(|id| id.as_str()) {
                return self.task_admitted(state, id).await;
            }
        }
        // `TaskCreated`, `TaskUpdated`, and `TaskCompleted` carry the task's
        // fields directly, so its id is at the top of the payload.
        if value.get("description").is_some() || value.get("expected_output").is_some() {
            if let Some(id) = value.get("id").and_then(|id| id.as_str()) {
                return self.task_admitted(state, id).await;
            }
        }
        false
    }

    async fn channel_admitted(&mut self, state: &AppState, channel_id: &str) -> bool {
        // A deleted channel's row is already gone, so the live lookup cannot
        // place it; the subscribe-time set still knows it was ours.
        if self.channels.contains(channel_id) {
            return true;
        }
        if let Some(workspace) = state
            .store
            .channel_workspace(&cowbot_core::channel::ChannelId::new(
                channel_id.to_string(),
            ))
            .await
            .ok()
            .flatten()
        {
            let mine = workspace.0 == self.workspace();
            if mine {
                self.channels.insert(channel_id.to_string());
            }
            return mine;
        }
        self.channels.contains(channel_id)
    }

    async fn task_admitted(&mut self, state: &AppState, task_id: &str) -> bool {
        if self.tasks.contains(task_id) {
            return true;
        }
        if let Some(workspace) = state
            .store
            .task_workspace(&cowbot_core::task::TaskId::new(task_id.to_string()))
            .await
            .ok()
            .flatten()
        {
            let mine = workspace.0 == self.workspace();
            if mine {
                self.tasks.insert(task_id.to_string());
            }
            return mine;
        }
        false
    }

    fn workspace(&self) -> &str {
        self.workspace_id.0.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_bus::EventBus;
    use cowbot_core::agent::AgentProfile;
    use cowbot_core::event::EventPayload;
    use cowbot_core::identity::Workspace;
    use cowbot_core::task::Task;

    async fn state() -> (AppState, Workspace, Workspace) {
        let store = cowbot_store::Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let config = crate::config::ServerConfig::default();
        let auth = std::sync::Arc::new(crate::auth::AuthService::new(store.clone(), config));
        let mine = Workspace::new("Mine");
        let other = Workspace::new("Other");
        store.create_workspace(&mine).await.unwrap();
        store.create_workspace(&other).await.unwrap();
        let state = AppState::new(store.clone(), auth, EventBus::default_bus());
        (state, mine, other)
    }

    #[tokio::test]
    async fn a_channel_event_is_placed_by_its_workspace() {
        let (state, mine, other) = state().await;
        let channels = state.channels();
        let mine_channel = channels.create_channel(&mine.id, "mine", "").await.unwrap();
        let other_channel = channels
            .create_channel(&other.id, "other", "")
            .await
            .unwrap();
        let mut scope = load_scope(&state, &mine.id).await.unwrap();

        let mine_event = Event::new(EventPayload::ChannelDeleted {
            channel_id: mine_channel.id.clone(),
        });
        assert!(scope.admit(&state, &mine_event).await);
        let other_event = Event::new(EventPayload::ChannelDeleted {
            channel_id: other_channel.id.clone(),
        });
        assert!(
            !scope.admit(&state, &other_event).await,
            "another workspace's channel must be dropped"
        );
    }

    #[tokio::test]
    async fn a_task_created_after_subscribe_still_reaches_the_socket() {
        // The admitted set starts as whatever existed at subscribe time.
        let (state, mine, other) = state().await;
        let agent = AgentProfile::new("Coder", "Engineer", "Ship", "Careful");
        state.store.upsert_agent(&mine.id, &agent).await.unwrap();
        // Subscribe before any task exists.
        let mut scope = load_scope(&state, &mine.id).await.unwrap();

        let mine_task = Task::new("t", "o").assign_to(agent.id.clone());
        state
            .store
            .create_task(&mine.id, None, &mine_task)
            .await
            .unwrap();
        let event = Event::new(EventPayload::TaskCreated(mine_task.clone()));
        assert!(
            scope.admit(&state, &event).await,
            "a task created after subscribe must be admitted"
        );

        // A task in another workspace is dropped even though it is unknown.
        state.store.upsert_agent(&other.id, &agent).await.ok();
        let other_task = Task::new("t2", "o").assign_to(agent.id.clone());
        state
            .store
            .create_task(&other.id, None, &other_task)
            .await
            .unwrap();
        let event = Event::new(EventPayload::TaskCreated(other_task));
        assert!(!scope.admit(&state, &event).await);
    }

    #[tokio::test]
    async fn an_event_that_names_no_place_is_dropped() {
        let (state, mine, _other) = state().await;
        let mut scope = load_scope(&state, &mine.id).await.unwrap();
        let agent = AgentProfile::new("A", "B", "C", "D");
        let event = Event::new(EventPayload::AgentUpdated(agent));
        assert!(!scope.admit(&state, &event).await);
    }
}
