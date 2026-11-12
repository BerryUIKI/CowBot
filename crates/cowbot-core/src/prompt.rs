//! Read-only plans of the next model request.
//!
//! The context inspector rebuilds what a conversation participant or a task
//! run would send without writing anything: named system-prompt slots, every
//! message in order, the memory block with its recall trace, and the tool
//! schemas (ADR-0036).
//!
//! A plan is a value type. It never owns store handles and never triggers a
//! model call, so the desktop can render it directly.

use serde::{Deserialize, Serialize};

use crate::conversation::ContextProjection;
use crate::memory::RecallPreview;
use crate::tool::ToolDefinition;
use crate::usage::TokenCounter;

/// Which kind of request a plan describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// One participant of a single-agent conversation.
    Conversation,
    /// One participant of a group conversation.
    Group,
    /// The first iteration of a task run.
    TaskRun,
}

/// Role of one message in a model request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptRole {
    System,
    User,
    Assistant,
    Tool,
}

/// One named segment of a system prompt.
///
/// Slot token counts are prefix differences over the assembled prompt, so the
/// prompt slots add up to the token count of the prompt text. A slot that
/// describes a separate system message — the Plan-mode instruction — is
/// counted on its own. Sending a message costs
/// [`crate::usage::MESSAGE_OVERHEAD_TOKENS`] more in a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSlotName {
    /// The agent profile: name, role, goal, and backstory.
    Agent,
    /// Workspace, rendering, and rules context shared with every agent.
    Platform,
    /// The ordered participant roster of a group conversation.
    Roster,
    /// Which participant answers unaddressed messages.
    DefaultResponder,
    /// How peer replies appear as user messages.
    PeerConvention,
    /// The private-conversation instruction of a single conversation.
    ConversationConvention,
    /// The Plan-mode instruction of a task run.
    ModeInstruction,
}

/// One named segment of a system prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSlot {
    pub name: PromptSlotName,
    pub content: String,
    pub tokens: u32,
}

/// One message in send order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptMessage {
    pub role: PromptRole,
    pub content: String,
    /// Token cost of the message, including per-message framing overhead.
    pub tokens: u32,
    /// The context entry this message came from, when it is history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    /// True for the latest user message, the one the reply answers.
    #[serde(default)]
    pub latest: bool,
    /// Names of the tool calls carried by an assistant message.
    #[serde(default)]
    pub tool_calls: Vec<String>,
    /// Tool call this message answers, for tool results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// One tool schema sent with a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptTool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    /// Token cost of this schema when serialized on its own. Per-tool costs do
    /// not add up to the array's cost, which is [`PromptPlan::projection`]'s
    /// `tool_tokens`.
    pub tokens: u32,
}

impl PromptTool {
    /// Describe and price the tool schemas a request carries.
    ///
    /// The total prices the serialized array, matching what the request sends;
    /// each entry prices its own serialized schema.
    pub fn plan(
        counter: &dyn TokenCounter,
        definitions: &[ToolDefinition],
    ) -> (Vec<PromptTool>, u32) {
        let tools = definitions
            .iter()
            .map(|definition| {
                let tokens = serde_json::to_string(definition)
                    .map(|json| counter.count(&json))
                    .unwrap_or(0);
                PromptTool {
                    name: definition.name.clone(),
                    description: definition.description.clone(),
                    parameters: definition.parameters.clone(),
                    tokens,
                }
            })
            .collect();
        let total = serde_json::to_string(definitions)
            .map(|json| counter.count(&json))
            .unwrap_or(0);
        (tools, total)
    }
}

/// The memory block one request injects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptMemory {
    pub block: String,
    /// Token cost of the block as a system message, including framing.
    pub tokens: u32,
}

/// Everything the next request would contain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptPlan {
    pub kind: PromptKind,
    /// Model the request is counted for.
    pub model: String,
    /// Provider that serves the model, when the caller knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Named system-prompt slots in send order.
    pub slots: Vec<PromptSlot>,
    /// Messages in send order, including the system and memory messages.
    pub messages: Vec<PromptMessage>,
    /// The memory block, when one is injected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<PromptMemory>,
    /// Every memory candidate the recall considered, selected or not.
    pub recall: RecallPreview,
    /// Tool schemas sent with the request.
    pub tools: Vec<PromptTool>,
    /// The context breakdown behind the plan.
    pub projection: ContextProjection,
    pub auto_compact: bool,
    pub compact_threshold: u32,
}

impl PromptPlan {
    /// Tokens the whole request would send, including the memory message.
    pub fn request_tokens(&self) -> u32 {
        let memory = self
            .memory
            .as_ref()
            .map(|memory| memory.tokens)
            .unwrap_or(0);
        self.projection.total_tokens().saturating_add(memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::ContextProjection;
    use crate::memory::RecallPreview;
    use crate::usage::{TokenSource, ESTIMATE_COUNTER};

    fn projection(system: u32, tools: u32, history: u32, latest: u32) -> ContextProjection {
        ContextProjection {
            system_tokens: system,
            tool_tokens: tools,
            history_tokens: history,
            latest_tokens: latest,
            active_entries: 0,
            excluded_entries: 0,
            token_source: TokenSource::Estimate,
            token_counter: Some("character estimate".to_string()),
        }
    }

    fn definition(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn tool_plans_price_the_array_and_each_schema() {
        let definitions = vec![
            definition("read_file", "Read a file"),
            definition("write_file", "Write a file"),
        ];
        let (tools, total) = PromptTool::plan(&ESTIMATE_COUNTER, &definitions);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "read_file");
        assert!(tools[0].tokens > 0);
        assert!(tools[1].tokens > 0);
        assert_eq!(
            total,
            ESTIMATE_COUNTER.count(&serde_json::to_string(&definitions).unwrap())
        );

        let (empty, empty_total) = PromptTool::plan(&ESTIMATE_COUNTER, &[]);
        assert!(empty.is_empty());
        assert_eq!(empty_total, ESTIMATE_COUNTER.count("[]"));
    }

    #[test]
    fn request_tokens_add_the_memory_message_to_the_projection() {
        let recall = RecallPreview {
            enabled: true,
            query: String::new(),
            block: None,
            estimated_tokens: 0,
            budget_tokens: 1,
            notes_budget_tokens: 1,
            notes_tokens: 0,
            candidates: Vec::new(),
        };
        let plan = PromptPlan {
            kind: PromptKind::Conversation,
            model: "test-model".to_string(),
            provider_id: None,
            slots: Vec::new(),
            messages: Vec::new(),
            memory: Some(PromptMemory {
                block: "memory".to_string(),
                tokens: 7,
            }),
            recall,
            tools: Vec::new(),
            projection: projection(10, 2, 30, 5),
            auto_compact: true,
            compact_threshold: 70,
        };
        assert_eq!(plan.request_tokens(), 10 + 2 + 30 + 5 + 7);
    }

    #[test]
    fn prompt_plan_round_trips_through_serialization() {
        let plan = PromptPlan {
            kind: PromptKind::Group,
            model: "gpt-4o".to_string(),
            provider_id: Some("openai".to_string()),
            slots: vec![PromptSlot {
                name: PromptSlotName::Roster,
                content: "- Planner".to_string(),
                tokens: 3,
            }],
            messages: vec![PromptMessage {
                role: PromptRole::User,
                content: "hello".to_string(),
                tokens: 6,
                entry_id: Some("entry-1".to_string()),
                latest: true,
                tool_calls: vec!["read_file".to_string()],
                tool_call_id: None,
            }],
            memory: None,
            recall: RecallPreview {
                enabled: false,
                query: String::new(),
                block: None,
                estimated_tokens: 0,
                budget_tokens: 10,
                notes_budget_tokens: 10,
                notes_tokens: 0,
                candidates: Vec::new(),
            },
            tools: Vec::new(),
            projection: projection(1, 0, 0, 6),
            auto_compact: false,
            compact_threshold: 80,
        };
        let json = serde_json::to_string(&plan).unwrap();
        let restored: PromptPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, plan);
        assert!(json.contains("\"kind\":\"group\""));
        assert!(json.contains("\"roster\""));
    }

    #[test]
    fn roles_and_slot_names_serialize_in_snake_case() {
        assert_eq!(
            serde_json::to_value(PromptRole::System).unwrap(),
            serde_json::json!("system")
        );
        assert_eq!(
            serde_json::to_value(PromptSlotName::DefaultResponder).unwrap(),
            serde_json::json!("default_responder")
        );
        assert_eq!(
            serde_json::to_value(PromptKind::TaskRun).unwrap(),
            serde_json::json!("task_run")
        );
    }

    #[test]
    fn messages_always_serialize_an_empty_tool_call_list() {
        // The desktop types tool_calls as an array; the field must be present
        // even when a message carries no calls.
        let message = PromptMessage {
            role: PromptRole::User,
            content: "hello".to_string(),
            tokens: 6,
            entry_id: None,
            latest: true,
            tool_calls: Vec::new(),
            tool_call_id: None,
        };
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["tool_calls"], serde_json::json!([]));
    }
}
