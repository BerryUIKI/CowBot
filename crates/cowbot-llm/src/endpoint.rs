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

/// One model an endpoint reports, with the window it advertises.
///
/// The window is `None` when the endpoint does not say. Nothing is assumed:
/// a provider that omits it leaves the desktop's metadata empty, and the
/// context meter falls back to absolute token counts (ADR-0029).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredModel {
    pub id: String,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

/// Read a window field, accepting a JSON number or a numeric string.
///
/// Providers disagree on both the field name and the encoding, so a value of
/// `"200000"` counts as much as `200000`. Zero is treated as absent: a window
/// of nothing is not a usable denominator.
fn window_field(entry: &Value, keys: &[&str]) -> Option<u32> {
    keys.iter().find_map(|key| {
        let value = entry.get(*key)?;
        let tokens = match value {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.trim().parse::<u64>().ok(),
            _ => None,
        }?;
        (tokens > 0).then(|| tokens.min(u32::MAX as u64) as u32)
    })
}

/// Extract models from a `GET /models` response body.
///
/// OpenAI Chat Completions, OpenAI Responses, and Anthropic all return a
/// top-level `data` array whose entries carry an `id`. Entries without a
/// string id are skipped, and repeated ids collapse to their first occurrence.
///
/// Endpoints that report a context window name it differently: OpenAI-style
/// relays use `context_length` or `max_input_tokens`, some use
/// `context_window`, and a few list the selectable sizes in `context_windows`
/// in ascending order, where the largest is the model's window.
pub(crate) fn parse_discovered_models(payload: &Value) -> CowResult<Vec<DiscoveredModel>> {
    let data = payload
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| CowError::Llm("Response did not contain a data array".to_string()))?;

    let mut models: Vec<DiscoveredModel> = Vec::new();
    for entry in data {
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() || models.iter().any(|model| model.id == id) {
            continue;
        }

        let context_window = window_field(
            entry,
            &["context_length", "max_input_tokens", "context_window"],
        )
        .or_else(|| {
            entry
                .get("context_windows")
                .and_then(Value::as_array)
                .and_then(|sizes| sizes.iter().filter_map(Value::as_u64).max())
                .filter(|tokens| *tokens > 0)
                .map(|tokens| tokens.min(u32::MAX as u64) as u32)
        });

        models.push(DiscoveredModel {
            id: id.to_string(),
            context_window,
            max_output_tokens: window_field(entry, &["max_output_tokens", "max_completion_tokens"]),
        });
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

        let models = parse_discovered_models(&payload).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4o");
        assert_eq!(models[1].id, "gpt-4o-mini");
        assert_eq!(models[0].context_window, None);
        assert_eq!(models[0].max_output_tokens, None);
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

        let models = parse_discovered_models(&payload).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-sonnet-5-5");
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

        let ids: Vec<String> = parse_discovered_models(&payload)
            .unwrap()
            .into_iter()
            .map(|model| model.id)
            .collect();
        assert_eq!(ids, vec!["model-a".to_string(), "model-b".to_string()]);
    }

    #[test]
    fn a_payload_without_data_is_an_error() {
        assert!(parse_discovered_models(&json!({"object": "list"})).is_err());
    }

    #[test]
    fn reads_context_length_and_max_output_tokens() {
        let payload = json!({
            "data": [{
                "id": "deepseek-v4.1-flash",
                "context_length": 1_000_000,
                "max_input_tokens": 1_000_000,
                "max_output_tokens": 128_000
            }]
        });

        let model = &parse_discovered_models(&payload).unwrap()[0];
        assert_eq!(model.context_window, Some(1_000_000));
        assert_eq!(model.max_output_tokens, Some(128_000));
    }

    #[test]
    fn falls_back_to_max_input_tokens() {
        let payload = json!({
            "data": [{"id": "bare", "max_input_tokens": 512_000, "max_completion_tokens": 64_000}]
        });

        let model = &parse_discovered_models(&payload).unwrap()[0];
        assert_eq!(model.context_window, Some(512_000));
        assert_eq!(model.max_output_tokens, Some(64_000));
    }

    #[test]
    fn takes_the_largest_of_a_context_windows_array() {
        let payload = json!({
            "data": [{"id": "sized", "context_windows": [300_000, 600_000, 960_000]}]
        });

        assert_eq!(
            parse_discovered_models(&payload).unwrap()[0].context_window,
            Some(960_000)
        );
    }

    #[test]
    fn accepts_numeric_strings() {
        let payload = json!({
            "data": [{"id": "stringy", "context_window": "200000", "max_output_tokens": "48000"}]
        });

        let model = &parse_discovered_models(&payload).unwrap()[0];
        assert_eq!(model.context_window, Some(200_000));
        assert_eq!(model.max_output_tokens, Some(48_000));
    }

    #[test]
    fn a_zero_window_is_treated_as_absent() {
        let payload = json!({
            "data": [{"id": "zeroed", "context_length": 0, "max_output_tokens": 0}]
        });

        let model = &parse_discovered_models(&payload).unwrap()[0];
        assert_eq!(model.context_window, None);
        assert_eq!(model.max_output_tokens, None);
    }
}
