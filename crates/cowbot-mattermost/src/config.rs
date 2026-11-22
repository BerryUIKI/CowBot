//! The configuration an operator stores for a Mattermost bot.
//!
//! Two values: where the server is, and the bot's session token. Everything else
//! — its own user id, the name it answers to, the server's post size limit — is
//! discovered at startup rather than typed in, because a value that can be read
//! from the server is a value an operator can get wrong.

use cowbot_core::error::{CowError, CowResult};
use serde::Deserialize;

/// A Mattermost bot's connection settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MattermostConfig {
    /// Server origin, for example `https://chat.example.com`.
    ///
    /// Stored without a trailing slash so a path can be appended without a
    /// double slash, and required to be `http` or `https` — a `wss` origin here
    /// is a mistake worth catching at parse time rather than at the first
    /// request.
    pub base_url: String,
    /// The bot account's session token.
    pub bot_token: String,
}

impl MattermostConfig {
    /// Read the configuration an operator stored.
    pub fn from_json(config: &str) -> CowResult<Self> {
        let mut parsed: MattermostConfig = serde_json::from_str(config)
            .map_err(|error| CowError::Config(format!("invalid Mattermost config: {error}")))?;
        parsed.base_url = parsed.base_url.trim().trim_end_matches('/').to_string();
        if !parsed.base_url.starts_with("http://") && !parsed.base_url.starts_with("https://") {
            return Err(CowError::Config(
                "base_url must start with http:// or https://; the WebSocket address is derived \
                 from it"
                    .into(),
            ));
        }
        if parsed.bot_token.trim().is_empty() {
            return Err(CowError::Config("bot_token cannot be empty".into()));
        }
        Ok(parsed)
    }

    /// The WebSocket origin the event stream lives on.
    pub fn socket_url(&self) -> String {
        let origin = if let Some(rest) = self.base_url.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base_url.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.base_url.clone()
        };
        format!("{origin}/api/v4/websocket")
    }

    /// An absolute API URL.
    pub fn api(&self, path: &str) -> String {
        format!("{}/api/v4{path}", self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: &str) -> CowResult<MattermostConfig> {
        MattermostConfig::from_json(json)
    }

    #[test]
    fn a_trailing_slash_is_removed_so_no_path_doubles_it() {
        let parsed =
            config(r#"{"base_url":"https://chat.example.com/","bot_token":"abc"}"#).unwrap();
        assert_eq!(parsed.base_url, "https://chat.example.com");
        assert_eq!(
            parsed.api("/users/me"),
            "https://chat.example.com/api/v4/users/me"
        );
    }

    #[test]
    fn the_socket_url_follows_the_transport() {
        let secure =
            config(r#"{"base_url":"https://chat.example.com","bot_token":"abc"}"#).unwrap();
        assert_eq!(
            secure.socket_url(),
            "wss://chat.example.com/api/v4/websocket"
        );

        let plain = config(r#"{"base_url":"http://localhost:8065","bot_token":"abc"}"#).unwrap();
        assert_eq!(plain.socket_url(), "ws://localhost:8065/api/v4/websocket");
    }

    #[test]
    fn a_missing_or_empty_token_is_an_error_naming_the_field() {
        let error =
            config(r#"{"base_url":"https://chat.example.com","bot_token":"  "}"#).unwrap_err();
        assert!(error.to_string().contains("bot_token"), "{error}");

        let missing = config(r#"{"base_url":"https://chat.example.com"}"#).unwrap_err();
        assert!(missing.to_string().contains("bot_token"), "{missing}");
    }

    #[test]
    fn a_base_url_without_a_scheme_is_refused_rather_than_guessed_at() {
        let error = config(r#"{"base_url":"chat.example.com","bot_token":"abc"}"#).unwrap_err();
        assert!(error.to_string().contains("http"), "{error}");
    }

    #[test]
    fn a_wss_origin_is_refused_because_the_socket_url_is_derived() {
        // An operator who pasted the socket URL would otherwise get a doubly
        // derived address and a confusing connection failure.
        let error =
            config(r#"{"base_url":"wss://chat.example.com","bot_token":"abc"}"#).unwrap_err();
        assert!(error.to_string().contains("http"), "{error}");
    }

    #[test]
    fn a_configuration_that_is_not_json_says_so() {
        let error = config("not json").unwrap_err();
        assert!(
            error.to_string().contains("invalid Mattermost config"),
            "{error}"
        );
    }
}
