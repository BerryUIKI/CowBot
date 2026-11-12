use std::fmt;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::AgentId;
use crate::channel::ChannelId;
use crate::error::CowResult;
use crate::run::RunId;
use crate::task::TaskId;

/// Normalized token usage for one LLM completion.
///
/// Provider protocols report cache accounting differently: OpenAI input counts
/// include cached tokens, while Anthropic input counts exclude them. The fields
/// here are already normalized so consumers can add the three input fields
/// without knowing which provider produced the response.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input tokens the provider processed without a cache hit.
    pub uncached_input_tokens: u32,
    /// Input tokens served from a prompt cache.
    pub cached_input_tokens: u32,
    /// Input tokens written to a prompt cache by this request.
    pub cache_write_input_tokens: u32,
    /// Output tokens, including reasoning tokens when the provider reports
    /// them inside the output count.
    pub output_tokens: u32,
    /// Reasoning tokens. This is a subset of [`Self::output_tokens`] and must
    /// not be added again when computing totals.
    pub reasoning_tokens: u32,
}

impl TokenUsage {
    /// Every input token the request occupied, cached or not.
    pub fn total_input_tokens(&self) -> u32 {
        self.uncached_input_tokens
            .saturating_add(self.cached_input_tokens)
            .saturating_add(self.cache_write_input_tokens)
    }

    /// Total tokens over input and output. Reasoning is not added separately.
    pub fn total_tokens(&self) -> u32 {
        self.total_input_tokens().saturating_add(self.output_tokens)
    }
}

/// Why an LLM call was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmCallKind {
    /// A completion inside an agent's thought/tool loop.
    Agent,
    /// A routing decision for a group conversation.
    Router,
    /// A rolling or conversation summary.
    Summary,
    /// A context compaction.
    Compact,
    /// Memory extraction from a completed conversation.
    MemoryExtract,
}

impl LlmCallKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Router => "router",
            Self::Summary => "summary",
            Self::Compact => "compact",
            Self::MemoryExtract => "memory_extract",
        }
    }
}

impl fmt::Display for LlmCallKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for LlmCallKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "agent" => Ok(Self::Agent),
            "router" => Ok(Self::Router),
            "summary" => Ok(Self::Summary),
            "compact" => Ok(Self::Compact),
            "memory_extract" => Ok(Self::MemoryExtract),
            _ => Err(()),
        }
    }
}

/// Usage data a recorder receives for one completion.
#[derive(Debug, Clone)]
pub struct LlmCallUsage {
    pub model: String,
    pub call_kind: LlmCallKind,
    /// `None` when the provider omitted usage or returned an unusable object.
    pub usage: Option<TokenUsage>,
    /// The provider's original usage object, kept for diagnostics.
    pub raw_usage: Option<serde_json::Value>,
}

/// One persisted LLM call.
#[derive(Debug, Clone)]
pub struct LlmCallRecord {
    pub id: String,
    pub run_id: Option<RunId>,
    pub task_id: Option<TaskId>,
    pub channel_id: Option<ChannelId>,
    pub agent_id: Option<AgentId>,
    pub call_kind: LlmCallKind,
    pub model: String,
    pub usage: Option<TokenUsage>,
    pub raw_usage: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

impl LlmCallRecord {
    pub fn new(call_kind: LlmCallKind, model: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            run_id: None,
            task_id: None,
            channel_id: None,
            agent_id: None,
            call_kind,
            model: model.into(),
            usage: None,
            raw_usage: None,
            created_at: Utc::now(),
        }
    }
}

/// Aggregated token counts over a set of calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    /// Number of rows, including rows whose provider reported no usage.
    pub calls: u64,
    pub uncached_input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
}

impl UsageTotals {
    pub fn total_input_tokens(&self) -> u64 {
        self.uncached_input_tokens
            .saturating_add(self.cached_input_tokens)
            .saturating_add(self.cache_write_input_tokens)
    }

    pub fn total_tokens(&self) -> u64 {
        self.total_input_tokens().saturating_add(self.output_tokens)
    }
}

/// Persists usage for one LLM call.
///
/// Recording is best-effort by contract: callers log a recorder error and
/// continue. Usage is observability data, not part of the task's audit trail.
#[async_trait]
pub trait LlmUsageRecorder: Send + Sync {
    async fn record_llm_call(&self, call: &LlmCallUsage) -> CowResult<()>;
}

/// Where a token count came from.
///
/// Counts are not interchangeable: only [`Self::Tiktoken`] and
/// [`Self::Provider`] are exact for the model that produced them. A caller
/// that mixes sources in one display must label them (ADR-0035).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    /// A model-specific tokenizer counted the text.
    Tiktoken,
    /// The provider reported how many tokens it processed.
    Provider,
    /// A character heuristic estimated the count.
    #[default]
    Estimate,
}

impl TokenSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tiktoken => "tiktoken",
            Self::Provider => "provider",
            Self::Estimate => "estimate",
        }
    }

    /// True when the count is a heuristic rather than an exact tokenization.
    pub fn is_estimate(&self) -> bool {
        matches!(self, Self::Estimate)
    }
}

impl fmt::Display for TokenSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Tokens a chat request spends on message framing beyond the text itself.
///
/// Chat protocols add role and separator tokens per message. Four is a
/// conservative constant for the meter and budget checks; providers' reported
/// usage remains authoritative after a request ran (ADR-0035).
pub const MESSAGE_OVERHEAD_TOKENS: u32 = 4;

/// Counts tokens the way one model will bill a request.
///
/// Implementations are object-safe so a service can pick a counter from the
/// model name at request time. The contract is intentionally small: callers
/// count text, count a whole chat message, and label the numbers they show.
pub trait TokenCounter: Send + Sync {
    /// Number of tokens in `text`.
    fn count(&self, text: &str) -> u32;

    /// Where the counts come from.
    fn source(&self) -> TokenSource;

    /// Short label for the UI, for example `o200k_base`.
    fn label(&self) -> &'static str;

    /// Tokens one chat message costs, including framing overhead.
    fn count_message(&self, text: &str) -> u32 {
        self.count(text).saturating_add(MESSAGE_OVERHEAD_TOKENS)
    }
}

/// Character-heuristic counter for models without a licensed tokenizer.
#[derive(Debug, Clone, Copy, Default)]
pub struct EstimateTokenCounter;

impl TokenCounter for EstimateTokenCounter {
    fn count(&self, text: &str) -> u32 {
        estimate_tokens(text)
    }

    fn source(&self) -> TokenSource {
        TokenSource::Estimate
    }

    fn label(&self) -> &'static str {
        "character estimate"
    }
}

/// Shared estimator for callers that have no model-specific counter.
pub static ESTIMATE_COUNTER: EstimateTokenCounter = EstimateTokenCounter;

/// Deterministic character-based token estimate.
///
/// ASCII characters count as a quarter token, CJK characters as one token, and
/// other non-ASCII characters as half a token. This is the fallback behind
/// [`EstimateTokenCounter`] for providers without a licensed tokenizer and for
/// persisted, model-independent counts; an exact counter is preferred wherever
/// the model is known (ADR-0035).
pub fn estimate_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }

    let mut weight = 0.0_f64;
    for character in text.chars() {
        weight += if character.is_ascii() {
            0.25
        } else if is_cjk(character) {
            1.0
        } else {
            0.5
        };
    }

    let tokens = weight.ceil().max(1.0);
    tokens.min(u32::MAX as f64) as u32
}

fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x1100..=0x11FF   // Hangul Jamo
            | 0x3040..=0x30FF // Hiragana and Katakana
            | 0x3130..=0x318F // Hangul Compatibility Jamo
            | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xAC00..=0xD7AF // Hangul Syllables
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
    )
}

/// Estimate the tokens in a JSON value by serializing it deterministically.
pub fn estimate_json_tokens(value: &serde_json::Value) -> u32 {
    serde_json::to_string(value)
        .map(|text| estimate_tokens(&text))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_totals_do_not_double_count_reasoning() {
        let usage = TokenUsage {
            uncached_input_tokens: 100,
            cached_input_tokens: 50,
            cache_write_input_tokens: 25,
            output_tokens: 40,
            reasoning_tokens: 15,
        };

        assert_eq!(usage.total_input_tokens(), 175);
        assert_eq!(usage.total_tokens(), 215);
    }

    #[test]
    fn call_kind_round_trips_through_strings() {
        for kind in [
            LlmCallKind::Agent,
            LlmCallKind::Router,
            LlmCallKind::Summary,
            LlmCallKind::Compact,
            LlmCallKind::MemoryExtract,
        ] {
            assert_eq!(kind.as_str().parse::<LlmCallKind>(), Ok(kind));
            assert_eq!(kind.to_string(), kind.as_str());
        }
        assert!("unknown".parse::<LlmCallKind>().is_err());
    }

    #[test]
    fn estimator_handles_empty_ascii_and_cjk() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hello"), 2);
        assert_eq!(estimate_tokens("你好世界"), 4);
        assert_eq!(estimate_tokens("hello 世界"), 4);
        assert_eq!(estimate_tokens("é"), 1);
    }

    #[test]
    fn estimator_rounds_up_non_empty_text() {
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("ab"), 1);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn estimator_counter_reports_its_source_and_label() {
        assert_eq!(ESTIMATE_COUNTER.source(), TokenSource::Estimate);
        assert!(ESTIMATE_COUNTER.source().is_estimate());
        assert_eq!(ESTIMATE_COUNTER.label(), "character estimate");
        assert_eq!(ESTIMATE_COUNTER.count("hello"), estimate_tokens("hello"));
    }

    #[test]
    fn counting_a_message_adds_framing_overhead() {
        let text = "hello world";
        assert_eq!(
            ESTIMATE_COUNTER.count_message(text),
            ESTIMATE_COUNTER.count(text) + MESSAGE_OVERHEAD_TOKENS
        );
        assert_eq!(ESTIMATE_COUNTER.count_message(""), MESSAGE_OVERHEAD_TOKENS);
    }

    #[test]
    fn token_source_round_trips_and_defaults_to_estimate() {
        for source in [
            TokenSource::Tiktoken,
            TokenSource::Provider,
            TokenSource::Estimate,
        ] {
            let json = serde_json::to_string(&source).unwrap();
            assert_eq!(serde_json::from_str::<TokenSource>(&json).unwrap(), source);
        }
        assert_eq!(
            serde_json::from_str::<TokenSource>("\"estimate\"").unwrap(),
            TokenSource::Estimate
        );
        assert_eq!(TokenSource::default(), TokenSource::Estimate);
        assert!(!TokenSource::Tiktoken.is_estimate());
        assert_eq!(TokenSource::Tiktoken.to_string(), "tiktoken");
    }

    #[test]
    fn json_estimation_serializes_the_value() {
        let value =
            serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}});
        assert!(estimate_json_tokens(&value) > 0);
        assert_eq!(estimate_json_tokens(&serde_json::Value::Null), 1);
    }

    #[test]
    fn totals_are_saturating() {
        let usage = TokenUsage {
            uncached_input_tokens: u32::MAX,
            cached_input_tokens: 1,
            ..TokenUsage::default()
        };
        assert_eq!(usage.total_input_tokens(), u32::MAX);
    }
}
