//! The configuration an operator stores for a Telegram bot (ADR-0058).
//!
//! One value in the common case: the bot token BotFather handed out. Everything
//! else the adapter needs — the bot's own id, the name it shows, the chats it
//! can reach — is read from Telegram at startup.

use cowbot_core::error::{CowError, CowResult};
use serde::Deserialize;

/// Telegram's cloud Bot API.
pub const DEFAULT_API_BASE: &str = "https://api.telegram.org";

/// A Telegram bot's connection settings.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct TelegramConfig {
    /// The token BotFather issued, shaped `<bot id>:<secret>`.
    pub bot_token: String,
    /// Where the Bot API lives.
    ///
    /// Overridable because Telegram publishes the server and lets anyone run
    /// their own Bot API server; a deployment behind one sets it here. The
    /// default is the cloud API, which is what almost everyone uses.
    #[serde(default = "cloud_api")]
    pub api_base: String,
}

fn cloud_api() -> String {
    DEFAULT_API_BASE.to_string()
}

impl std::fmt::Debug for TelegramConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is a bearer credential in a URL path; a debug print of this
        // type must not be able to leak it into a log line.
        formatter
            .debug_struct("TelegramConfig")
            .field("bot_token", &"[sealed]")
            .field("api_base", &self.api_base)
            .finish()
    }
}

impl TelegramConfig {
    /// Read the configuration an operator stored.
    pub fn from_json(config: &str) -> CowResult<Self> {
        let mut parsed: TelegramConfig = serde_json::from_str(config)
            .map_err(|error| CowError::Config(format!("invalid Telegram config: {error}")))?;
        parsed.bot_token = parsed.bot_token.trim().to_string();
        parsed.api_base = parsed.api_base.trim().trim_end_matches('/').to_string();

        // Telegram tokens are always `<digits>:<secret>`. Checking the shape
        // catches the mistakes an operator actually makes — pasting the bot's
        // username, a t.me link, or the word "token" — at parse time, where the
        // message can name the field, instead of as a 401 at startup.
        let shaped = parsed
            .bot_token
            .split_once(':')
            .is_some_and(|(id, secret)| {
                !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && !secret.is_empty()
            });
        if !shaped {
            return Err(CowError::Config(
                "bot_token must be the token BotFather issued, shaped '<digits>:<secret>'; \
                 a bot's @username does not work here"
                    .into(),
            ));
        }
        if !parsed.api_base.starts_with("http://") && !parsed.api_base.starts_with("https://") {
            return Err(CowError::Config(
                "api_base must start with http:// or https://".into(),
            ));
        }
        Ok(parsed)
    }

    /// Whether this configuration points at Telegram's own cloud API.
    pub fn uses_cloud_api(&self) -> bool {
        self.api_base == DEFAULT_API_BASE
    }

    /// The URL one Bot API method lives at.
    ///
    /// The token is part of the path because that is the whole of Telegram's
    /// authentication for a bot. It is why `Debug` and error messages have to be
    /// careful not to print this.
    pub fn method_url(&self, method: &str) -> String {
        format!("{}/bot{}/{method}", self.api_base, self.bot_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> CowResult<TelegramConfig> {
        TelegramConfig::from_json(json)
    }

    #[test]
    fn a_token_only_configuration_points_at_the_cloud_api() {
        let parsed = parse(r#"{"bot_token":"123456:ABC-DEF"}"#).unwrap();
        assert_eq!(parsed.api_base, DEFAULT_API_BASE);
        assert!(parsed.uses_cloud_api());
        assert_eq!(
            parsed.method_url("getMe"),
            "https://api.telegram.org/bot123456:ABC-DEF/getMe"
        );
    }

    #[test]
    fn a_self_hosted_api_base_is_kept_without_a_trailing_slash() {
        let parsed =
            parse(r#"{"bot_token":"123456:ABC-DEF","api_base":"https://bot.internal.example/"}"#)
                .unwrap();
        assert_eq!(parsed.api_base, "https://bot.internal.example");
        assert!(!parsed.uses_cloud_api());
        assert_eq!(
            parsed.method_url("sendMessage"),
            "https://bot.internal.example/bot123456:ABC-DEF/sendMessage"
        );
    }

    #[test]
    fn a_bot_username_is_refused_with_a_message_naming_the_field() {
        // The mistake an operator actually makes: BotFather shows both a name
        // and a token, and the name is the memorable one.
        for wrong in [
            r#"{"bot_token":"@cowbot_help_bot"}"#,
            r#"{"bot_token":"https://t.me/cowbot_help_bot"}"#,
            r#"{"bot_token":"token"}"#,
            r#"{"bot_token":"123456:"}"#,
            r#"{"bot_token":":ABC"}"#,
            r#"{"bot_token":"  "}"#,
        ] {
            let error = parse(wrong).unwrap_err();
            assert!(error.to_string().contains("bot_token"), "{wrong}: {error}");
        }
    }

    #[test]
    fn a_missing_token_says_which_field_is_missing() {
        let error = parse(r#"{"api_base":"https://api.telegram.org"}"#).unwrap_err();
        assert!(error.to_string().contains("bot_token"), "{error}");
    }

    #[test]
    fn an_api_base_without_a_scheme_is_refused() {
        let error = parse(r#"{"bot_token":"1:A","api_base":"api.telegram.org"}"#).unwrap_err();
        assert!(error.to_string().contains("http"), "{error}");
    }

    #[test]
    fn a_configuration_that_is_not_json_says_so() {
        let error = parse("not json").unwrap_err();
        assert!(
            error.to_string().contains("invalid Telegram config"),
            "{error}"
        );
    }

    #[test]
    fn a_debug_print_never_carries_the_token() {
        let config = parse(r#"{"bot_token":"123456:super-secret"}"#).unwrap();
        let printed = format!("{config:?}");
        assert!(!printed.contains("super-secret"), "{printed}");
        assert!(!printed.contains("123456"), "{printed}");
        assert!(printed.contains("api.telegram.org"), "{printed}");
    }
}
