use crate::endpoint::parse_model_ids;
use crate::provider::LlmProvider;
use crate::stream::{CancelSignal, SseDecoder};
use crate::types::{
    ChatCompletionRequest, ChatCompletionResponse, CompletionOutcome, LlmToolCall, StreamChunk,
};
use async_trait::async_trait;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::usage::TokenUsage;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, error};

/// Generic client for any OpenAI-compatible API endpoint.
#[derive(Clone)]
pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl OpenAiCompatibleProvider {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
        }
    }

    pub fn openai(api_key: String) -> Self {
        Self::new("https://api.openai.com/v1", Some(api_key))
    }

    pub fn deepseek(api_key: String) -> Self {
        Self::new("https://api.deepseek.com", Some(api_key))
    }

    pub fn ollama(base_url: Option<String>) -> Self {
        let url = base_url.unwrap_or_else(|| "http://localhost:11434/v1".to_string());
        Self::new(url, None)
    }

    fn models_url(&self) -> String {
        format!("{}/models", self.base_url)
    }
}

#[derive(Serialize)]
struct OpenAiChatRequest<'a> {
    model: &'a str,
    messages: &'a [crate::types::ChatMessage],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
}

#[derive(Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<Value>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiRawToolCall>>,
}

#[derive(Deserialize)]
struct OpenAiRawToolCall {
    id: String,
    function: OpenAiRawFunction,
}

#[derive(Deserialize)]
struct OpenAiRawFunction {
    name: String,
    arguments: String,
}

fn token_field(value: &Value, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|tokens| tokens.min(u32::MAX as u64) as u32)
}

/// Normalize an OpenAI-compatible usage object.
///
/// `prompt_tokens` includes cached tokens, so the uncached share is the
/// difference. DeepSeek additionally reports hit and miss tokens; those win
/// when present.
fn parse_usage(raw: &Value) -> Option<TokenUsage> {
    let prompt = token_field(raw, "prompt_tokens");
    let hit = token_field(raw, "prompt_cache_hit_tokens");
    let miss = token_field(raw, "prompt_cache_miss_tokens");
    let detail_cached = raw
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .map(|tokens| tokens.min(u32::MAX as u64) as u32);
    let output = token_field(raw, "completion_tokens");
    let reasoning = raw
        .pointer("/completion_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64)
        .map(|tokens| tokens.min(u32::MAX as u64) as u32);

    if prompt.is_none()
        && hit.is_none()
        && miss.is_none()
        && detail_cached.is_none()
        && output.is_none()
    {
        return None;
    }

    let mut cached = hit.or(detail_cached).unwrap_or(0);
    let mut uncached = match miss {
        Some(miss) => miss,
        None => prompt.unwrap_or(0).saturating_sub(cached),
    };
    if let Some(prompt) = prompt {
        if cached > prompt {
            cached = prompt;
            uncached = 0;
        }
    }

    Some(TokenUsage {
        uncached_input_tokens: uncached,
        cached_input_tokens: cached,
        cache_write_input_tokens: 0,
        output_tokens: output.unwrap_or(0),
        reasoning_tokens: reasoning.unwrap_or(0),
    })
}

/// Assemble the Chat Completions request body.
fn request_body(request: &ChatCompletionRequest, stream: bool) -> Value {
    // Format tools in OpenAI JSON schema format
    let tools: Vec<serde_json::Value> = request
        .tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters
                }
            })
        })
        .collect();

    serde_json::to_value(OpenAiChatRequest {
        model: &request.model,
        messages: &request.messages,
        tools,
        temperature: request.temperature,
        stream,
    })
    .unwrap_or_else(|_| json!({}))
}

/// One `tool_calls` entry of a streamed delta.
#[derive(Deserialize)]
struct OpenAiDeltaToolCall {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiDeltaFunction>,
}

#[derive(Deserialize)]
struct OpenAiDeltaFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiStreamChoice {
    #[serde(default)]
    delta: OpenAiStreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct OpenAiStreamDelta {
    #[serde(default)]
    content: Option<String>,
    /// DeepSeek and compatible servers stream thinking here.
    #[serde(default)]
    reasoning_content: Option<String>,
    /// OpenRouter-style reasoning field.
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiDeltaToolCall>>,
}

#[derive(Deserialize)]
struct OpenAiStreamEvent {
    #[serde(default)]
    choices: Vec<OpenAiStreamChoice>,
    #[serde(default)]
    usage: Option<Value>,
}

/// Accumulates streamed tool calls, which arrive as partial argument text
/// keyed by a stable `index`.
#[derive(Default)]
struct ToolCallAccumulator {
    slots: Vec<OpenAiToolCallSlot>,
}

#[derive(Default)]
struct OpenAiToolCallSlot {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    fn absorb(&mut self, deltas: &[OpenAiDeltaToolCall]) {
        for delta in deltas {
            while self.slots.len() <= delta.index {
                self.slots.push(OpenAiToolCallSlot::default());
            }
            let slot = &mut self.slots[delta.index];
            if let Some(id) = &delta.id {
                if !id.is_empty() {
                    slot.id = id.clone();
                }
            }
            if let Some(function) = &delta.function {
                if let Some(name) = &function.name {
                    slot.name.push_str(name);
                }
                if let Some(arguments) = &function.arguments {
                    slot.arguments.push_str(arguments);
                }
            }
        }
    }

    fn finish(self) -> Vec<LlmToolCall> {
        self.slots
            .into_iter()
            .filter(|slot| !slot.name.is_empty())
            .map(|slot| LlmToolCall {
                id: if slot.id.is_empty() {
                    uuid_like_id()
                } else {
                    slot.id
                },
                name: slot.name,
                arguments: serde_json::from_str(&slot.arguments)
                    .unwrap_or_else(|_| json!({ "raw": slot.arguments })),
            })
            .collect()
    }
}

/// A streamed tool call without an id still needs a stable one to pair its
/// result with; the call id is what a tool result references.
fn uuid_like_id() -> String {
    format!("call_{}", uuid::Uuid::new_v4().simple())
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        "openai-compatible"
    }

    async fn chat_complete(
        &self,
        request: &ChatCompletionRequest,
    ) -> CowResult<ChatCompletionResponse> {
        let url = format!("{}/chat/completions", self.base_url);

        let req_body = request_body(request, false);

        debug!(url = %url, model = %request.model, "Dispatching chat completion request");

        let mut http = self.client.post(&url).json(&req_body);
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

        let resp_json: OpenAiChatResponse = response
            .json()
            .await
            .map_err(|e| CowError::Llm(format!("Failed to parse response JSON: {}", e)))?;

        let OpenAiChatResponse {
            choices,
            usage: raw_usage,
        } = resp_json;
        let choice = choices
            .into_iter()
            .next()
            .ok_or_else(|| CowError::Llm("No choices in chat response".to_string()))?;

        let mut tool_calls = Vec::new();
        if let Some(raw_calls) = choice.message.tool_calls {
            for raw in raw_calls {
                let args_json = serde_json::from_str(&raw.function.arguments)
                    .unwrap_or_else(|_| json!({ "raw": raw.function.arguments }));
                tool_calls.push(LlmToolCall {
                    id: raw.id,
                    name: raw.function.name,
                    arguments: args_json,
                });
            }
        }

        Ok(ChatCompletionResponse {
            content: choice.message.content,
            tool_calls,
            finish_reason: choice.finish_reason,
            usage: raw_usage.as_ref().and_then(parse_usage),
            raw_usage,
        })
    }

    async fn chat_stream(
        &self,
        request: &ChatCompletionRequest,
        cancel: &CancelSignal,
        on_chunk: &mut (dyn FnMut(StreamChunk) + Send),
    ) -> CowResult<CompletionOutcome> {
        cancel.ensure_live()?;
        let url = format!("{}/chat/completions", self.base_url);
        // Ask for usage in the final chunk; servers that do not know the field
        // ignore it.
        let mut body = request_body(request, true);
        body["stream_options"] = json!({ "include_usage": true });

        debug!(url = %url, model = %request.model, "Streaming chat completion");

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
        let mut tools = ToolCallAccumulator::default();
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
                let Ok(event) = serde_json::from_str::<OpenAiStreamEvent>(data) else {
                    continue;
                };

                for choice in &event.choices {
                    if let Some(reasoning) = choice
                        .delta
                        .reasoning_content
                        .as_ref()
                        .or(choice.delta.reasoning.as_ref())
                    {
                        if !reasoning.is_empty() {
                            let piece = StreamChunk::Reasoning(reasoning.clone());
                            outcome.apply(piece.clone());
                            on_chunk(piece);
                        }
                    }
                    if let Some(content) = &choice.delta.content {
                        if !content.is_empty() {
                            let piece = StreamChunk::Text(content.clone());
                            outcome.apply(piece.clone());
                            on_chunk(piece);
                        }
                    }
                    if let Some(deltas) = &choice.delta.tool_calls {
                        tools.absorb(deltas);
                    }
                    if choice.finish_reason.is_some() {
                        outcome.finish_reason = choice.finish_reason.clone();
                    }
                }

                if let Some(raw) = &event.usage {
                    let piece = StreamChunk::Usage {
                        usage: parse_usage(raw),
                        raw_usage: Some(raw.clone()),
                    };
                    outcome.apply(piece.clone());
                    on_chunk(piece);
                }
            }
        }

        for call in tools.finish() {
            let piece = StreamChunk::ToolCall(call);
            outcome.apply(piece.clone());
            on_chunk(piece);
        }

        Ok(outcome)
    }

    async fn list_models(&self) -> CowResult<Vec<String>> {
        let url = self.models_url();
        debug!(url = %url, "Fetching OpenAI-compatible model list");

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

    #[test]
    fn models_url_appends_to_the_configured_base() {
        assert_eq!(
            OpenAiCompatibleProvider::new("https://api.openai.com/v1", None).models_url(),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            OpenAiCompatibleProvider::new("https://api.deepseek.com/", None).models_url(),
            "https://api.deepseek.com/models"
        );
    }

    #[test]
    fn parses_openai_usage_with_cached_tokens() {
        let usage = parse_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "total_tokens": 120,
            "prompt_tokens_details": {"cached_tokens": 40},
            "completion_tokens_details": {"reasoning_tokens": 5}
        }))
        .expect("usage should parse");

        assert_eq!(usage.uncached_input_tokens, 60);
        assert_eq!(usage.cached_input_tokens, 40);
        assert_eq!(usage.cache_write_input_tokens, 0);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.reasoning_tokens, 5);
    }

    #[test]
    fn parses_deepseek_cache_hit_and_miss_tokens() {
        let usage = parse_usage(&json!({
            "prompt_tokens": 120,
            "completion_tokens": 10,
            "prompt_cache_hit_tokens": 80,
            "prompt_cache_miss_tokens": 40
        }))
        .expect("usage should parse");

        assert_eq!(usage.uncached_input_tokens, 40);
        assert_eq!(usage.cached_input_tokens, 80);
        assert_eq!(usage.output_tokens, 10);
    }

    #[test]
    fn missing_or_malformed_usage_is_none() {
        assert!(parse_usage(&json!({})).is_none());
        assert!(parse_usage(&json!({"prompt_tokens": "many"})).is_none());
    }

    #[test]
    fn cached_tokens_larger_than_the_prompt_are_clamped() {
        let usage = parse_usage(&json!({
            "prompt_tokens": 50,
            "prompt_tokens_details": {"cached_tokens": 70}
        }))
        .expect("usage should parse");

        assert_eq!(usage.uncached_input_tokens, 0);
        assert_eq!(usage.cached_input_tokens, 50);
    }
}
