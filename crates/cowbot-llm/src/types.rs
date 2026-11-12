use cowbot_core::tool::ToolDefinition;
use cowbot_core::usage::TokenUsage;
use serde::{Deserialize, Serialize};

/// Role in a chat completion interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

/// An individual message in a chat history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<LlmToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::System,
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Tool,
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// Tool call emitted by an LLM completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Request sent to an LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

/// Response returned from an LLM completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<LlmToolCall>,
    pub finish_reason: Option<String>,
    /// Normalized usage, or `None` when the provider omitted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    /// The provider's original usage object, for diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_usage: Option<serde_json::Value>,
}

impl ChatCompletionResponse {
    /// A response with text only.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            usage: None,
            raw_usage: None,
        }
    }
}

/// One normalized piece of a streamed completion.
///
/// Every protocol is reduced to these four kinds, so the agent loop and the
/// turn runner never branch on the provider (ADR-0041).
#[derive(Debug, Clone)]
pub enum StreamChunk {
    /// Visible answer text.
    Text(String),
    /// Model reasoning or thinking, shown apart from the answer.
    Reasoning(String),
    /// A complete tool call. Partial arguments are assembled by the decoder.
    ToolCall(LlmToolCall),
    /// Final token usage and the provider's raw usage object.
    Usage {
        usage: Option<TokenUsage>,
        raw_usage: Option<serde_json::Value>,
    },
}

/// Everything one streamed completion produced.
#[derive(Debug, Clone, Default)]
pub struct CompletionOutcome {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<LlmToolCall>,
    pub finish_reason: Option<String>,
    pub usage: Option<TokenUsage>,
    pub raw_usage: Option<serde_json::Value>,
}

impl CompletionOutcome {
    /// Present the outcome in the same shape as a non-streamed response.
    pub fn into_response(self) -> ChatCompletionResponse {
        ChatCompletionResponse {
            content: if self.content.is_empty() {
                None
            } else {
                Some(self.content)
            },
            tool_calls: self.tool_calls,
            finish_reason: self.finish_reason,
            usage: self.usage,
            raw_usage: self.raw_usage,
        }
    }

    /// Fold one chunk into the accumulated outcome.
    pub fn apply(&mut self, chunk: StreamChunk) {
        match chunk {
            StreamChunk::Text(text) => self.content.push_str(&text),
            StreamChunk::Reasoning(text) => self.reasoning.push_str(&text),
            StreamChunk::ToolCall(call) => self.tool_calls.push(call),
            StreamChunk::Usage { usage, raw_usage } => {
                if usage.is_some() {
                    self.usage = usage;
                }
                if raw_usage.is_some() {
                    self.raw_usage = raw_usage;
                }
            }
        }
    }
}
