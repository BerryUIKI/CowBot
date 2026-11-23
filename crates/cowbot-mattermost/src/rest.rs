//! The Mattermost REST calls the adapter makes (ADR-0057).
//!
//! Three things: who the bot is, how large a post may be, and posting a reply.
//! Everything else comes over the socket.

use cowbot_core::error::{CowError, CowResult};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::MattermostConfig;

/// How long one REST call may take before it is treated as failed.
///
/// A Mattermost server that does not answer in ten seconds is not going to
/// answer usefully for this message; the next one can try.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a file download may take.
///
/// Longer than a JSON call: a 20 MB attachment over a slow link is not a broken
/// server, and the size cap is what bounds this rather than the clock.
const FILE_TIMEOUT: Duration = Duration::from_secs(120);

/// The largest post the server accepts, in bytes of UTF-8.
///
/// A value used only when the server does not report one. Mattermost's own
/// default is 16 384; hard-coding that as *the* answer would break a deployment
/// that raised or lowered it, which is why it is read from the server first.
pub const DEFAULT_MAX_POST_BYTES: usize = 16_384;

/// The bot account behind a token, as the server describes it.
#[derive(Debug, Clone, Deserialize)]
pub struct MattermostUser {
    pub id: String,
    pub username: String,
    /// Whether this account is a bot. Used to confirm the token really is one
    /// rather than a human's, which would make the adapter answer as that
    /// person.
    #[serde(default)]
    pub is_bot: bool,
}

/// The server's own configuration, of which only the post size matters here.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServerConfig {
    /// `MaxPostSize` in the server's `config.json`, in bytes.
    #[serde(rename = "MaxPostSize", default)]
    pub max_post_size: Option<i64>,
}

impl ServerConfig {
    /// The effective post limit, falling back to Mattermost's default.
    pub fn max_post_bytes(&self) -> usize {
        self.max_post_size
            .filter(|size| *size > 0)
            .map(|size| size as usize)
            .unwrap_or(DEFAULT_MAX_POST_BYTES)
    }
}

/// A post created on the server.
#[derive(Debug, Clone, Deserialize)]
pub struct CreatedPost {
    pub id: String,
}

/// An uploaded file, as `GET /files/{id}/info` describes it.
#[derive(Debug, Clone, Deserialize)]
pub struct FileInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub mime_type: String,
}

/// The request body for a new post.
#[derive(Debug, Serialize)]
struct CreatePost<'a> {
    channel_id: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_id: Option<&'a str>,
}

/// The request body for editing an existing post (ADR-0060).
///
/// Only the message is sent. Mattermost's update call replaces the whole post,
/// and sending back fields the adapter did not read would drop the ones it did
/// not know about.
#[derive(Debug, Serialize)]
struct UpdatePost<'a> {
    id: &'a str,
    message: &'a str,
}

/// The calls one adapter makes.
#[derive(Clone)]
pub struct Rest {
    http: HttpClient,
    config: MattermostConfig,
}

impl std::fmt::Debug for Rest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is deliberately absent: a debug print of this type must not
        // be able to leak it into a log line.
        formatter
            .debug_struct("Rest")
            .field("base_url", &self.config.base_url)
            .finish_non_exhaustive()
    }
}

impl Rest {
    pub fn new(config: MattermostConfig) -> Self {
        Self {
            http: HttpClient::new(),
            config,
        }
    }

    /// A client over an already-built HTTP client, for tests that point it at a
    /// local server.
    pub fn with_http(config: MattermostConfig, http: HttpClient) -> Self {
        Self { http, config }
    }

    pub fn config(&self) -> &MattermostConfig {
        &self.config
    }

    /// Who the token belongs to.
    pub async fn me(&self) -> CowResult<MattermostUser> {
        self.get_json("/users/me").await
    }

    /// One user, by id.
    ///
    /// Used to turn a mention's user id into a username. A display name is what
    /// the pipeline stores, and the id alone reads as a machine identifier in
    /// the timeline.
    pub async fn user(&self, user_id: &str) -> CowResult<MattermostUser> {
        self.get_json(&format!("/users/{user_id}")).await
    }

    /// Whether `user_id` is a member of `channel_id`.
    ///
    /// Mattermost answers `201` with a member object when the user is in the
    /// channel, and `400` with a `not_in_channel` style error when they are not.
    /// That `400` is the answer this exists to read, so it is mapped rather than
    /// propagated: "the bot is not in this channel" is a fact about the binding,
    /// not a failure of the request.
    pub async fn is_channel_member(&self, channel_id: &str, user_id: &str) -> CowResult<bool> {
        let response = self
            .http
            .get(
                self.config
                    .api(&format!("/channels/{channel_id}/members/{user_id}")),
            )
            .bearer_auth(self.token())
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        if status.is_success() {
            return Ok(true);
        }
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::BAD_REQUEST {
            return Ok(false);
        }
        // A 401 or a 5xx is not an answer about membership; reporting `false`
        // would tell an operator to fix a binding that is fine.
        Err(rejected("checking channel membership", status, response).await)
    }

    /// The server's configuration, for the post size limit.
    pub async fn server_config(&self) -> CowResult<ServerConfig> {
        self.get_json("/config/client?format=json").await
    }

    /// One uploaded file's metadata (ADR-0060).
    ///
    /// The post event names only the id, so this is how the size is learned
    /// before the bytes are asked for.
    pub async fn file_info(&self, file_id: &str) -> CowResult<FileInfo> {
        self.get_json(&format!("/files/{file_id}/info")).await
    }

    /// One uploaded file's bytes, read with a hard cap.
    ///
    /// The cap is enforced while reading rather than after: buffering a file the
    /// deployment will refuse is exactly the denial of service the cap prevents.
    /// A file that grows past the cap mid-read is an error, not a truncated save.
    pub async fn file_bytes(&self, file_id: &str, max_bytes: u64) -> CowResult<Vec<u8>> {
        let response = self
            .http
            .get(self.config.api(&format!("/files/{file_id}")))
            .bearer_auth(self.token())
            .timeout(FILE_TIMEOUT)
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(rejected("downloading a file", status, response).await);
        }
        // A `Content-Length` above the cap is refused before any body is read.
        if let Some(length) = response.content_length() {
            if length > max_bytes {
                return Err(CowError::Config(format!(
                    "the file is {length} bytes, above the {max_bytes}-byte limit"
                )));
            }
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(transport)?;
            if (body.len() + chunk.len()) as u64 > max_bytes {
                return Err(CowError::Config(format!(
                    "the file grew past the {max_bytes}-byte limit and was not kept"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    /// Post a message, optionally in a thread.
    pub async fn create_post(
        &self,
        channel_id: &str,
        message: &str,
        root_id: Option<&str>,
    ) -> CowResult<CreatedPost> {
        let response = self
            .http
            .post(self.config.api("/posts"))
            .bearer_auth(self.token())
            .timeout(REQUEST_TIMEOUT)
            .json(&CreatePost {
                channel_id,
                message,
                root_id,
            })
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(rejected("posting a message", status, response).await);
        }
        response.json().await.map_err(transport)
    }

    /// Replace a post's message.
    ///
    /// Used to strike out an approval card once it is decided, so a person
    /// scrolling back does not tap a button whose decision already landed.
    pub async fn update_post(&self, post_id: &str, message: &str) -> CowResult<()> {
        let response = self
            .http
            .put(self.config.api(&format!("/posts/{post_id}")))
            .bearer_auth(self.token())
            .timeout(REQUEST_TIMEOUT)
            .json(&UpdatePost {
                id: post_id,
                message,
            })
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(rejected("updating a post", status, response).await);
        }
        Ok(())
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> CowResult<T> {
        let response = self
            .http
            .get(self.config.api(path))
            .bearer_auth(self.token())
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        if !status.is_success() {
            return Err(rejected("reading the server", status, response).await);
        }
        response.json().await.map_err(transport)
    }

    fn token(&self) -> &str {
        &self.config.bot_token
    }
}

/// A request that never reached the server, or a body that did not parse.
fn transport(error: reqwest::Error) -> CowError {
    CowError::Llm(format!("mattermost: {error}"))
}

/// A response the server answered with a non-success status.
///
/// The body is included when it is short: Mattermost puts a readable reason
/// there, and an operator debugging a binding needs it more than a status code.
async fn rejected(
    doing: &str,
    status: reqwest::StatusCode,
    response: reqwest::Response,
) -> CowError {
    let body = response.text().await.unwrap_or_default();
    let detail = body.trim();
    let detail = if detail.len() > 200 {
        &detail[..200]
    } else {
        detail
    };
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    CowError::Llm(format!(
        "mattermost: {} while {doing}{detail}",
        status.as_u16()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_that_reports_no_limit_falls_back_to_mattermosts_default() {
        assert_eq!(
            ServerConfig::default().max_post_bytes(),
            DEFAULT_MAX_POST_BYTES
        );
        let zero = ServerConfig {
            max_post_size: Some(0),
        };
        assert_eq!(zero.max_post_bytes(), DEFAULT_MAX_POST_BYTES);
        let negative = ServerConfig {
            max_post_size: Some(-5),
        };
        assert_eq!(negative.max_post_bytes(), DEFAULT_MAX_POST_BYTES);
    }

    #[test]
    fn a_reported_limit_is_used_verbatim() {
        let raised = ServerConfig {
            max_post_size: Some(65_536),
        };
        assert_eq!(raised.max_post_bytes(), 65_536);
        let lowered = ServerConfig {
            max_post_size: Some(4_000),
        };
        assert_eq!(lowered.max_post_bytes(), 4_000);
    }

    #[test]
    fn the_client_config_key_is_read_as_the_server_spells_it() {
        let parsed: ServerConfig =
            serde_json::from_str(r#"{"MaxPostSize": 20000, "Other": true}"#).unwrap();
        assert_eq!(parsed.max_post_bytes(), 20_000);
    }

    #[test]
    fn a_debug_print_never_carries_the_token() {
        let rest = Rest::new(MattermostConfig {
            base_url: "https://chat.example.com".into(),
            bot_token: "super-secret-token".into(),
        });
        let printed = format!("{rest:?}");
        assert!(!printed.contains("super-secret-token"), "{printed}");
        assert!(printed.contains("chat.example.com"));
    }
}
