//! The Slack Web API calls the adapter makes (ADR-0059).
//!
//! Every method answers `{ "ok": true, … }` or `{ "ok": false, "error": "…" }`
//! with an HTTP 200, so the status code is not the result and `ok` has to be
//! read. The bot token goes in an `Authorization` header, never in the URL, so a
//! failure can print the request without leaking it.

use std::time::Duration;

use cowbot_core::error::CowError;
use reqwest::Client as HttpClient;
use serde::Deserialize;

use crate::config::SlackConfig;

/// How long one Web API call may take.
///
/// Slack answers quickly or not at all; a call that has not returned in fifteen
/// seconds belongs to the next message, not this one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a file download may take. The size cap bounds it, not the clock.
const FILE_TIMEOUT: Duration = Duration::from_secs(120);

/// A Web API failure, kept richer than [`CowError`] where the caller has to tell
/// cases apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackError {
    /// Slack's own error string, for example `channel_not_found`.
    pub error: String,
    /// Seconds Slack asked us to wait, from `Retry-After`.
    pub retry_after: Option<u64>,
}

impl SlackError {
    /// Whether Slack rejected the token.
    ///
    /// Terminal: a revoked or wrong token never starts working, and retrying
    /// hides which token is wrong behind a reconnect loop.
    pub fn is_auth(&self) -> bool {
        matches!(
            self.error.as_str(),
            "not_authed" | "invalid_auth" | "account_inactive" | "token_revoked" | "token_expired"
        )
    }

    /// Whether the bot is not in the channel.
    ///
    /// A fact about the binding, not a fault: this is the failure the binding
    /// health check exists to surface, because a bot absent from a channel
    /// receives nothing and logs nothing.
    pub fn is_not_in_channel(&self) -> bool {
        matches!(
            self.error.as_str(),
            "not_in_channel" | "channel_not_found" | "is_archived"
        )
    }

    /// Whether waiting and retrying is the right response.
    pub fn is_rate_limited(&self) -> bool {
        self.error == "ratelimited" || self.retry_after.is_some()
    }
}

impl std::fmt::Display for SlackError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "slack: {}", self.error)
    }
}

impl From<SlackError> for CowError {
    fn from(error: SlackError) -> Self {
        CowError::Llm(error.to_string())
    }
}

/// The calls one adapter makes.
#[derive(Clone)]
pub struct Api {
    http: HttpClient,
    config: SlackConfig,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Api")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// The envelope every method answers with.
#[derive(Debug, Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    response_metadata: Option<Metadata>,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    #[serde(default)]
    messages: Vec<String>,
}

impl Api {
    pub fn new(config: SlackConfig) -> Self {
        Self {
            http: HttpClient::new(),
            config,
        }
    }

    /// A client over an already-built HTTP client, for a test pointing it at a
    /// local fake.
    pub fn with_http(config: SlackConfig, http: HttpClient) -> Self {
        Self { http, config }
    }

    pub fn config(&self) -> &SlackConfig {
        &self.config
    }

    /// Exchange the app token for a Socket Mode WebSocket address.
    ///
    /// The address is single-use and expires after thirty seconds, so it is
    /// fetched immediately before every dial rather than cached.
    pub async fn open_connection(&self) -> Result<String, SlackError> {
        #[derive(Deserialize)]
        struct Opened {
            url: String,
        }
        let opened: Opened = self
            .call_with("apps.connections.open", &self.config.app_token, &[])
            .await?;
        Ok(opened.url)
    }

    /// Who the bot is, so the adapter can tell its own messages from a person's.
    pub async fn auth_test(&self) -> Result<Identity, SlackError> {
        self.call_with("auth.test", &self.config.bot_token, &[])
            .await
    }

    /// Post one message, optionally in a thread, with optional Block Kit blocks.
    ///
    /// `text` is always sent even when blocks are: it is what a client that
    /// cannot render blocks shows, and what a notification preview reads.
    pub async fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
        blocks: Option<serde_json::Value>,
    ) -> Result<Posted, SlackError> {
        let mut form: Vec<(&str, String)> = vec![
            ("channel", channel.to_string()),
            ("text", text.to_string()),
            ("mrkdwn", "true".to_string()),
            ("unfurl_links", "false".to_string()),
        ];
        if let Some(thread) = thread_ts {
            form.push(("thread_ts", thread.to_string()));
        }
        if let Some(blocks) = blocks {
            form.push(("blocks", blocks.to_string()));
        }
        self.call_with("chat.postMessage", &self.config.bot_token, &form)
            .await
    }

    /// Replace a posted message's text and blocks.
    ///
    /// Used to retire an approval card once it is decided. Slack refuses to
    /// update a `ts` the app did not post, which is why the adapter keeps the id
    /// it was given.
    pub async fn update_message(
        &self,
        channel: &str,
        ts: &str,
        text: &str,
        blocks: Option<serde_json::Value>,
    ) -> Result<(), SlackError> {
        let mut form: Vec<(&str, String)> = vec![
            ("channel", channel.to_string()),
            ("ts", ts.to_string()),
            ("text", text.to_string()),
        ];
        if let Some(blocks) = blocks {
            form.push(("blocks", blocks.to_string()));
        }
        let _: serde_json::Value = self
            .call_with("chat.update", &self.config.bot_token, &form)
            .await?;
        Ok(())
    }

    /// Information about a file a message referenced.
    ///
    /// The event carries a private URL and a size; the token is what makes the
    /// URL readable, which is why the download goes through this API rather
    /// than fetching whatever URL arrived in the event.
    pub async fn file_info(&self, file_id: &str) -> Result<FileInfo, SlackError> {
        self.call_with(
            "files.info",
            &self.config.bot_token,
            &[("file", file_id.to_string())],
        )
        .await
    }

    /// One file's bytes, read with a hard cap.
    ///
    /// Slack's `url_private` requires the bot token as an `Authorization`
    /// header. The URL's host is checked before the request: it arrives inside an
    /// event, and an event is untrusted input.
    pub async fn file_bytes(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, SlackError> {
        if !trusted_file_host(url) {
            return Err(SlackError {
                error: format!("refusing to download a file from an unexpected host: {url}"),
                retry_after: None,
            });
        }
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.config.bot_token)
            .timeout(FILE_TIMEOUT)
            .send()
            .await
            .map_err(|error| SlackError {
                error: format!("could not reach Slack: {error}"),
                retry_after: None,
            })?;
        if !response.status().is_success() {
            return Err(SlackError {
                error: format!("Slack refused the file download ({})", response.status()),
                retry_after: None,
            });
        }
        if let Some(length) = response.content_length() {
            if length > max_bytes {
                return Err(SlackError {
                    error: format!("the file is {length} bytes, above the {max_bytes}-byte limit"),
                    retry_after: None,
                });
            }
        }
        let bytes = response.bytes().await.map_err(|error| SlackError {
            error: format!("the download broke off: {error}"),
            retry_after: None,
        })?;
        if bytes.len() as u64 > max_bytes {
            return Err(SlackError {
                error: format!("the file grew past the {max_bytes}-byte limit"),
                retry_after: None,
            });
        }
        Ok(bytes.to_vec())
    }

    /// One channel's metadata, which is what proves the bot can see it.
    pub async fn call_conversations_info(&self, channel: &str) -> Result<Channel, SlackError> {
        self.call_with(
            "conversations.info",
            &self.config.bot_token,
            &[("channel", channel.to_string())],
        )
        .await
    }

    /// Every member of a channel, which is what proves the bot is in it.
    ///
    /// Slack answers `not_in_channel` for a channel the bot cannot see, and lists
    /// the bot for one it can. The difference is the whole point: a bot absent
    /// from a channel receives nothing and logs nothing.
    pub async fn call_conversations_members(
        &self,
        channel: &str,
    ) -> Result<Vec<String>, SlackError> {
        #[derive(Deserialize)]
        struct Members {
            #[serde(default)]
            members: Vec<String>,
        }
        let members: Members = self
            .call_with(
                "conversations.members",
                &self.config.bot_token,
                &[
                    ("channel", channel.to_string()),
                    ("limit", "200".to_string()),
                ],
            )
            .await?;
        Ok(members.members)
    }

    async fn call_with<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        token: &str,
        form: &[(&str, String)],
    ) -> Result<T, SlackError> {
        let response = self
            .http
            .post(self.config.method_url(method))
            .bearer_auth(token)
            .timeout(REQUEST_TIMEOUT)
            .form(form)
            .send()
            .await
            .map_err(|error| SlackError {
                error: format!("could not reach Slack: {error}"),
                retry_after: None,
            })?;

        // A rate limit is the one case where the status carries the answer:
        // Slack sends `Retry-After` and no usable body.
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(SlackError {
                error: "ratelimited".into(),
                retry_after,
            });
        }

        let body = response.text().await.unwrap_or_default();
        let envelope: Envelope = serde_json::from_str(&body).map_err(|_| SlackError {
            error: format!("unexpected answer from Slack: {}", truncate(&body)),
            retry_after,
        })?;
        if !envelope.ok {
            // `response_metadata.messages` carries the detail Slack leaves out of
            // `error`, and it is the only place a mis-scoped token explains
            // itself.
            let detail = envelope
                .response_metadata
                .map(|metadata| metadata.messages.join("; "))
                .filter(|detail| !detail.is_empty());
            return Err(SlackError {
                error: match detail {
                    Some(detail) => format!(
                        "{} ({detail})",
                        envelope.error.unwrap_or_else(|| "unknown".into())
                    ),
                    None => envelope.error.unwrap_or_else(|| "unknown".into()),
                },
                retry_after,
            });
        }
        serde_json::from_str(&body).map_err(|error| SlackError {
            error: format!("Slack answered a shape this adapter cannot read: {error}"),
            retry_after,
        })
    }
}

/// The bot's own identity.
#[derive(Debug, Clone, Deserialize)]
pub struct Identity {
    pub user_id: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub team_id: Option<String>,
}

/// A channel, as `conversations.info` describes it.
#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub is_archived: bool,
}

/// A posted message.
#[derive(Debug, Clone, Deserialize)]
pub struct Posted {
    pub ts: String,
    #[serde(default)]
    pub channel: String,
}

/// One file, as `files.info` describes it.
#[derive(Debug, Clone, Deserialize)]
pub struct FileInfo {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub mimetype: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub url_private: Option<String>,
}

/// Whether `url` is one Slack's file API would serve.
///
/// The URL arrives inside an event, so the check is that it names Slack: a
/// crafted event pointing at another host would otherwise have the adapter
/// attach its bot token to a request the sender chose.
pub fn trusted_file_host(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let host = rest.split(['/', '?']).next().unwrap_or_default();
    let host = host.split('@').next_back().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();
    host == "slack.com" || host.ends_with(".slack.com")
}

/// The first part of a body, for an error message.
fn truncate(body: &str) -> &str {
    let trimmed = body.trim();
    match trimmed.char_indices().nth(200) {
        Some((index, _)) => &trimmed[..index],
        None => trimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(name: &str) -> SlackError {
        SlackError {
            error: name.into(),
            retry_after: None,
        }
    }

    #[test]
    fn a_bad_token_is_terminal_and_a_missing_channel_is_a_binding_fact() {
        // The two are different jobs for the caller: one stops the adapter, the
        // other is reported as an unreachable binding.
        for name in [
            "not_authed",
            "invalid_auth",
            "token_revoked",
            "token_expired",
        ] {
            assert!(error(name).is_auth(), "{name}");
        }
        for name in ["not_in_channel", "channel_not_found"] {
            assert!(error(name).is_not_in_channel(), "{name}");
            assert!(!error(name).is_auth(), "{name}");
        }
        assert!(!error("channel_not_found").is_rate_limited());
        assert!(error("ratelimited").is_rate_limited());
    }

    #[test]
    fn a_file_url_must_name_slack_before_a_token_is_attached() {
        assert!(trusted_file_host(
            "https://files.slack.com/files-pri/T1-F1/x.png"
        ));
        assert!(trusted_file_host("https://slack.com/x"));
        // The attack: an event pointing the download somewhere else.
        assert!(!trusted_file_host("https://evil.example/x.png"));
        assert!(!trusted_file_host("https://slack.com.evil.example/x"));
        assert!(!trusted_file_host(
            "https://evil.example/?u=https://slack.com"
        ));
        assert!(!trusted_file_host("file:///etc/passwd"));
        // A userinfo section must not be read as the host.
        assert!(!trusted_file_host("https://slack.com@evil.example/x"));
    }

    #[test]
    fn a_long_body_is_truncated_on_a_character_boundary() {
        let body = "é".repeat(400);
        let cut = truncate(&body);
        assert_eq!(cut.chars().count(), 200);
    }
}
