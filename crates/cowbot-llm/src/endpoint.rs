//! URL helpers and model listing parsing shared by the LLM providers.

use cowbot_core::error::{CowError, CowResult};
use serde_json::Value;

/// Append `suffix` (for example `/messages`) to a provider base URL.
///
/// Accepts a bare host, a host with `/v1`, or an endpoint that already ends
/// with the suffix, so a pasted full endpoint URL works unchanged.
pub(crate) fn endpoint_url(base_url: &str, suffix: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with(suffix) {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{base}{suffix}")
    } else {
        format!("{base}/v1{suffix}")
    }
}

/// Model listing URL for a provider base URL.
///
/// Accepts the same forms as [`endpoint_url`]: a bare host, a host with
/// `/v1`, or the full chat endpoint URL. A full endpoint has its `/messages`
/// or `/responses` suffix trimmed before `/models` is appended.
pub(crate) fn models_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let base = base
        .strip_suffix("/messages")
        .or_else(|| base.strip_suffix("/responses"))
        .unwrap_or(base);
    endpoint_url(base, "/models")
}

/// Extract model ids from a `GET /models` response body.
///
/// OpenAI Chat Completions, OpenAI Responses, and Anthropic all return a
/// top-level `data` array whose entries carry an `id`. Entries without a
/// string id are skipped, and repeated ids collapse to their first occurrence.
pub(crate) fn parse_model_ids(payload: &Value) -> CowResult<Vec<String>> {
    let data = payload
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| CowError::Llm("Response did not contain a data array".to_string()))?;

    let mut models: Vec<String> = Vec::new();
    for entry in data {
        if let Some(id) = entry.get("id").and_then(Value::as_str) {
            let id = id.trim();
            if !id.is_empty() && !models.iter().any(|model| model == id) {
                models.push(id.to_string());
            }
        }
    }
    Ok(models)
}

/// Read a value's numeric usage field, saturating at `u32::MAX`.
///
/// Providers report counts as JSON numbers of varying width; every protocol
/// normalizes them the same way.
pub(crate) fn token_field(value: &Value, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|tokens| tokens.min(u32::MAX as u64) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn appends_the_version_prefix_when_missing() {
        assert_eq!(
            endpoint_url("https://api.anthropic.com", "/messages"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            endpoint_url("http://127.0.0.1:8788/", "/messages"),
            "http://127.0.0.1:8788/v1/messages"
        );
    }

    #[test]
    fn keeps_an_existing_version_prefix() {
        assert_eq!(
            endpoint_url("https://api.openai.com/v1", "/responses"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn accepts_a_full_endpoint_url() {
        assert_eq!(
            endpoint_url("http://127.0.0.1:8788/v1/messages", "/messages"),
            "http://127.0.0.1:8788/v1/messages"
        );
        assert_eq!(
            endpoint_url("https://api.openai.com/v1/responses/", "/responses"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn parses_openai_style_model_lists() {
        let payload = json!({
            "object": "list",
            "data": [
                {"id": "gpt-4o", "object": "model", "owned_by": "system"},
                {"id": "gpt-4o-mini", "object": "model", "owned_by": "system"}
            ]
        });

        assert_eq!(
            parse_model_ids(&payload).unwrap(),
            vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()]
        );
    }

    #[test]
    fn parses_anthropic_style_model_lists() {
        let payload = json!({
            "data": [
                {
                    "type": "model",
                    "id": "claude-sonnet-5-5",
                    "display_name": "Claude Sonnet 5.5",
                    "created_at": "2026-01-01T00:00:00Z"
                }
            ],
            "has_more": false,
            "first_id": "claude-sonnet-5-5",
            "last_id": "claude-sonnet-5-5"
        });

        assert_eq!(
            parse_model_ids(&payload).unwrap(),
            vec!["claude-sonnet-5-5".to_string()]
        );
    }

    #[test]
    fn skips_entries_without_an_id_and_collapses_duplicates() {
        let payload = json!({
            "data": [
                {"id": "model-a"},
                {"id": "model-a"},
                {"display_name": "No id"},
                {"id": "  "},
                {"id": "model-b"}
            ]
        });

        assert_eq!(
            parse_model_ids(&payload).unwrap(),
            vec!["model-a".to_string(), "model-b".to_string()]
        );
    }

    #[test]
    fn a_payload_without_data_is_an_error() {
        assert!(parse_model_ids(&json!({"object": "list"})).is_err());
    }
}
