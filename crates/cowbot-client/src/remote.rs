//! Remote mode: a `Client` over HTTP and WebSocket (ADR-0045).
//!
//! The remote client covers the workspace collaboration surface — channels,
//! messages, tasks, runs, approvals, permission rules, and audit. The parts of
//! the `Client` trait that are inherently local (worktrees, local compaction,
//! in-process crew execution) return a clear error rather than pretending to
//! work; the desktop hides them in server mode.

use async_trait::async_trait;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::approval::{Approval, ApprovalId, ApprovalLevel, BypassCritical, WorkspacePolicy};
use cowbot_core::assignment::{Assignment, AssignmentEvent, AssignmentId};
use cowbot_core::audit::AuditEvent;
use cowbot_core::channel::{Channel, ChannelId, ChannelMessage};
use cowbot_core::conversation::ChannelKind;
use cowbot_core::conversation::{
    ChannelParticipant, ConversationContext, ConversationSettings, ConversationSettingsUpdate,
    ConversationTurn, TurnId,
};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::Event;
use cowbot_core::identity::{Workspace, WorkspaceId};
use cowbot_core::memory::{MemoryEntry, MemorySettings};
use cowbot_core::permission::{PermissionRule, RuleDecision};
use cowbot_core::prompt_snapshot::{
    PromptSnapshotDetail, PromptSnapshotSettings, PromptSnapshotSummary,
};
use cowbot_core::run::{Run, RunId, ToolCallRecord};
use cowbot_core::task::{Task, TaskId};
use cowbot_core::template::TemplateDocument;
use cowbot_core::tool::ToolDefinition;
use reqwest::{Client as HttpClient, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::{
    AgentUpdate, Client, ClientSession, ClientUser, ConversationPreview, EventStream, MemoryFilter,
    MemoryNotes, MemorySave, MemorySettingsUpdate, MemoryUpdate, NewConversation, NewMemory,
    NewTask, NoteSave, RecalledMemory, SessionDecisionView, TaskRunPreview,
};

/// A client bound to one server session.
pub struct RemoteClient {
    http: HttpClient,
    base: String,
    token: String,
    workspace: Workspace,
    user: ClientUser,
}

impl std::fmt::Debug for RemoteClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is never printed.
        f.debug_struct("RemoteClient")
            .field("base", &self.base)
            .field("workspace", &self.workspace.id)
            .finish()
    }
}

/// Sign-in answer from the server.
#[derive(Debug, serde::Deserialize)]
struct SignInResponse {
    token: String,
    user: ServerUser,
    workspaces: Vec<ServerWorkspace>,
}

#[derive(Debug, serde::Deserialize)]
struct ServerUser {
    id: String,
    name: String,
    email: String,
}

#[derive(Debug, serde::Deserialize)]
struct ServerWorkspace {
    id: String,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl RemoteClient {
    /// Sign in and bind to a workspace.
    ///
    /// When `workspace_id` is `None` the first workspace the user belongs to is
    /// chosen, which is what a single-team deployment wants.
    pub async fn sign_in(
        base_url: &str,
        email: &str,
        password: &str,
        workspace_id: Option<&str>,
    ) -> CowResult<Self> {
        let base = base_url.trim_end_matches('/').to_string();
        let http = HttpClient::builder().build().map_err(|error| {
            CowError::Config(format!("the HTTP client could not start: {error}"))
        })?;
        let response = http
            .post(format!("{base}/api/v1/auth/sign-in"))
            .json(&json!({ "email": email, "password": password }))
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(map_error(status, &body));
        }
        let sign_in: SignInResponse = serde_json::from_value(body)
            .map_err(|error| CowError::Config(format!("unexpected sign-in response: {error}")))?;
        let chosen = match workspace_id {
            Some(id) => sign_in
                .workspaces
                .iter()
                .find(|workspace| workspace.id == id)
                .ok_or_else(|| {
                    CowError::Config("that workspace is not available to this account".into())
                })?,
            None => sign_in.workspaces.first().ok_or_else(|| {
                CowError::Config("this account does not belong to a workspace".into())
            })?,
        };
        Ok(Self {
            http,
            base,
            token: sign_in.token,
            workspace: Workspace {
                id: WorkspaceId(chosen.id.clone()),
                name: chosen.name.clone(),
                created_at: chosen.created_at,
            },
            user: ClientUser {
                id: cowbot_core::identity::UserId(sign_in.user.id),
                name: sign_in.user.name,
                email: sign_in.user.email,
            },
        })
    }

    /// The workspaces this account belongs to, for a picker.
    pub async fn workspaces(
        &self,
        base_url: &str,
        email: &str,
        password: &str,
    ) -> CowResult<Vec<(String, String)>> {
        let base = base_url.trim_end_matches('/');
        let http = HttpClient::new();
        let response = http
            .post(format!("{base}/api/v1/auth/sign-in"))
            .json(&json!({ "email": email, "password": password }))
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(map_error(status, &body));
        }
        let sign_in: SignInResponse = serde_json::from_value(body)
            .map_err(|error| CowError::Config(format!("unexpected sign-in response: {error}")))?;
        Ok(sign_in
            .workspaces
            .into_iter()
            .map(|workspace| (workspace.id, workspace.name))
            .collect())
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn workspace_path(&self, suffix: &str) -> String {
        format!("/api/v1/workspaces/{}{suffix}", self.workspace.id.0)
    }

    /// One request against the API.
    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: String,
        body: Option<Value>,
    ) -> CowResult<T> {
        let mut request = self
            .http
            .request(method, self.url(&path))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(transport)?;
        let status = response.status();
        let value: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(map_error(status, &value));
        }
        serde_json::from_value(value)
            .map_err(|error| CowError::Storage(format!("unexpected response shape: {error}")))
    }

    async fn send_empty(&self, method: Method, path: String) -> CowResult<()> {
        self.send_empty_with(method, path, None).await
    }

    /// Send a body and discard the response, for writes the caller does not
    /// need the server's echo of.
    async fn send_empty_with(
        &self,
        method: Method,
        path: String,
        body: Option<Value>,
    ) -> CowResult<()> {
        let mut request = self
            .http
            .request(method, self.url(&path))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(transport)?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let value: Value = response.json().await.unwrap_or(Value::Null);
        Err(map_error(status, &value))
    }
}

/// Map a transport failure into the client error type.
fn transport(error: reqwest::Error) -> CowError {
    if error.is_connect() {
        CowError::Config(format!("the server could not be reached: {error}"))
    } else {
        CowError::Storage(format!("the request failed: {error}"))
    }
}

/// Map an API error body back into a client error.
///
/// The `code` decides the variant, so a caller handling `CowError` sees the
/// same shape it would in local mode.
fn map_error(status: StatusCode, body: &Value) -> CowError {
    let code = body
        .get("code")
        .and_then(|code| code.as_str())
        .unwrap_or("");
    let message = body
        .get("message")
        .and_then(|message| message.as_str())
        .unwrap_or("the server returned an error")
        .to_string();
    match status {
        StatusCode::UNAUTHORIZED if code == "invalid_credentials" => CowError::Config(message),
        StatusCode::UNAUTHORIZED => {
            CowError::Config("the session is no longer valid; sign in again".into())
        }
        StatusCode::TOO_MANY_REQUESTS => CowError::RateLimited(message),
        StatusCode::NOT_FOUND => CowError::NotFound(message),
        StatusCode::FORBIDDEN | StatusCode::CONFLICT | StatusCode::BAD_REQUEST => {
            CowError::Config(message)
        }
        _ => CowError::Storage(message),
    }
}

/// A method that exists only in local mode.
fn local_only(what: &str) -> CowError {
    CowError::Config(format!(
        "{what} is not available in server mode; it needs the local runtime"
    ))
}

#[async_trait]
impl Client for RemoteClient {
    async fn session(&self) -> CowResult<ClientSession> {
        Ok(ClientSession {
            user: self.user.clone(),
            workspace: self.workspace.clone(),
        })
    }

    async fn update_user_profile(&self, name: &str, email: &str) -> CowResult<ClientSession> {
        // The server's account routes do not expose a profile edit yet; report
        // it plainly rather than silently doing nothing.
        let _ = (name, email);
        Err(CowError::Config(
            "profile editing is not available in server mode yet".into(),
        ))
    }

    fn subscribe(&self) -> EventStream {
        // The socket runs on a background task; the receiver is handed back
        // immediately, so a caller that never reads does not block the socket.
        let (sender, receiver) = mpsc::channel(256);
        let url = format!(
            "{}/api/v1/workspaces/{}/stream?token={}",
            self.base
                .replace("https://", "wss://")
                .replace("http://", "ws://"),
            self.workspace.id.0,
            self.token
        );
        tokio::spawn(async move {
            let Ok((mut socket, _)) = tokio_tungstenite::connect_async(url).await else {
                return;
            };
            use futures::StreamExt;
            while let Some(Ok(message)) = socket.next().await {
                let Ok(text) = message.into_text() else {
                    continue;
                };
                let Ok(event) = serde_json::from_str::<Event>(&text) else {
                    // A `Resync` notice says the subscriber fell behind. Surface
                    // it as a system notice so a screen reloads instead of
                    // going quietly stale.
                    if let Ok(notice) = serde_json::from_str::<Value>(&text) {
                        if notice.get("type").and_then(|t| t.as_str()) == Some("Resync") {
                            let skipped =
                                notice.get("skipped").and_then(|s| s.as_u64()).unwrap_or(0);
                            tracing::warn!(skipped, "the live stream fell behind");
                            let event =
                                Event::new(cowbot_core::event::EventPayload::SystemNotice {
                                    message: format!(
                                        "The live stream fell behind and skipped {skipped} events; reload to catch up."
                                    ),
                                });
                            if sender.send(event).await.is_err() {
                                break;
                            }
                        }
                    }
                    continue;
                };
                if sender.send(event).await.is_err() {
                    break;
                }
            }
        });
        EventStream::new(receiver)
    }

    async fn list_channels(&self) -> CowResult<Vec<Channel>> {
        let path = self.workspace_path("/channels");
        let bodies: Vec<ApiChannel> = self.request(Method::GET, path, None).await?;
        Ok(bodies.into_iter().map(ApiChannel::into_channel).collect())
    }

    async fn get_channel(&self, channel_id: &ChannelId) -> CowResult<Option<Channel>> {
        match self
            .request::<ApiChannel>(
                Method::GET,
                format!("/api/v1/channels/{}", channel_id.0),
                None,
            )
            .await
        {
            Ok(channel) => Ok(Some(channel.into_channel())),
            Err(CowError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn create_channel(&self, name: &str, topic: &str) -> CowResult<Channel> {
        let path = self.workspace_path("/channels");
        let body: ApiChannel = self
            .request(
                Method::POST,
                path,
                Some(json!({ "name": name, "topic": topic })),
            )
            .await?;
        Ok(body.into_channel())
    }

    async fn rename_channel(
        &self,
        channel_id: &ChannelId,
        name: &str,
        topic: &str,
    ) -> CowResult<Channel> {
        let body: ApiChannel = self
            .request(
                Method::PATCH,
                format!("/api/v1/channels/{}", channel_id.0),
                Some(json!({ "name": name, "topic": topic })),
            )
            .await?;
        Ok(body.into_channel())
    }

    async fn delete_channel(&self, channel_id: &ChannelId) -> CowResult<Channel> {
        let channel = self
            .get_channel(channel_id)
            .await?
            .ok_or_else(|| CowError::Config("the channel was not found".into()))?;
        self.send_empty(Method::DELETE, format!("/api/v1/channels/{}", channel_id.0))
            .await?;
        Ok(channel)
    }

    async fn set_channel_pinned(&self, channel_id: &ChannelId, pinned: bool) -> CowResult<Channel> {
        let body: ApiChannel = self
            .request(
                Method::PATCH,
                format!("/api/v1/channels/{}", channel_id.0),
                Some(json!({ "pinned": pinned })),
            )
            .await?;
        Ok(body.into_channel())
    }

    async fn clear_channel_messages(&self, channel_id: &ChannelId) -> CowResult<u64> {
        #[derive(serde::Deserialize)]
        struct Cleared {
            cleared: u64,
        }
        let body: Cleared = self
            .request(
                Method::POST,
                format!("/api/v1/channels/{}/messages/clear", channel_id.0),
                None,
            )
            .await?;
        Ok(body.cleared)
    }

    async fn post_message(
        &self,
        channel_id: &ChannelId,
        content: &str,
    ) -> CowResult<ChannelMessage> {
        let body: ChannelMessage = self
            .request(
                Method::POST,
                format!("/api/v1/channels/{}/messages", channel_id.0),
                Some(json!({ "content": content })),
            )
            .await?;
        Ok(body)
    }

    async fn post_thread_message(
        &self,
        channel_id: &ChannelId,
        content: &str,
        thread_id: &str,
    ) -> CowResult<ChannelMessage> {
        let body: ChannelMessage = self
            .request(
                Method::POST,
                format!("/api/v1/channels/{}/messages", channel_id.0),
                Some(json!({ "content": content, "thread_id": thread_id })),
            )
            .await?;
        Ok(body)
    }

    async fn list_messages(
        &self,
        channel_id: &ChannelId,
        limit: u32,
    ) -> CowResult<Vec<ChannelMessage>> {
        self.request(
            Method::GET,
            format!("/api/v1/channels/{}/messages?limit={limit}", channel_id.0),
            None,
        )
        .await
    }

    async fn create_conversation(&self, conversation: &NewConversation) -> CowResult<Channel> {
        let path = self.workspace_path("/conversations");
        self.request(
            Method::POST,
            path,
            Some(json!({
                "title": conversation.title,
                "agent_ids": conversation
                    .agent_ids
                    .iter()
                    .map(|id| id.0.clone())
                    .collect::<Vec<_>>(),
                "provider_id": conversation.provider_id,
                "model": conversation.model,
                "router_enabled": conversation.router_enabled,
                "default_responder": conversation
                    .default_responder
                    .as_ref()
                    .map(|id| id.0.clone()),
            })),
        )
        .await
    }

    async fn list_conversation_summaries(&self) -> CowResult<Vec<crate::ConversationSummary>> {
        let path = self.workspace_path("/conversations");
        let bodies: Vec<ServerConversationSummary> = self.request(Method::GET, path, None).await?;
        Ok(bodies
            .into_iter()
            .map(ServerConversationSummary::into_summary)
            .collect())
    }

    async fn list_conversation_participants(
        &self,
        channel_id: &ChannelId,
    ) -> CowResult<Vec<ChannelParticipant>> {
        let path = self.workspace_path(&format!("/conversations/{}/participants", channel_id.0));
        self.request(Method::GET, path, None).await
    }

    async fn list_conversation_turns(
        &self,
        channel_id: &ChannelId,
        limit: u32,
    ) -> CowResult<Vec<ConversationTurn>> {
        let path = self.workspace_path(&format!(
            "/conversations/{}/turns?limit={limit}",
            channel_id.0
        ));
        self.request(Method::GET, path, None).await
    }

    async fn conversation_settings(
        &self,
        channel_id: &ChannelId,
    ) -> CowResult<ConversationSettings> {
        let path = self.workspace_path(&format!("/conversations/{}", channel_id.0));
        self.request(Method::GET, path, None).await
    }

    async fn update_conversation_settings(
        &self,
        channel_id: &ChannelId,
        update: &ConversationSettingsUpdate,
    ) -> CowResult<ConversationSettings> {
        let path = self.workspace_path(&format!("/conversations/{}", channel_id.0));
        self.request(
            Method::PATCH,
            path,
            Some(json!({
                "auto_compact": update.auto_compact,
                "compact_threshold": update.compact_threshold,
                "router_enabled": update.router_enabled,
                "routing_mode": update.routing_mode,
                "muted_participants": update
                    .muted_participants
                    .as_ref()
                    .map(|ids| ids.iter().map(|id| id.0.clone()).collect::<Vec<_>>()),
                "reply_cleanup": update.reply_cleanup,
                "default_responder": update
                    .default_responder
                    .as_ref()
                    .map(|id| id.0.clone()),
                "provider_id": update.provider_id,
                "model": update.model,
                "model_follows_default": update.model_follows_default,
            })),
        )
        .await
    }

    async fn conversation_context(&self, channel_id: &ChannelId) -> CowResult<ConversationContext> {
        let path = self.workspace_path(&format!("/conversations/{}/context", channel_id.0));
        self.request(Method::GET, path, None).await
    }

    async fn conversation_context_preview(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
        draft: Option<&str>,
        platform_context: Option<&str>,
        plan_mode: bool,
        tools: &[ToolDefinition],
    ) -> CowResult<ConversationPreview> {
        let mut path = self.workspace_path(&format!("/conversations/{}/preview", channel_id.0));
        path.push_str(&format!(
            "?agent_id={}&plan_mode={plan_mode}&tools={}",
            agent_id.0,
            urlencode(&serde_json::to_string(tools)?)
        ));
        if let Some(draft) = draft {
            path.push_str(&format!("&draft={}", urlencode(draft)));
        }
        if let Some(context) = platform_context {
            path.push_str(&format!("&platform_context={}", urlencode(context)));
        }
        self.request(Method::GET, path, None).await
    }

    async fn task_run_context_preview(
        &self,
        run_id: &RunId,
        model: &str,
        tools: &[ToolDefinition],
    ) -> CowResult<Option<TaskRunPreview>> {
        let path = format!(
            "/api/v1/runs/{}/preview?model={}&tools={}",
            run_id.0,
            urlencode(model),
            urlencode(&serde_json::to_string(tools)?)
        );
        self.request(Method::GET, path, None).await
    }

    async fn list_run_prompt_snapshots(
        &self,
        run_id: &RunId,
    ) -> CowResult<Vec<PromptSnapshotSummary>> {
        self.request(
            Method::GET,
            format!("/api/v1/runs/{}/snapshots", run_id.0),
            None,
        )
        .await
    }

    async fn list_recent_prompt_snapshots(
        &self,
        limit: u32,
    ) -> CowResult<Vec<PromptSnapshotSummary>> {
        let path = self.workspace_path(&format!("/snapshots?limit={limit}"));
        self.request(Method::GET, path, None).await
    }

    async fn prompt_snapshot(&self, llm_call_id: &str) -> CowResult<Option<PromptSnapshotDetail>> {
        let path = self.workspace_path(&format!("/snapshots/{llm_call_id}"));
        self.request(Method::GET, path, None).await
    }

    async fn prompt_snapshot_settings(&self) -> CowResult<PromptSnapshotSettings> {
        let path = self.workspace_path("/snapshots/settings");
        self.request(Method::GET, path, None).await
    }

    async fn update_prompt_snapshot_settings(
        &self,
        enabled: Option<bool>,
        retention_days: Option<u32>,
    ) -> CowResult<PromptSnapshotSettings> {
        let path = self.workspace_path("/snapshots/settings");
        self.request(
            Method::PATCH,
            path,
            Some(json!({ "enabled": enabled, "retention_days": retention_days })),
        )
        .await
    }

    async fn purge_prompt_snapshots(&self) -> CowResult<u64> {
        let path = self.workspace_path("/snapshots/purge");
        self.request(Method::POST, path, None).await
    }

    async fn clear_conversation_context(&self, channel_id: &ChannelId) -> CowResult<u64> {
        let path = self.workspace_path(&format!("/conversations/{}/context", channel_id.0));
        self.request(Method::DELETE, path, None).await
    }

    async fn clear_conversation_participant_context(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
    ) -> CowResult<u64> {
        let path = self.workspace_path(&format!(
            "/conversations/{}/context/{}",
            channel_id.0, agent_id.0
        ));
        self.request(Method::DELETE, path, None).await
    }

    async fn memory_notes(&self, channel_id: Option<&ChannelId>) -> CowResult<MemoryNotes> {
        let mut path = self.workspace_path("/memory/notes");
        if let Some(channel_id) = channel_id {
            path.push_str(&format!("?channel_id={}", channel_id.0));
        }
        self.request(Method::GET, path, None).await
    }

    async fn save_workspace_note(&self, content: &str) -> CowResult<NoteSave> {
        let path = self.workspace_path("/memory/notes");
        self.request(Method::PUT, path, Some(json!({ "content": content })))
            .await
    }

    async fn save_conversation_note(
        &self,
        channel_id: &ChannelId,
        content: &str,
    ) -> CowResult<NoteSave> {
        let path = self.workspace_path(&format!("/memory/notes/{}", channel_id.0));
        self.request(Method::PUT, path, Some(json!({ "content": content })))
            .await
    }

    async fn delete_conversation_note(&self, channel_id: &ChannelId) -> CowResult<u64> {
        let path = self.workspace_path(&format!("/memory/notes/{}", channel_id.0));
        self.request(Method::DELETE, path, None).await
    }

    async fn memory_settings(&self) -> CowResult<MemorySettings> {
        let path = self.workspace_path("/memory/settings");
        self.request(Method::GET, path, None).await
    }

    async fn update_memory_settings(
        &self,
        update: &MemorySettingsUpdate,
    ) -> CowResult<MemorySettings> {
        let path = self.workspace_path("/memory/settings");
        self.request(
            Method::PATCH,
            path,
            Some(json!({
                "extraction_enabled": update.extraction_enabled,
                "recall_enabled": update.recall_enabled,
                "notes_budget_tokens": update.notes_budget_tokens,
                "recall_budget_tokens": update.recall_budget_tokens,
            })),
        )
        .await
    }

    async fn list_memory_entries(&self, filter: &MemoryFilter) -> CowResult<Vec<MemoryEntry>> {
        let mut path = self.workspace_path("/memory/entries");
        let mut separator = '?';
        if let Some(status) = filter.status {
            path.push_str(&format!("{separator}status={}", status.as_str()));
            separator = '&';
        }
        if let Some(agent_id) = &filter.agent_id {
            path.push_str(&format!("{separator}agent_id={}", agent_id.0));
            separator = '&';
        }
        if let Some(channel_id) = &filter.channel_id {
            path.push_str(&format!("{separator}channel_id={}", channel_id.0));
        }
        self.request(Method::GET, path, None).await
    }

    async fn add_memory_entry(&self, new: NewMemory) -> CowResult<MemorySave> {
        let path = self.workspace_path("/memory/entries");
        self.request(
            Method::POST,
            path,
            Some(json!({
                "content": new.content,
                "kind": new.kind.as_str(),
                "agent_id": new.agent_id.as_ref().map(|id| id.0.clone()),
                "channel_id": new.channel_id.as_ref().map(|id| id.0.clone()),
                "pinned": new.pinned,
            })),
        )
        .await
    }

    async fn update_memory_entry(&self, id: &str, update: &MemoryUpdate) -> CowResult<MemorySave> {
        let path = self.workspace_path(&format!("/memory/entries/{id}"));
        self.request(
            Method::PATCH,
            path,
            Some(json!({
                "content": update.content,
                "kind": update.kind.map(|kind| kind.as_str()),
                "pinned": update.pinned,
            })),
        )
        .await
    }

    async fn approve_memory_entry(&self, id: &str) -> CowResult<MemoryEntry> {
        self.set_memory_status(id, "approve").await
    }

    async fn dismiss_memory_entry(&self, id: &str) -> CowResult<MemoryEntry> {
        self.set_memory_status(id, "dismiss").await
    }

    async fn archive_memory_entry(&self, id: &str) -> CowResult<MemoryEntry> {
        self.set_memory_status(id, "archive").await
    }

    async fn delete_memory_entry(&self, id: &str) -> CowResult<u64> {
        let path = self.workspace_path(&format!("/memory/entries/{id}"));
        self.request(Method::DELETE, path, None).await
    }

    async fn preview_memory_recall(
        &self,
        channel_id: Option<&ChannelId>,
        agent_id: &AgentId,
        query: &str,
    ) -> CowResult<Option<RecalledMemory>> {
        let mut path = self.workspace_path("/memory/recall");
        path.push_str(&format!(
            "?agent_id={}&query={}",
            agent_id.0,
            urlencode(query)
        ));
        if let Some(channel_id) = channel_id {
            path.push_str(&format!("&channel_id={}", channel_id.0));
        }
        self.request(Method::GET, path, None).await
    }

    async fn create_task(&self, task: &NewTask) -> CowResult<Task> {
        let path = self.workspace_path("/tasks");
        self.request(
            Method::POST,
            path,
            Some(json!({
                "title": task.description,
                "prompt": task.expected_output,
                "assignee": task.assignee.0,
                "channel_id": task.channel_id.as_ref().map(|id| id.0.clone()),
                "approval_level": approval_level_name(task.approval_level),
            })),
        )
        .await
    }

    async fn list_tasks(&self) -> CowResult<Vec<Task>> {
        let path = self.workspace_path("/tasks");
        self.request(Method::GET, path, None).await
    }

    async fn list_run_tasks(&self, task_id: &TaskId) -> CowResult<Vec<Task>> {
        // The server returns one task at a time; a run group is a local
        // concept. Returning the task itself keeps the caller's loop working.
        Ok(self.get_task(task_id).await?.into_iter().collect())
    }

    async fn get_task(&self, task_id: &TaskId) -> CowResult<Option<Task>> {
        match self
            .request::<Task>(Method::GET, format!("/api/v1/tasks/{}", task_id.0), None)
            .await
        {
            Ok(task) => Ok(Some(task)),
            Err(CowError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn cancel_task(&self, task_id: &TaskId, _reason: &str) -> CowResult<()> {
        self.send_empty(Method::POST, format!("/api/v1/tasks/{}/cancel", task_id.0))
            .await
    }

    async fn cancel_turn(&self, _turn_id: &TurnId) -> CowResult<bool> {
        Err(local_only("stopping a live turn"))
    }

    async fn archive_task(&self, task_id: &TaskId) -> CowResult<Task> {
        self.set_archived(task_id, true).await
    }

    async fn unarchive_task(&self, task_id: &TaskId) -> CowResult<Task> {
        self.set_archived(task_id, false).await
    }

    async fn approve_task_plan(&self, task_id: &TaskId) -> CowResult<Task> {
        self.request(
            Method::POST,
            format!("/api/v1/tasks/{}/approve-plan", task_id.0),
            None,
        )
        .await
    }

    async fn list_assignments(&self, task_id: &TaskId) -> CowResult<Vec<Assignment>> {
        self.request(
            Method::GET,
            format!("/api/v1/tasks/{}/assignments", task_id.0),
            None,
        )
        .await
    }

    async fn list_runs(&self, task_id: &TaskId) -> CowResult<Vec<Run>> {
        self.request(
            Method::GET,
            format!("/api/v1/tasks/{}/runs", task_id.0),
            None,
        )
        .await
    }

    async fn get_run(&self, run_id: &RunId) -> CowResult<Option<Run>> {
        match self
            .request::<Run>(Method::GET, format!("/api/v1/runs/{}", run_id.0), None)
            .await
        {
            Ok(run) => Ok(Some(run)),
            Err(CowError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn list_tool_calls(&self, run_id: &RunId) -> CowResult<Vec<ToolCallRecord>> {
        self.request(
            Method::GET,
            format!("/api/v1/runs/{}/tool-calls", run_id.0),
            None,
        )
        .await
    }

    async fn list_assignment_events(
        &self,
        assignment_id: &AssignmentId,
    ) -> CowResult<Vec<AssignmentEvent>> {
        self.request(
            Method::GET,
            format!("/api/v1/assignments/{}/events", assignment_id.0),
            None,
        )
        .await
    }

    async fn list_audit_events(
        &self,
        limit: u32,
        source: Option<&str>,
    ) -> CowResult<Vec<AuditEvent>> {
        let mut path = format!("{}?limit={limit}", self.workspace_path("/audit"));
        if let Some(source) = source {
            path.push_str(&format!("&source={source}"));
        }
        self.request(Method::GET, path, None).await
    }

    async fn list_permission_rules(&self) -> CowResult<Vec<PermissionRule>> {
        let path = self.workspace_path("/permission-rules");
        self.request(Method::GET, path, None).await
    }

    async fn create_permission_rule(
        &self,
        tool: &str,
        command: Option<&str>,
        decision: RuleDecision,
    ) -> CowResult<PermissionRule> {
        let path = self.workspace_path("/permission-rules");
        self.request(
            Method::POST,
            path,
            Some(json!({ "tool": tool, "command": command, "decision": decision })),
        )
        .await
    }

    async fn set_permission_rule_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> CowResult<PermissionRule> {
        let path = self.workspace_path(&format!("/permission-rules/{id}"));
        self.request(Method::PATCH, path, Some(json!({ "enabled": enabled })))
            .await
    }

    async fn delete_permission_rule(&self, id: &str) -> CowResult<()> {
        self.send_empty(
            Method::DELETE,
            self.workspace_path(&format!("/permission-rules/{id}")),
        )
        .await
    }

    async fn workspace_policy(&self) -> CowResult<WorkspacePolicy> {
        let path = self.workspace_path("/policy");
        self.request(Method::GET, path, None).await
    }

    async fn set_bypass_critical(
        &self,
        bypass_critical: BypassCritical,
    ) -> CowResult<WorkspacePolicy> {
        let path = self.workspace_path("/policy");
        self.request(
            Method::PATCH,
            path,
            Some(json!({ "bypass_critical": bypass_critical })),
        )
        .await
    }

    async fn pending_approvals(&self) -> CowResult<Vec<Approval>> {
        let path = self.workspace_path("/approvals");
        self.request(Method::GET, path, None).await
    }

    async fn decide_approval(
        &self,
        approval_id: &ApprovalId,
        approved: bool,
    ) -> CowResult<Approval> {
        self.decide(approval_id, approved, false).await
    }

    async fn decide_approval_for_session(
        &self,
        approval_id: &ApprovalId,
        approved: bool,
    ) -> CowResult<Approval> {
        self.decide(approval_id, approved, true).await
    }

    async fn session_decisions(&self) -> CowResult<Vec<SessionDecisionView>> {
        Err(local_only("session decisions"))
    }

    async fn clear_session_decisions(&self) -> CowResult<()> {
        Err(local_only("session decisions"))
    }

    async fn list_templates(&self) -> CowResult<Vec<TemplateDocument>> {
        let path = self.workspace_path("/templates");
        self.request(Method::GET, path, None).await
    }

    async fn get_template(&self, id: &str) -> CowResult<Option<TemplateDocument>> {
        let path = self.workspace_path(&format!("/templates/{id}"));
        match self
            .request::<Option<TemplateDocument>>(Method::GET, path, None)
            .await
        {
            Ok(document) => Ok(document),
            Err(CowError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn save_template(&self, document: &TemplateDocument) -> CowResult<()> {
        let path = self.workspace_path("/templates");
        self.send_empty_with(Method::POST, path, Some(json!({ "document": document })))
            .await
    }

    async fn delete_template(&self, id: &str) -> CowResult<()> {
        let path = self.workspace_path(&format!("/templates/{id}"));
        self.send_empty(Method::DELETE, path).await
    }

    async fn import_template(&self, json: &str) -> CowResult<TemplateDocument> {
        let path = self.workspace_path("/templates");
        self.request(Method::POST, path, Some(json!({ "json": json })))
            .await
    }

    async fn export_template(&self, id: &str) -> CowResult<String> {
        let path = self.workspace_path(&format!("/templates/{id}/export"));
        self.request(Method::GET, path, None).await
    }

    async fn instantiate_agents(&self, template_id: &str) -> CowResult<Vec<AgentProfile>> {
        let path = self.workspace_path(&format!("/templates/{template_id}/instantiate"));
        self.request(Method::POST, path, None).await
    }

    async fn request_task_worktree(
        &self,
        _task_id: &TaskId,
        _project_id: &str,
        _project_path: &str,
        _base_branch: &str,
    ) -> CowResult<cowbot_core::worktree::TaskWorktree> {
        Err(local_only("a git worktree"))
    }

    async fn list_agents(&self) -> CowResult<Vec<AgentProfile>> {
        let path = self.workspace_path("/agents");
        self.request(Method::GET, path, None).await
    }

    async fn update_agent(&self, update: &AgentUpdate) -> CowResult<AgentProfile> {
        let path = self.workspace_path("/agents");
        self.request(
            Method::PATCH,
            path,
            Some(json!({
                "id": update.id.0,
                "name": update.name,
                "role": update.role,
                "tools": update.tools,
            })),
        )
        .await
    }
}

impl RemoteClient {
    async fn set_archived(&self, task_id: &TaskId, archived: bool) -> CowResult<Task> {
        self.request(
            Method::POST,
            format!("/api/v1/tasks/{}/archive", task_id.0),
            Some(json!({ "archived": archived })),
        )
        .await
    }

    async fn decide(
        &self,
        approval_id: &ApprovalId,
        approved: bool,
        for_session: bool,
    ) -> CowResult<Approval> {
        self.request(
            Method::POST,
            format!("/api/v1/approvals/{}/decide", approval_id.0),
            Some(json!({ "approved": approved, "for_session": for_session })),
        )
        .await
    }

    async fn set_memory_status(&self, id: &str, action: &str) -> CowResult<MemoryEntry> {
        let path = self.workspace_path(&format!("/memory/entries/{id}/{action}"));
        self.request(Method::POST, path, None).await
    }
}

/// Percent-encode a query-string value.
///
/// The recall query is free text, so anything outside the unreserved set has
/// to be escaped before it reaches the URL.
fn urlencode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            b' ' => encoded.push('+'),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// A channel as the server sends it.
#[derive(Debug, serde::Deserialize)]
struct ApiChannel {
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    workspace_id: Option<String>,
    name: String,
    topic: String,
    kind: String,
    agent_id: Option<String>,
    pinned: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    #[allow(dead_code)]
    role: Option<String>,
}

impl ApiChannel {
    fn into_channel(self) -> Channel {
        Channel {
            id: ChannelId(self.id),
            name: self.name,
            topic: self.topic,
            created_at: self.created_at,
            agent_id: self.agent_id.map(AgentId),
            pinned: self.pinned,
            kind: parse_kind(&self.kind),
        }
    }
}

/// A conversation as the server lists it, mapped onto the client summary.
#[derive(Debug, serde::Deserialize)]
struct ServerConversationSummary {
    channel: ApiChannel,
    settings: ConversationSettings,
    participants: Vec<AgentProfile>,
}

impl ServerConversationSummary {
    fn into_summary(self) -> crate::ConversationSummary {
        crate::ConversationSummary {
            channel: self.channel.into_channel(),
            settings: self.settings,
            participants: self.participants,
        }
    }
}

fn approval_level_name(level: ApprovalLevel) -> &'static str {
    match level {
        ApprovalLevel::Ask => "ask",
        ApprovalLevel::AcceptEdits => "accept-edits",
        ApprovalLevel::SafeAuto => "safe-auto",
        ApprovalLevel::PlanOnly => "plan-only",
        ApprovalLevel::Trusted => "trusted",
    }
}

fn parse_kind(value: &str) -> ChannelKind {
    match value {
        "direct" => ChannelKind::Direct,
        "conversation" => ChannelKind::Conversation,
        _ => ChannelKind::Channel,
    }
}
