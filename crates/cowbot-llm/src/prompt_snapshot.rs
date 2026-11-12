//! Capture a provider request as an audit snapshot (ADR-0037).
//!
//! The agent runtime and the service call sites both hold a
//! [`ChatCompletionRequest`] at the moment they call a provider, so the
//! conversion lives beside the protocol types instead of being reimplemented
//! per caller. Capturing never mutates the request.

use cowbot_core::prompt::PromptRole;
use cowbot_core::prompt_snapshot::{LlmPromptSnapshot, SnapshotMessage, SnapshotToolCall};
use cowbot_core::usage::LlmCallKind;

use crate::tokens::token_counter_for_model;
use crate::types::{ChatCompletionRequest, ChatMessage, ChatRole};

/// Snapshot an outgoing request: model, call kind, messages, and tool schemas.
///
/// The result is unsealed. A caller that knows the prompt slots, the memory
/// block, or the context projection adds them here, then calls [`seal_snapshot`]
/// so redaction, bounding, and hashing happen exactly once before storage.
pub fn snapshot_request(
    model: &str,
    call_kind: LlmCallKind,
    request: &ChatCompletionRequest,
) -> LlmPromptSnapshot {
    LlmPromptSnapshot::new(call_kind, model)
        .with_messages(request.messages.iter().map(snapshot_message).collect())
        .with_tools(request.tools.clone())
}

/// Convert one chat message, including its tool calls and arguments.
pub fn snapshot_message(message: &ChatMessage) -> SnapshotMessage {
    let tool_calls = message
        .tool_calls
        .as_ref()
        .map(|calls| {
            calls
                .iter()
                .map(|call| SnapshotToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    SnapshotMessage {
        role: prompt_role(&message.role),
        content: message.content.clone(),
        name: message.name.clone(),
        tool_calls,
        tool_call_id: message.tool_call_id.clone(),
    }
}

/// Seal a snapshot with the tokenizer of the model it names.
pub fn seal_snapshot(snapshot: LlmPromptSnapshot) -> LlmPromptSnapshot {
    let counter = token_counter_for_model(&snapshot.model);
    snapshot.seal(counter.as_ref())
}

fn prompt_role(role: &ChatRole) -> PromptRole {
    match role {
        ChatRole::System => PromptRole::System,
        ChatRole::User => PromptRole::User,
        ChatRole::Assistant => PromptRole::Assistant,
        ChatRole::Tool => PromptRole::Tool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LlmToolCall;
    use serde_json::json;

    fn request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "gpt-4o".to_string(),
            messages: vec![
                ChatMessage::system("You are CowBot."),
                ChatMessage {
                    role: ChatRole::Assistant,
                    content: String::new(),
                    name: None,
                    tool_calls: Some(vec![LlmToolCall {
                        id: "call-1".to_string(),
                        name: "read_file".to_string(),
                        arguments: json!({"path": "src/main.rs"}),
                    }]),
                    tool_call_id: None,
                },
            ],
            tools: vec![cowbot_core::tool::ToolDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({"type": "object"}),
            }],
            temperature: Some(0.2),
        }
    }

    #[test]
    fn a_request_becomes_a_snapshot_of_what_it_sends() {
        let snapshot = snapshot_request("gpt-4o", LlmCallKind::Agent, &request());
        assert_eq!(snapshot.call_kind, LlmCallKind::Agent);
        assert_eq!(snapshot.model, "gpt-4o");
        assert_eq!(snapshot.messages.len(), 2);
        assert_eq!(snapshot.messages[0].role, PromptRole::System);
        assert_eq!(snapshot.messages[1].tool_calls[0].name, "read_file");
        assert_eq!(
            snapshot.messages[1].tool_calls[0].arguments,
            json!({"path": "src/main.rs"})
        );
        assert_eq!(snapshot.tools.len(), 1);
        // Capturing does not seal: the caller decides what else to attach.
        assert!(snapshot.content_hash.is_empty());
    }

    #[test]
    fn sealing_hashes_and_counts_with_the_model_counter() {
        let sealed = seal_snapshot(snapshot_request("gpt-4o", LlmCallKind::Agent, &request()));
        assert!(sealed.hash_matches());
        assert!(sealed.token_count > 0);
        assert!(!sealed.truncated);
    }
}
