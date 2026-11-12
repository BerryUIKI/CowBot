//! The `remember` tool: an agent writes one durable memory entry (M4).
//!
//! The tool is scoped to the assignment that runs it, so the entry belongs to
//! the calling agent and to the task's channel. The scheduler registers it per
//! assignment for agents whose profile lists [`REMEMBER_TOOL_NAME`], which
//! keeps the normal tool-selection model. Its Medium risk level makes Ask mode
//! show an approval card before the write.

use async_trait::async_trait;
use cowbot_core::agent::AgentId;
use cowbot_core::channel::ChannelId;
use cowbot_core::error::CowError;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::memory::{MemoryKind, MemorySource};
use cowbot_core::tool::{Tool, ToolCall, ToolResult};
use cowbot_store::Store;
use serde_json::json;

use crate::memory::{MemoryService, NewMemory};

/// Name an agent profile lists to receive the tool.
pub const REMEMBER_TOOL_NAME: &str = "remember";

/// The tool description shown to the model.
const REMEMBER_DESCRIPTION: &str = "Save one durable fact, user preference, or decision to \
long-term memory. The entry applies to this agent in the current conversation and is recalled in \
later turns. Use a self-contained sentence. Do not store secrets or transient details.";

/// A `remember` call bound to one assignment's agent and channel.
pub struct RememberTool {
    memory: MemoryService,
    agent_id: AgentId,
    channel_id: Option<ChannelId>,
}

impl RememberTool {
    pub fn new(
        store: Store,
        workspace_id: WorkspaceId,
        agent_id: AgentId,
        channel_id: Option<ChannelId>,
    ) -> Self {
        Self {
            memory: MemoryService::new(store, workspace_id),
            agent_id,
            channel_id,
        }
    }
}

#[async_trait]
impl Tool for RememberTool {
    fn name(&self) -> &str {
        REMEMBER_TOOL_NAME
    }

    fn description(&self) -> &str {
        REMEMBER_DESCRIPTION
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "One self-contained durable statement."
                },
                "kind": {
                    "type": "string",
                    "enum": ["fact", "preference", "decision", "summary", "note"],
                    "description": "What the statement records."
                },
                "pinned": {
                    "type": "boolean",
                    "description": "Pinned entries are recalled before scored entries."
                }
            },
            "required": ["content", "kind"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, CowError> {
        let Some(content) = call
            .arguments
            .get("content")
            .and_then(|value| value.as_str())
        else {
            return Ok(ToolResult::failure(
                &call.id,
                "content is required and must be a string",
            ));
        };
        let Some(kind) = call.arguments.get("kind").and_then(|value| value.as_str()) else {
            return Ok(ToolResult::failure(
                &call.id,
                "kind is required (fact, preference, decision, summary, or note)",
            ));
        };
        let Ok(kind) = kind.parse::<MemoryKind>() else {
            return Ok(ToolResult::failure(
                &call.id,
                "kind must be one of fact, preference, decision, summary, or note",
            ));
        };
        let pinned = call
            .arguments
            .get("pinned")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);

        let new = NewMemory::user(content)
            .with_kind(kind)
            .with_scope(Some(self.agent_id.clone()), self.channel_id.clone())
            .with_pinned(pinned)
            .with_source(MemorySource::Agent);
        match self.memory.add_entry(new).await {
            Ok(saved) => Ok(ToolResult::success(
                &call.id,
                format!(
                    "Saved memory entry {}{}.",
                    saved.entry.id,
                    if saved.redacted {
                        " (credentials were redacted)"
                    } else {
                        ""
                    }
                ),
            )),
            Err(error) => Ok(ToolResult::failure(&call.id, error.to_string())),
        }
    }
}
