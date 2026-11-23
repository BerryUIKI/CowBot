//! The configuration an operator stores for a Slack app (ADR-0059).
//!
//! Three values, and which are required depends on the transport:
//!
//! * `bot_token` — the `xoxb-` token used to post and to read files.
//! * `app_token` — the `xapp-` token with `connections:write`, used only to open
//!   the Socket Mode connection.
//! * `signing_secret` — used only by the Events API transport, which this build
//!   does not run.
//!
//! The prefixes are checked because they are the single most common mistake: an
//! `xoxb-` token pasted into the app-token field fails with `not_authed`, which
//! names the symptom rather than the mistake.

use cowbot_core::error::{CowError, CowResult};
use serde::Deserialize;

/// Slack's Web API origin.
pub const DEFAULT_API_BASE: &str = "https://slack.com/api";

/// A Slack app's connection settings.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct SlackConfig {
    /// The bot token, `xoxb-…`, for the Web API.
    pub bot_token: String,
    /// The app-level token, `xapp-…`, for Socket Mode.
    pub app_token: String,
    /// The app's signing secret.
    ///
    /// Kept for the Events API transport, which needs it and which this build
    /// does not implement. Optional so an operator using Socket Mode does not
    /// store a secret nothing reads.
    #[serde(default)]
    pub signing_secret: Option<String>,
    /// The Web API origin, for a test pointed at a local fake.
    #[serde(default = "default_api_base")]
    pub api_base: String,
}

fn default_api_base() -> String {
    DEFAULT_API_BASE.to_string()
}

impl SlackConfig {
    /// Read the configuration an operator stored.
    pub fn from_json(config: &str) -> CowResult<Self> {
        let mut parsed: SlackConfig = serde_json::from_str(config)
            .map_err(|error| CowError::Config(format!("invalid Slack config: {error}")))?;
        parsed.bot_token = parsed.bot_token.trim().to_string();
        parsed.app_token = parsed.app_token.trim().to_string();
        parsed.api_base = parsed.api_base.trim().trim_end_matches('/').to_string();

        if !parsed.api_base.starts_with("http://") && !parsed.api_base.starts_with("https://") {
            return Err(CowError::Config(
                "api_base must start with http:// or https://".into(),
            ));
        }
        // The prefixes are Slack's own, and a swapped pair is the mistake this
        // catches: `not_authed` at the first call names nothing an operator can
        // act on.
        if !parsed.bot_token.starts_with("xoxb-") {
            return Err(CowError::Config(
                "bot_token must be the bot token Slack shows as the `xoxb-` token; the app-level \
                 `xapp-` token goes in app_token"
                    .into(),
            ));
        }
        if !parsed.app_token.starts_with("xapp-") {
            return Err(CowError::Config(
                "app_token must be the app-level token with the `connections:write` scope, which \
                 Slack shows as an `xapp-` token; the bot token goes in bot_token"
                    .into(),
            ));
        }
        Ok(parsed)
    }

    /// An absolute Web API URL for one method.
    pub fn method_url(&self, method: &str) -> String {
        format!("{}/{}", self.api_base, method)
    }
}

impl std::fmt::Debug for SlackConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Neither token is printed: this type can reach a log line through the
        // adapter's `Debug`, and an `xoxb-` token is a full credential.
        formatter
            .debug_struct("SlackConfig")
            .field("api_base", &self.api_base)
            .field("bot_token", &"[sealed]")
            .field("app_token", &"[sealed]")
            .field(
                "signing_secret",
                &self.signing_secret.as_ref().map(|_| "[sealed]"),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: &str) -> CowResult<SlackConfig> {
        SlackConfig::from_json(json)
    }

    #[test]
    fn the_two_tokens_are_read_and_a_trailing_slash_is_removed() {
        let parsed = config(
            r#"{"bot_token":"xoxb-1","app_token":"xapp-1","api_base":"https://slack.com/api/"}"#,
        )
        .unwrap();
        assert_eq!(parsed.api_base, "https://slack.com/api");
        assert_eq!(
            parsed.method_url("chat.postMessage"),
            "https://slack.com/api/chat.postMessage"
        );
    }

    #[test]
    fn a_swapped_pair_names_the_mistake_rather_than_failing_later() {
        // The commonest configuration error, and the one whose own error
        // message ("not_authed") says nothing useful.
        let swapped = config(r#"{"bot_token":"xapp-1","app_token":"xoxb-1"}"#).unwrap_err();
        assert!(swapped.to_string().contains("xoxb-"), "{swapped}");

        let missing = config(r#"{"bot_token":"xoxb-1"}"#).unwrap_err();
        assert!(missing.to_string().contains("app_token"), "{missing}");
    }

    #[test]
    fn a_signing_secret_is_optional_because_socket_mode_does_not_read_it() {
        let parsed = config(r#"{"bot_token":"xoxb-1","app_token":"xapp-1"}"#).unwrap();
        assert!(parsed.signing_secret.is_none());
    }

    #[test]
    fn an_api_base_without_a_scheme_is_refused() {
        let error =
            config(r#"{"bot_token":"xoxb-1","app_token":"xapp-1","api_base":"slack.com/api"}"#)
                .unwrap_err();
        assert!(error.to_string().contains("http"), "{error}");
    }

    #[test]
    fn a_debug_print_never_carries_a_token() {
        let parsed = config(
            r#"{"bot_token":"xoxb-secret","app_token":"xapp-secret","signing_secret":"sss"}"#,
        )
        .unwrap();
        let printed = format!("{parsed:?}");
        assert!(!printed.contains("xoxb-secret"), "{printed}");
        assert!(!printed.contains("xapp-secret"), "{printed}");
        assert!(!printed.contains("sss"), "{printed}");
    }
}
