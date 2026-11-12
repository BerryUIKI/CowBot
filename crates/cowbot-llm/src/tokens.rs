//! Model-aware token counting.
//!
//! `tiktoken-rs` compiles in the OpenAI BPE vocabularies, so requests for the
//! OpenAI-compatible families that use `o200k_base` or `cl100k_base` can be
//! counted exactly without a network call. Everything else — Anthropic,
//! DeepSeek, local runtimes, and unrecognized names — falls back to the
//! character estimator, because bundling another vendor's vocabulary is a
//! licensing and maintenance risk (ADR-0035).

use std::sync::{Arc, OnceLock};

use cowbot_core::usage::{EstimateTokenCounter, TokenCounter, TokenSource};
use tiktoken_rs::tokenizer::{get_tokenizer, Tokenizer};
use tiktoken_rs::CoreBPE;

/// A tokenizer with a licensed, compiled-in implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTokenizer {
    /// `o200k_base`: GPT-5, GPT-4.1, GPT-4o, and the o-series.
    O200kBase,
    /// `cl100k_base`: GPT-4, GPT-3.5-turbo, and Azure aliases.
    Cl100kBase,
}

impl ModelTokenizer {
    /// The compiled-in vocabulary, loaded once per process.
    ///
    /// Loading fails only if the embedded data is unreadable, which cannot
    /// happen in a correctly built binary; the caller then falls back to the
    /// estimator instead of panicking in a meter refresh.
    fn bpe(self) -> Option<&'static CoreBPE> {
        match self {
            Self::O200kBase => {
                static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();
                BPE.get_or_init(|| tiktoken_rs::o200k_base().ok()).as_ref()
            }
            Self::Cl100kBase => {
                static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();
                BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok()).as_ref()
            }
        }
    }

    /// Encoding name shown in the context meter.
    pub fn label(self) -> &'static str {
        match self {
            Self::O200kBase => "o200k_base",
            Self::Cl100kBase => "cl100k_base",
        }
    }
}

/// Exact counter backed by one of the compiled-in BPE vocabularies.
pub struct TiktokenCounter {
    bpe: &'static CoreBPE,
    tokenizer: ModelTokenizer,
}

impl TokenCounter for TiktokenCounter {
    fn count(&self, text: &str) -> u32 {
        self.bpe
            .encode_ordinary(text)
            .len()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    fn source(&self) -> TokenSource {
        TokenSource::Tiktoken
    }

    fn label(&self) -> &'static str {
        self.tokenizer.label()
    }
}

/// The tokenizer a model uses, if a licensed implementation covers it.
///
/// The name is normalized first: routing prefixes such as `openai/`, case, and
/// fine-tune suffixes (`ft:gpt-4o-2024-08-06:org::id`) are stripped so a proxy
/// model name still resolves to the vendor's encoding.
pub fn tokenizer_for_model(model: &str) -> Option<ModelTokenizer> {
    match get_tokenizer(&normalize_model_name(model))? {
        Tokenizer::O200kBase => Some(ModelTokenizer::O200kBase),
        Tokenizer::Cl100kBase => Some(ModelTokenizer::Cl100kBase),
        _ => None,
    }
}

/// The counter to use for `model`.
///
/// Returns an exact [`TiktokenCounter`] for recognized OpenAI-compatible
/// models and the shared character [`EstimateTokenCounter`] for everything
/// else.
pub fn token_counter_for_model(model: &str) -> Arc<dyn TokenCounter> {
    if let Some(tokenizer) = tokenizer_for_model(model) {
        if let Some(bpe) = tokenizer.bpe() {
            return Arc::new(TiktokenCounter { bpe, tokenizer });
        }
    }
    Arc::new(EstimateTokenCounter)
}

/// Lowercase a model name and drop routing prefixes and fine-tune suffixes.
fn normalize_model_name(model: &str) -> String {
    let model = model.trim().to_ascii_lowercase();
    let model = model.rsplit('/').next().unwrap_or_default();
    let model = model.strip_prefix("ft:").unwrap_or(model);
    model.split(':').next().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::usage::{estimate_tokens, MESSAGE_OVERHEAD_TOKENS};

    #[test]
    fn openai_chat_models_map_to_their_encoding() {
        for model in [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4.1",
            "gpt-4.1-mini",
            "gpt-5",
            "o1",
            "o3-mini",
        ] {
            assert_eq!(
                tokenizer_for_model(model),
                Some(ModelTokenizer::O200kBase),
                "{model}"
            );
        }
        for model in ["gpt-4", "gpt-4-turbo", "gpt-3.5-turbo", "gpt-35-turbo"] {
            assert_eq!(
                tokenizer_for_model(model),
                Some(ModelTokenizer::Cl100kBase),
                "{model}"
            );
        }
    }

    #[test]
    fn routed_and_fine_tuned_names_normalize_before_lookup() {
        for model in [
            "openai/gpt-4o",
            "GPT-4O",
            "ft:gpt-4o-2024-08-06:org:proj:id",
        ] {
            assert_eq!(
                tokenizer_for_model(model),
                Some(ModelTokenizer::O200kBase),
                "{model}"
            );
        }
        assert_eq!(
            tokenizer_for_model("ft:gpt-4:org::id"),
            Some(ModelTokenizer::Cl100kBase)
        );
    }

    #[test]
    fn unknown_models_fall_back_to_the_estimator() {
        for model in [
            "claude-sonnet-5-5",
            "deepseek-chat",
            "llama-3.2-3b",
            "mock-model",
            "",
        ] {
            let counter = token_counter_for_model(model);
            assert_eq!(counter.source(), TokenSource::Estimate, "{model}");
            assert_eq!(counter.label(), "character estimate", "{model}");
            assert_eq!(
                counter.count("hello world"),
                estimate_tokens("hello world"),
                "{model}"
            );
        }
    }

    #[test]
    fn known_models_count_with_the_compiled_vocabulary() {
        let o200k = token_counter_for_model("gpt-4o");
        assert_eq!(o200k.source(), TokenSource::Tiktoken);
        assert_eq!(o200k.label(), "o200k_base");
        assert_eq!(o200k.count(""), 0);
        assert_eq!(o200k.count("hello world"), 2);
        assert_eq!(
            o200k.count_message("hello world"),
            2 + MESSAGE_OVERHEAD_TOKENS
        );

        let cl100k = token_counter_for_model("gpt-4");
        assert_eq!(cl100k.source(), TokenSource::Tiktoken);
        assert_eq!(cl100k.label(), "cl100k_base");
        assert_eq!(cl100k.count("hello world"), 2);
    }

    #[test]
    fn long_text_counts_more_than_the_character_estimate_for_the_same_text() {
        // Repetitive punctuation and digits are where the quarter-token ASCII
        // heuristic drifts furthest from a real BPE.
        let text = "0123456789-+/*%#@!".repeat(20);
        let counted = token_counter_for_model("gpt-4o").count(&text);
        let estimated = estimate_tokens(&text);
        assert!(
            counted > estimated,
            "counted {counted}, estimated {estimated}"
        );
    }
}
