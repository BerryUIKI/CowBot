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
    workspace_id: Option<String>,
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
        workspace_id: Some(workspace_id.0.clone()),
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
        let Ok(value) = serde_json::to_value(&event.payload) else {
            return false;
        };
        if let Some(channel_id) = value.get("channel_id").and_then(|id| id.as_str()) {
            if let Some(workspace) = state
                .store
                .channel_workspace(&cowbot_core::channel::ChannelId::new(
                    channel_id.to_string(),
                ))
                .await
                .ok()
                .flatten()
            {
                return workspace.0 == self.workspace();
            }
        }
        if let Some(task_id) = value.get("task_id").and_then(|id| id.as_str()) {
            return self.tasks.contains(task_id);
        }
        if let Some(channel) = value.get("channel") {
            if let Some(id) = channel.get("id").and_then(|id| id.as_str()) {
                return self.channels.contains(id);
            }
        }
        if let Some(task) = value.get("task") {
            if let Some(id) = task.get("id").and_then(|id| id.as_str()) {
                return self.tasks.contains(id);
            }
        }
        false
    }

    fn workspace(&self) -> &str {
        self.workspace_id.as_deref().unwrap_or_default()
    }
}
