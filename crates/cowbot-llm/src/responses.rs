//! Client for the OpenAI Responses API (`POST /v1/responses`).
//!
//! The Responses API is the shape used by Codex-style clients: tool calls are
//! top-level `function_call` items and tool results are `function_call_output`
//! items rather than chat messages.

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

/// Client for any OpenAI Responses-compatible API endpoint.
#[derive(Clone)]
pub struct ResponsesProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl ResponsesProvider {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into(),
            api_key,
        }
    }

    fn responses_url(&self) -> String {
        endpoint_url(&self.base_url, "/responses")
    }

    fn models_url(&self) -> String {
        models_url(&self.base_url)
    }
}

/// Convert the internal chat history into Responses `input` items.
fn convert_input(messages: &[ChatMessage]) -> Vec<Value> {
    let mut items = Vec::new();

    for message in messages {
        match message.role {
            ChatRole::System => items.push(json!({
                "role": "system",
                "content": &message.content,
            })),
            ChatRole::User => items.push(json!({
                "role": "user",
                "content": &message.content,
            })),
            ChatRole::Assistant => {
                if !message.content.is_empty() {
                    items.push(json!({
                        "role": "assistant",
                        "content": &message.content,
                    }));
                }
                if let Some(tool_calls) = &message.tool_calls {
                    for call in tool_calls {
                        let arguments = serde_json::to_string(&call.arguments)
                            .unwrap_or_else(|_| "{}".to_string());
                        items.push(json!({
                            "type": "function_call",
                            "call_id": &call.id,
                            "name": &call.name,
                            "arguments": arguments,
                        }));
                    }
                }
            }
            ChatRole::Tool => items.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id.clone().unwrap_or_default(),
                "output": &message.content,
            })),
        }
    }

    items
}

fn request_body(request: &ChatCompletionRequest) -> Value {
    let mut body = json!({
        "model": &request.model,
        "input": convert_input(&request.messages),
    });

    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }

    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": &tool.name,
                    "description": &tool.description,
                    "parameters": &tool.parameters,
                })
            })
            .collect();
        body["tools"] = Value::Array(tools);
    }

    body
}

fn parse_response(value: Value) -> CowResult<ChatCompletionResponse> {
    let raw_usage = value.get("usage").cloned();
    let output = value
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| CowError::Llm("Response did not contain an output array".to_string()))?;

    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if part.get("type").and_then(Value::as_str) == Some("output_text") {
                            if let Some(text) = part.get("text").and_then(Value::as_str) {
                                text_parts.push(text.to_string());
                            }
                        }
                    }
                }
            }
            Some("function_call") => {
                let id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or_else(|| json!({}));
                tool_calls.push(LlmToolCall {
                    id,
                    name,
                    arguments,
                });
            }
            _ => {}
        }
    }

    // Some compatible servers expose only the convenience field.
    if text_parts.is_empty() {
        if let Some(text) = value.get("output_text").and_then(Value::as_str) {
            text_parts.push(text.to_string());
        }
    }

    let content = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    };
    let finish_reason = value
        .get("status")
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

/// Responses input tokens include cached tokens, so the uncached share is the
/// difference.
fn parse_usage(raw: &Value) -> Option<TokenUsage> {
    let input = token_field(raw, "input_tokens");
    let output = token_field(raw, "output_tokens");
    let cached = raw
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .map(|tokens| tokens.min(u32::MAX as u64) as u32);
    let reasoning = raw
        .pointer("/output_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64)
        .map(|tokens| tokens.min(u32::MAX as u64) as u32);

    if input.is_none() && output.is_none() && cached.is_none() && reasoning.is_none() {
        return None;
    }

    let cached = cached.unwrap_or(0);
    let uncached = input.unwrap_or(0).saturating_sub(cached);

    Some(TokenUsage {
        uncached_input_tokens: uncached,
        cached_input_tokens: cached,
        cache_write_input_tokens: 0,
        output_tokens: output.unwrap_or(0),
        reasoning_tokens: reasoning.unwrap_or(0),
    })
}

/// A function call accumulating streamed argument fragments.
struct PendingCall {
    id: String,
    arguments: String,
}

#[async_trait]
impl LlmProvider for ResponsesProvider {
    fn name(&self) -> &str {
        "openai-responses"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        let url = self.responses_url();
        debug!(url = %url, model = %request.model, "Dispatching OpenAI responses request");

        let mut http = self.client.post(&url).json(&request_body(request));
        if let Some(key) = &self.api_key {
            http = http.bearer_auth(key);
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
        cancel.ensure_live()?;
        let url = self.responses_url();
        let mut body = request_body(request);
        body["stream"] = Value::Bool(true);
        debug!(url = %url, model = %request.model, "Streaming OpenAI responses request");

        let mut http = self
            .client
            .post(&url)
            .header("accept", "text/event-stream")
            .json(&body);
        if let Some(key) = &self.api_key {
            http = http.bearer_auth(key);
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
        // Function-call arguments stream as fragments; the call is complete at
        // `response.output_item.done`.
        let mut pending: Vec<PendingCall> = Vec::new();
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
            let bytes = chunk
                .map_err(|e| CowError::Llm(format!("Streaming read failed: {}", e)))?;

            for frame in decoder.push(&bytes) {
                let data = frame.data.trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                let event_type = frame
                    .event
                    .clone()
                    .or_else(|| event.get("type").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_default();

                match event_type.as_str() {
                    "response.output_text.delta" => {
                        if let Some(text) = event.get("delta").and_then(Value::as_str) {
                            if !text.is_empty() {
                                let piece = StreamChunk::Text(text.to_string());
                                outcome.apply(piece.clone());
                                on_chunk(piece);
                            }
                        }
                    }
                    "response.reasoning_summary_text.delta"
                    | "response.reasoning_text.delta" => {
                        if let Some(text) = event.get("delta").and_then(Value::as_str) {
                            if !text.is_empty() {
                                let piece = StreamChunk::Reasoning(text.to_string());
                                outcome.apply(piece.clone());
                                on_chunk(piece);
                            }
                        }
                    }
                    "response.output_item.added" => {
                        if let Some(item) = event.get("item") {
                            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                                pending.push(PendingCall {
                                    id: item
                                        .get("call_id")
                                        .or_else(|| item.get("id"))
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    arguments: String::new(),
                                });
                            }
                        }
                    }
                    "response.function_call_arguments.delta" => {
                        if let (Some(index), Some(partial)) = (
                            event.get("output_index").and_then(Value::as_u64),
                            event.get("delta").and_then(Value::as_str),
                        ) {
                            if let Some(slot) = pending.get_mut(index as usize) {
                                slot.arguments.push_str(partial);
                            }
                        }
                    }
                    "response.output_item.done" => {
                        if let Some(item) = event.get("item") {
                            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                                let id = item
                                    .get("call_id")
                                    .or_else(|| item.get("id"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                let arguments = item
                                    .get("arguments")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                                    .filter(|arguments| !arguments.is_empty())
                                    .or_else(|| {
                                        pending
                                            .iter()
                                            .find(|call| call.id == id)
                                            .map(|call| call.arguments.clone())
                                    })
                                    .unwrap_or_else(|| "{}".to_string());
                                let call = LlmToolCall {
                                    id,
                                    name: item
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    arguments: serde_json::from_str(&arguments)
                                        .unwrap_or_else(|_| json!({ "raw": arguments })),
                                };
                                let piece = StreamChunk::ToolCall(call);
                                outcome.apply(piece.clone());
                                on_chunk(piece);
                            }
                        }
                    }
                    "response.completed" => {
                        if let Some(response) = event.get("response") {
                            if let Some(reason) =
                                response.get("status").and_then(Value::as_str)
                            {
                                outcome.finish_reason = Some(reason.to_string());
                            }
                            if let Some(usage) = response.get("usage") {
                                let piece = StreamChunk::Usage {
                                    usage: parse_usage(usage),
                                    raw_usage: Some(usage.clone()),
                                };
                                outcome.apply(piece.clone());
                                on_chunk(piece);
                            }
                        }
                        finished = true;
                        break;
                    }
                    "response.failed" | "error" => {
                        let message = event
                            .pointer("/response/error/message")
                            .or_else(|| event.pointer("/error/message"))
                            .or_else(|| event.get("message"))
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
        debug!(url = %url, "Fetching OpenAI Responses model list");

        let mut http = self.client.get(&url);
        if let Some(key) = &self.api_key {
            http = http.bearer_auth(key);
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
    fn messages_and_function_calls_become_input_items() {
        let mut assistant = ChatMessage::assistant("Calling a tool.");
        assistant.tool_calls = Some(vec![tool_call(
            "call_1",
            "read_file",
            json!({"path": "a.txt"}),
        )]);
        let messages = vec![
            ChatMessage::system("You are CowBot."),
            ChatMessage::user("Read a.txt"),
            assistant,
            ChatMessage::tool("file contents", "call_1"),
        ];

        let items = convert_input(&messages);

        assert_eq!(
            items,
            vec![
                json!({"role": "system", "content": "You are CowBot."}),
                json!({"role": "user", "content": "Read a.txt"}),
                json!({"role": "assistant", "content": "Calling a tool."}),
                json!({
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "read_file",
                    "arguments": "{\"path\":\"a.txt\"}"
                }),
                json!({
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "file contents"
                }),
            ]
        );
    }

    #[test]
    fn tools_use_the_responses_function_shape() {
        let request = ChatCompletionRequest {
            model: "gpt-4o".to_string(),
            messages: vec![ChatMessage::user("hi")],
            tools: vec![ToolDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({"type": "object"}),
            }],
            temperature: Some(0.5),
        };

        let body = request_body(&request);

        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["temperature"], 0.5);
        assert_eq!(
            body["tools"],
            json!([{
                "type": "function",
                "name": "read_file",
                "description": "Read a file",
                "parameters": {"type": "object"}
            }])
        );
    }

    #[test]
    fn parses_output_text_and_function_calls() {
        let payload = json!({
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "Done."}]
                },
                {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_7",
                    "name": "run",
                    "arguments": "{\"cmd\":\"ls\"}"
                }
            ]
        });

        let response = parse_response(payload).unwrap();

        assert_eq!(response.content.as_deref(), Some("Done."));
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call_7");
        assert_eq!(response.tool_calls[0].name, "run");
        assert_eq!(response.tool_calls[0].arguments, json!({"cmd": "ls"}));
        assert_eq!(response.finish_reason.as_deref(), Some("completed"));
    }

    #[test]
    fn parses_usage_with_cached_and_reasoning_tokens() {
        let usage_json = json!({
            "input_tokens": 1000,
            "output_tokens": 50,
            "input_tokens_details": {"cached_tokens": 400},
            "output_tokens_details": {"reasoning_tokens": 10}
        });
        let payload = json!({
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "Hello."}]
            }],
            "usage": usage_json
        });

        let response = parse_response(payload).unwrap();
        let usage = response.usage.expect("usage should parse");

        assert_eq!(usage.uncached_input_tokens, 600);
        assert_eq!(usage.cached_input_tokens, 400);
        assert_eq!(usage.cache_write_input_tokens, 0);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.reasoning_tokens, 10);
        assert_eq!(response.raw_usage, Some(usage_json));
    }

    #[test]
    fn falls_back_to_the_convenience_output_text_field() {
        let payload = json!({"status": "completed", "output": [], "output_text": "Hello"});

        let response = parse_response(payload).unwrap();

        assert_eq!(response.content.as_deref(), Some("Hello"));
    }

    #[test]
    fn missing_output_is_an_error() {
        assert!(parse_response(json!({"status": "completed"})).is_err());
    }

    #[test]
    fn endpoint_accepts_base_and_full_urls() {
        let provider = ResponsesProvider::new("http://127.0.0.1:8788", None);
        assert_eq!(
            provider.responses_url(),
            "http://127.0.0.1:8788/v1/responses"
        );
        assert_eq!(provider.models_url(), "http://127.0.0.1:8788/v1/models");

        let provider = ResponsesProvider::new("https://api.openai.com/v1/responses", None);
        assert_eq!(
            provider.responses_url(),
            "https://api.openai.com/v1/responses"
        );
        assert_eq!(provider.models_url(), "https://api.openai.com/v1/models");
    }
}
