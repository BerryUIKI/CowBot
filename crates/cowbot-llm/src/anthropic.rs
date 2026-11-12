//! Client for the Anthropic Messages API (`POST /v1/messages`).
//!
//! Works with the official Anthropic endpoint and with relays that expose the
//! same protocol, such as Claude Code-compatible proxies. The API key is sent
//! both as `x-api-key` (official API) and as a bearer token (relays configured
//! with `ANTHROPIC_AUTH_TOKEN`).

use crate::endpoint::{endpoint_url, models_url, parse_model_ids};
use crate::provider::LlmProvider;
use crate::stream::{CancelSignal, SseDecoder};
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, ChatMessage, ChatRole, CompletionOutcome,
    LlmToolCall, StreamChunk,
};
use async_trait::async_trait;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::usage::TokenUsage;
use futures::StreamExt;
use serde_json::{json, Value};
use tracing::{debug, error};

const ANTHROPIC_VERSION: &str = "2023-06-01";

/// `max_tokens` is required by the Messages API. Agent replies are bounded by
/// the runtime loop, so a fixed ceiling keeps provider configuration small.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Client for any Anthropic Messages-compatible API endpoint.
#[derive(Clone)]
pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl AnthropicProvider {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            api_key,
        }
    }

    fn messages_url(&self) -> String {
        endpoint_url(&self.base_url, "/messages")
    }

    fn models_url(&self) -> String {
        models_url(&self.base_url)
    }
}

/// Convert the internal chat history into an Anthropic system prompt and
/// message list.
///
/// Assistant tool calls become `tool_use` blocks, and consecutive tool results
/// collapse into a single user message of `tool_result` blocks, which is the
/// shape the Messages API expects.
fn convert_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<Value>) {
    let mut system_parts = Vec::new();
    let mut converted = Vec::new();
    let mut index = 0;

    while index < messages.len() {
        let message = &messages[index];
        match message.role {
            ChatRole::System => {
                if !message.content.is_empty() {
                    system_parts.push(message.content.clone());
                }
                index += 1;
            }
            ChatRole::User => {
                converted.push(json!({
                    "role": "user",
                    "content": &message.content,
                }));
                index += 1;
            }
            ChatRole::Assistant => {
                let mut blocks = Vec::new();
                if !message.content.is_empty() {
                    blocks.push(json!({
                        "type": "text",
                        "text": &message.content,
                    }));
                }
                if let Some(tool_calls) = &message.tool_calls {
                    for call in tool_calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": &call.id,
                            "name": &call.name,
                            "input": &call.arguments,
                        }));
                    }
                }
                if !blocks.is_empty() {
                    converted.push(json!({
                        "role": "assistant",
                        "content": blocks,
                    }));
                }
                index += 1;
            }
            ChatRole::Tool => {
                let mut blocks = Vec::new();
                while index < messages.len() && messages[index].role == ChatRole::Tool {
                    let tool_message = &messages[index];
                    blocks.push(json!({
                        "type": "tool_result",
                        "tool_use_id": tool_message.tool_call_id.clone().unwrap_or_default(),
                        "content": &tool_message.content,
                    }));
                    index += 1;
                }
                converted.push(json!({
                    "role": "user",
                    "content": blocks,
                }));
            }
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    (system, converted)
}

fn request_body(request: &ChatCompletionRequest) -> Value {
    let (system, messages) = convert_messages(&request.messages);

    let mut body = json!({
        "model": &request.model,
        "max_tokens": DEFAULT_MAX_TOKENS,
        "messages": messages,
    });

    if let Some(system) = system {
        body["system"] = Value::String(system);
    }

    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }

    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": &tool.name,
                    "description": &tool.description,
                    "input_schema": &tool.parameters,
                })
            })
            .collect();
        body["tools"] = Value::Array(tools);
    }

    body
}

/// Which kind of content block streamed text belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
    ToolUse,
}

/// Accumulates one Anthropic content block while it streams.
///
/// `tool_use` input arrives as JSON fragments in `input_json_delta` events, so
/// the arguments are only usable once `content_block_stop` closes the block.
#[derive(Default)]
struct StreamedBlock {
    kind: Option<BlockKind>,
    id: String,
    name: String,
    arguments: String,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        let url = self.messages_url();
        debug!(url = %url, model = %request.model, "Dispatching Anthropic messages request");

        let mut http = self
            .client
            .post(&url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&request_body(request));
        if let Some(key) = &self.api_key {
            http = http.header("x-api-key", key.as_str()).bearer_auth(key);
        }

        let response = http
            .send()
            .await
            .map_err(|e| CowError::Llm(format!("HTTP request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let err_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read response body".to_string());
            error!(status = %status, body = %err_text, "LLM provider returned non-200 status");
            return Err(CowError::Llm(format!(
                "API returned error status {}: {}",
                status, err_text
            )));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| CowError::Llm(format!("Failed to parse response JSON: {}", e)))?;

        parse_response(payload)
    }

    async fn chat_stream(
        &self,
        request: &ChatCompletionRequest,
        cancel: &CancelSignal,
        on_chunk: &mut (dyn FnMut(StreamChunk) + Send),
    ) -> CowResult<CompletionOutcome> {
        crate::stream::ensure_live(cancel)?;
        let url = self.messages_url();
        let mut body = request_body(request);
        body["stream"] = Value::Bool(true);
        debug!(url = %url, model = %request.model, "Streaming Anthropic messages");

        let mut http = self
            .client
            .post(&url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("accept", "text/event-stream")
            .json(&body);
        if let Some(key) = &self.api_key {
            http = http.header("x-api-key", key.as_str()).bearer_auth(key);
        }

        let response = tokio::select! {
            response = http.send() => response
                .map_err(|e| CowError::Llm(format!("HTTP request failed: {}", e)))?,
            () = cancel.cancelled() => return Err(CowError::Cancelled),
        };

        let status = response.status();
        if !status.is_success() {
            let err_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read response body".to_string());
            error!(status = %status, body = %err_text, "LLM provider returned non-200 status");
            return Err(CowError::Llm(format!(
                "API returned error status {}: {}",
                status, err_text
            )));
        }

        let mut outcome = CompletionOutcome::default();
        let mut decoder = SseDecoder::new();
        let mut block = StreamedBlock::default();
        let mut finished = false;
        let mut stream = response.bytes_stream();

        loop {
            let next = tokio::select! {
                next = stream.next() => next,
                () = cancel.cancelled() => return Err(CowError::Cancelled),
            };
            let Some(chunk) = next else {
                break;
            };
            let bytes =
                chunk.map_err(|e| CowError::Llm(format!("Streaming read failed: {}", e)))?;

            for frame in decoder.push(&bytes) {
                let data = frame.data.trim();
                if data.is_empty() {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                let event_type = frame
                    .event
                    .clone()
                    .or_else(|| {
                        event
                            .get("type")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();

                match event_type.as_str() {
                    "message_start" => {
                        if let Some(usage) = event.pointer("/message/usage") {
                            emit_usage(usage, &mut outcome, on_chunk);
                        }
                    }
                    "content_block_start" => {
                        block = StreamedBlock::default();
                        block.kind =
                            match event.pointer("/content_block/type").and_then(Value::as_str) {
                                Some("tool_use") => Some(BlockKind::ToolUse),
                                Some("thinking") => Some(BlockKind::Thinking),
                                _ => Some(BlockKind::Text),
                            };
                        if let Some(id) = event.pointer("/content_block/id").and_then(Value::as_str)
                        {
                            block.id = id.to_string();
                        }
                        if let Some(name) =
                            event.pointer("/content_block/name").and_then(Value::as_str)
                        {
                            block.name = name.to_string();
                        }
                    }
                    "content_block_delta" => {
                        let delta_type = event.pointer("/delta/type").and_then(Value::as_str);
                        match delta_type {
                            Some("input_json_delta") => {
                                if let Some(partial) =
                                    event.pointer("/delta/partial_json").and_then(Value::as_str)
                                {
                                    block.arguments.push_str(partial);
                                }
                            }
                            Some("thinking_delta") => {
                                if let Some(text) =
                                    event.pointer("/delta/thinking").and_then(Value::as_str)
                                {
                                    if !text.is_empty() {
                                        emit_text(
                                            StreamChunk::Reasoning(text.to_string()),
                                            &mut outcome,
                                            on_chunk,
                                        );
                                    }
                                }
                            }
                            _ => {
                                if let Some(text) =
                                    event.pointer("/delta/text").and_then(Value::as_str)
                                {
                                    if !text.is_empty() {
                                        let kind = block.kind.unwrap_or(BlockKind::Text);
                                        let chunk = match kind {
                                            BlockKind::Thinking => {
                                                StreamChunk::Reasoning(text.to_string())
                                            }
                                            _ => StreamChunk::Text(text.to_string()),
                                        };
                                        emit_text(chunk, &mut outcome, on_chunk);
                                    }
                                }
                            }
                        }
                    }
                    "content_block_stop" => {
                        let closed = std::mem::take(&mut block);
                        if closed.kind == Some(BlockKind::ToolUse) {
                            let call = LlmToolCall {
                                id: closed.id,
                                name: closed.name,
                                arguments: serde_json::from_str(&closed.arguments)
                                    .unwrap_or_else(|_| json!({ "raw": closed.arguments })),
                            };
                            emit_text(StreamChunk::ToolCall(call), &mut outcome, on_chunk);
                        }
                    }
                    "message_delta" => {
                        if let Some(reason) =
                            event.pointer("/delta/stop_reason").and_then(Value::as_str)
                        {
                            outcome.finish_reason = Some(reason.to_string());
                        }
                        if let Some(usage) = event.get("usage") {
                            emit_usage(usage, &mut outcome, on_chunk);
                        }
                    }
                    "message_stop" => {
                        finished = true;
                        break;
                    }
                    "error" => {
                        let message = event
                            .pointer("/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("the provider reported a streaming error");
                        return Err(CowError::Llm(message.to_string()));
                    }
                    _ => {}
                }
            }

            if finished {
                break;
            }
        }

        Ok(outcome)
    }

    async fn list_models(&self) -> CowResult<Vec<String>> {
        let url = self.models_url();
        debug!(url = %url, "Fetching Anthropic model list");

        let mut http = self
            .client
            .get(&url)
            .header("anthropic-version", ANTHROPIC_VERSION);
        if let Some(key) = &self.api_key {
            http = http.header("x-api-key", key.as_str()).bearer_auth(key);
        }

        let response = http
            .send()
            .await
            .map_err(|e| CowError::Llm(format!("HTTP request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let err_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read response body".to_string());
            error!(status = %status, body = %err_text, "Model list request returned non-200 status");
            return Err(CowError::Llm(format!(
                "API returned error status {}: {}",
                status, err_text
            )));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| CowError::Llm(format!("Failed to parse response JSON: {}", e)))?;

        parse_model_ids(&payload)
    }
}

fn parse_response(value: Value) -> CowResult<ChatCompletionResponse> {
    let raw_usage = value.get("usage").cloned();
    let blocks = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| CowError::Llm("Response did not contain a content array".to_string()))?;

    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    text_parts.push(text.to_string());
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let arguments = block.get("input").cloned().unwrap_or_else(|| json!({}));
                tool_calls.push(LlmToolCall {
                    id,
                    name,
                    arguments,
                });
            }
            _ => {}
        }
    }

    let content = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    };
    let finish_reason = value
        .get("stop_reason")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(ChatCompletionResponse {
        content,
        tool_calls,
        finish_reason,
        usage: raw_usage.as_ref().and_then(parse_usage),
        raw_usage,
    })
}

fn token_field(value: &Value, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|tokens| tokens.min(u32::MAX as u64) as u32)
}

/// Anthropic input tokens exclude cached tokens, so the three fields are
/// additive without subtraction.
fn parse_usage(raw: &Value) -> Option<TokenUsage> {
    let input = token_field(raw, "input_tokens");
    let output = token_field(raw, "output_tokens");
    let cached = token_field(raw, "cache_read_input_tokens");
    let cache_write = token_field(raw, "cache_creation_input_tokens");

    if input.is_none() && output.is_none() && cached.is_none() && cache_write.is_none() {
        return None;
    }

    Some(TokenUsage {
        uncached_input_tokens: input.unwrap_or(0),
        cached_input_tokens: cached.unwrap_or(0),
        cache_write_input_tokens: cache_write.unwrap_or(0),
        output_tokens: output.unwrap_or(0),
        reasoning_tokens: 0,
    })
}

/// Record streamed usage and forward it.
///
/// Anthropic splits usage across `message_start` (input and cache) and
/// `message_delta` (output), so each object is normalized on its own and the
/// later values replace the earlier ones.
fn emit_usage(
    raw: &Value,
    outcome: &mut CompletionOutcome,
    on_chunk: &mut (dyn FnMut(StreamChunk) + Send),
) {
    let chunk = StreamChunk::Usage {
        usage: parse_usage(raw),
        raw_usage: Some(raw.clone()),
    };
    outcome.apply(chunk.clone());
    on_chunk(chunk);
}

/// Record a streamed piece of content.
fn emit_text(
    chunk: StreamChunk,
    outcome: &mut CompletionOutcome,
    on_chunk: &mut (dyn FnMut(StreamChunk) + Send),
) {
    outcome.apply(chunk.clone());
    on_chunk(chunk);
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::tool::ToolDefinition;
    use serde_json::json;

    fn tool_call(id: &str, name: &str, arguments: Value) -> LlmToolCall {
        LlmToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
        }
    }

    #[test]
    fn system_messages_become_the_top_level_prompt() {
        let messages = vec![
            ChatMessage::system("You are CowBot."),
            ChatMessage::system("Be concise."),
            ChatMessage::user("Hello"),
        ];

        let (system, converted) = convert_messages(&messages);

        assert_eq!(system.as_deref(), Some("You are CowBot.\n\nBe concise."));
        assert_eq!(converted, vec![json!({"role": "user", "content": "Hello"})]);
    }

    #[test]
    fn assistant_tool_calls_become_tool_use_blocks() {
        let mut assistant = ChatMessage::assistant("Let me look.");
        assistant.tool_calls = Some(vec![tool_call(
            "toolu_1",
            "read_file",
            json!({"path": "a.txt"}),
        )]);
        let messages = vec![
            ChatMessage::user("Read a.txt"),
            assistant,
            ChatMessage::tool("file contents", "toolu_1"),
        ];

        let (system, converted) = convert_messages(&messages);

        assert!(system.is_none());
        assert_eq!(
            converted,
            vec![
                json!({"role": "user", "content": "Read a.txt"}),
                json!({
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "Let me look."},
                        {"type": "tool_use", "id": "toolu_1", "name": "read_file", "input": {"path": "a.txt"}}
                    ]
                }),
                json!({
                    "role": "user",
                    "content": [
                        {"type": "tool_result", "tool_use_id": "toolu_1", "content": "file contents"}
                    ]
                }),
            ]
        );
    }

    #[test]
    fn consecutive_tool_results_share_one_user_message() {
        let mut assistant = ChatMessage::assistant("");
        assistant.tool_calls = Some(vec![
            tool_call("toolu_1", "read_file", json!({"path": "a.txt"})),
            tool_call("toolu_2", "read_file", json!({"path": "b.txt"})),
        ]);
        let messages = vec![
            ChatMessage::user("Read both"),
            assistant,
            ChatMessage::tool("a", "toolu_1"),
            ChatMessage::tool("b", "toolu_2"),
        ];

        let (_, converted) = convert_messages(&messages);

        assert_eq!(
            converted[1],
            json!({
                "role": "assistant",
                "content": [
                    {"type": "tool_use", "id": "toolu_1", "name": "read_file", "input": {"path": "a.txt"}},
                    {"type": "tool_use", "id": "toolu_2", "name": "read_file", "input": {"path": "b.txt"}}
                ]
            })
        );
        assert_eq!(
            converted[2],
            json!({
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": "a"},
                    {"type": "tool_result", "tool_use_id": "toolu_2", "content": "b"}
                ]
            })
        );
    }

    #[test]
    fn tools_use_the_anthropic_input_schema_shape() {
        let request = ChatCompletionRequest {
            model: "claude-sonnet-5-5".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![ToolDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            }],
            temperature: Some(0.5),
        };

        let body = request_body(&request);

        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(
            body["tools"],
            json!([{
                "name": "read_file",
                "description": "Read a file",
                "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
            }])
        );
        assert!(body.get("system").is_none());
    }

    #[test]
    fn parses_text_and_tool_use_content() {
        let payload = json!({
            "content": [
                {"type": "text", "text": "Checking."},
                {"type": "tool_use", "id": "toolu_9", "name": "run", "input": {"cmd": "ls"}}
            ],
            "stop_reason": "tool_use"
        });

        let response = parse_response(payload).unwrap();

        assert_eq!(response.content.as_deref(), Some("Checking."));
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "toolu_9");
        assert_eq!(response.tool_calls[0].name, "run");
        assert_eq!(response.tool_calls[0].arguments, json!({"cmd": "ls"}));
        assert_eq!(response.finish_reason.as_deref(), Some("tool_use"));
    }

    #[test]
    fn parses_usage_with_cache_accounting() {
        let usage_json = json!({
            "input_tokens": 100,
            "output_tokens": 20,
            "cache_read_input_tokens": 300,
            "cache_creation_input_tokens": 50
        });
        let payload = json!({
            "content": [{"type": "text", "text": "Hello."}],
            "stop_reason": "end_turn",
            "usage": usage_json
        });

        let response = parse_response(payload).unwrap();
        let usage = response.usage.expect("usage should parse");

        assert_eq!(usage.uncached_input_tokens, 100);
        assert_eq!(usage.cached_input_tokens, 300);
        assert_eq!(usage.cache_write_input_tokens, 50);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.reasoning_tokens, 0);
        assert_eq!(response.raw_usage, Some(usage_json));
    }

    #[test]
    fn missing_content_is_an_error() {
        assert!(parse_response(json!({"stop_reason": "end_turn"})).is_err());
    }

    #[test]
    fn endpoint_accepts_base_and_full_urls() {
        let provider = AnthropicProvider::new("http://127.0.0.1:8788", None);
        assert_eq!(provider.messages_url(), "http://127.0.0.1:8788/v1/messages");
        assert_eq!(provider.models_url(), "http://127.0.0.1:8788/v1/models");

        let provider = AnthropicProvider::new("https://api.anthropic.com/v1/messages", None);
        assert_eq!(
            provider.messages_url(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(provider.models_url(), "https://api.anthropic.com/v1/models");
    }
}
