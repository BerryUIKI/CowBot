use crate::endpoint::parse_model_ids;
use crate::provider::LlmProvider;
use crate::types::{ChatCompletionRequest, ChatCompletionResponse, LlmToolCall};
use async_trait::async_trait;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::usage::TokenUsage;
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

        let req_body = OpenAiChatRequest {
            model: &request.model,
            messages: &request.messages,
            tools,
            temperature: request.temperature,
        };

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
