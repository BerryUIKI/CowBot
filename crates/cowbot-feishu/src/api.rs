//! The Feishu OpenAPI calls the adapter makes (ADR-0061).
//!
//! Two rules shape this module:
//!
//! * **Every response is HTTP 200**, and a business failure is a non-zero `code`
//!   in the body. An HTTP-status-only check reads a failed send as a success.
//! * **The tenant token expires in about two hours** and is derived from the app
//!   secret, so it is cached in memory and never configured: a stored copy is a
//!   stored secret that goes stale.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use cowbot_core::error::CowError;
use reqwest::Client as HttpClient;
use serde::Deserialize;

use crate::config::FeishuConfig;

/// How long one OpenAPI call may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a file download may take. The size cap bounds it, not the clock.
const FILE_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a token is trusted for.
///
/// Feishu grants 7 200 seconds. The margin means a token is never used in the
/// last minute of its life, which is when a request in flight would be refused
/// and the retry would need the very token that just expired.
const TOKEN_MARGIN: Duration = Duration::from_secs(60);

/// A failed call, kept richer than [`CowError`] where the caller has to tell
/// cases apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// Feishu's own code, or the HTTP status when the body carried none.
    pub code: i64,
    pub message: String,
    /// Seconds to wait, from the rate-limit header, when Feishu sent one.
    pub retry_after: Option<u64>,
}

impl ApiError {
    /// Whether the app's credentials were refused.
    ///
    /// Terminal: a wrong or disabled app never starts working.
    pub fn is_auth(&self) -> bool {
        matches!(self.code, 99_971_601..=99_971_605) || self.code == 401
    }

    /// Whether the bot is not in the chat.
    ///
    /// A fact about the binding rather than a fault: this is the silent failure
    /// the binding health check exists to surface.
    pub fn is_not_in_chat(&self) -> bool {
        // 230002 is "bot is not in the chat"; 230027 is a chat id the bot cannot
        // see.
        matches!(self.code, 230_002 | 230_027)
    }

    /// Whether waiting and retrying is right.
    pub fn is_rate_limited(&self) -> bool {
        self.code == 99_991_400 || self.retry_after.is_some()
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "feishu: {} {}", self.code, self.message)
    }
}

impl From<ApiError> for CowError {
    fn from(error: ApiError) -> Self {
        CowError::Llm(error.to_string())
    }
}

/// The calls one adapter makes.
#[derive(Clone)]
pub struct Api {
    http: HttpClient,
    config: FeishuConfig,
    /// The current token and the instant it stops being trusted.
    token: std::sync::Arc<Mutex<Option<(String, Instant)>>>,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Api")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// The body every OpenAPI call answers with.
#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    msg: String,
    /// The OAuth endpoint spells its failure differently.
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

impl Api {
    pub fn new(config: FeishuConfig) -> Self {
        Self {
            http: HttpClient::new(),
            config,
            token: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    pub fn config(&self) -> &FeishuConfig {
        &self.config
    }

    /// A tenant access token, fetching one only when the cached one is stale.
    pub async fn tenant_token(&self) -> Result<String, ApiError> {
        {
            let cached = self
                .token
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((token, expiry)) = cached.as_ref() {
                if Instant::now() < *expiry {
                    return Ok(token.clone());
                }
            }
        }

        let url = format!("{}/oauth/v3/token", self.config.domain.accounts());
        let response = self
            .http
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.config.app_id.as_str()),
                ("client_secret", self.config.app_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|error| ApiError {
                code: 0,
                message: format!("could not reach Feishu: {error}"),
                retry_after: None,
            })?;
        let body: Envelope = response.json().await.map_err(|error| ApiError {
            code: 0,
            message: format!("Feishu answered a token response this adapter cannot read: {error}"),
            retry_after: None,
        })?;

        // The OAuth endpoint reports failure with `error`, not `code`, and a
        // missing token is a failure whatever the body says.
        let Some(token) = body.access_token else {
            return Err(ApiError {
                code: body.code,
                message: body
                    .error_description
                    .or(body.error)
                    .unwrap_or_else(|| body.msg.clone()),
                retry_after: None,
            });
        };

        let lifetime = Duration::from_secs(body.expires_in.unwrap_or(7_200));
        let expiry = Instant::now() + lifetime.saturating_sub(TOKEN_MARGIN);
        let mut cached = self
            .token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cached = Some((token.clone(), expiry));
        Ok(token)
    }

    /// The exchange the long connection begins with.
    ///
    /// The body is **PascalCase** here and snake_case everywhere else: this
    /// endpoint predates the OpenAPI convention, and a snake_case body answers a
    /// non-zero code that reads like a credential failure.
    pub async fn open_connection(&self) -> Result<Connection, ApiError> {
        let response = self
            .http
            .post(self.config.open_api("/callback/ws/endpoint"))
            .header("locale", "zh")
            .timeout(REQUEST_TIMEOUT)
            .json(&serde_json::json!({
                "AppID": self.config.app_id,
                "AppSecret": self.config.app_secret,
            }))
            .send()
            .await
            .map_err(|error| ApiError {
                code: 0,
                message: format!("could not reach Feishu: {error}"),
                retry_after: None,
            })?;
        let body: ConnectionEnvelope = response.json().await.map_err(|error| ApiError {
            code: 0,
            message: format!("Feishu answered a handshake this adapter cannot read: {error}"),
            retry_after: None,
        })?;
        if body.code != 0 {
            return Err(ApiError {
                code: body.code,
                message: body.msg,
                retry_after: None,
            });
        }
        let Some(data) = body.data else {
            return Err(ApiError {
                code: 0,
                message: "Feishu answered a handshake with no connection in it".into(),
                retry_after: None,
            });
        };
        Ok(data)
    }

    /// Who the bot is.
    ///
    /// The `open_id` is not configuration and is not in a message. Without it the
    /// adapter cannot tell "the bot was mentioned" from "a bot was mentioned",
    /// which is the whole addressing rule.
    pub async fn bot_info(&self) -> Result<BotInfo, ApiError> {
        #[derive(Deserialize)]
        struct Body {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            bot: Option<BotInfo>,
        }
        let body: Body = self
            .call("GET", "/open-apis/bot/v3/info", None, None)
            .await?;
        body.bot.ok_or(ApiError {
            code: body.code,
            message: body.msg,
            retry_after: None,
        })
    }

    /// Post one message into a chat.
    ///
    /// `msg_type` is `text`, `interactive`, or a file kind, and `content` is a
    /// JSON **string** — the double encoding is Feishu's, not this adapter's.
    pub async fn send_message(
        &self,
        chat_id: &str,
        msg_type: &str,
        content: &str,
    ) -> Result<Sent, ApiError> {
        #[derive(Deserialize)]
        struct Body {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            data: Option<Sent>,
        }
        let payload = serde_json::json!({
            "receive_id": chat_id,
            "msg_type": msg_type,
            "content": content,
        });
        let body: Body = self
            .call(
                "POST",
                "/open-apis/im/v1/messages?receive_id_type=chat_id",
                Some(payload),
                None,
            )
            .await?;
        body.data.ok_or(ApiError {
            code: body.code,
            message: body.msg,
            retry_after: None,
        })
    }

    /// Update an interactive card.
    ///
    /// Only a card can be updated, and only one that declared `update_multi`. A
    /// text message cannot be revised at all, which is why an approval card is
    /// the only thing this adapter edits.
    pub async fn update_card(
        &self,
        message_id: &str,
        card: &serde_json::Value,
    ) -> Result<(), ApiError> {
        let payload = serde_json::json!({ "content": card.to_string() });
        let body: serde_json::Value = self
            .call(
                "PATCH",
                &format!("/open-apis/im/v1/messages/{message_id}"),
                Some(payload),
                None,
            )
            .await?;
        let code = body
            .get("code")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if code != 0 {
            return Err(ApiError {
                code,
                message: body
                    .get("msg")
                    .and_then(|value| value.as_str())
                    .unwrap_or("the card update was refused")
                    .to_string(),
                retry_after: None,
            });
        }
        Ok(())
    }

    /// One chat's information, which is what tells the adapter it can see it.
    pub async fn chat(&self, chat_id: &str) -> Result<Chat, ApiError> {
        #[derive(Deserialize)]
        struct Body {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            data: Option<Chat>,
        }
        let body: Body = self
            .call(
                "GET",
                &format!("/open-apis/im/v1/chats/{chat_id}"),
                None,
                None,
            )
            .await?;
        body.data.ok_or(ApiError {
            code: body.code,
            message: body.msg,
            retry_after: None,
        })
    }

    /// Whether the bot is a member of a chat.
    ///
    /// The real binding check: a bot absent from a chat receives nothing and logs
    /// nothing.
    pub async fn is_in_chat(&self, chat_id: &str) -> Result<bool, ApiError> {
        #[derive(Deserialize)]
        struct Body {
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: String,
            #[serde(default)]
            data: Option<Inside>,
        }
        #[derive(Deserialize)]
        struct Inside {
            #[serde(default)]
            is_in_chat: bool,
        }
        let body: Body = self
            .call(
                "GET",
                &format!("/open-apis/im/v1/chats/{chat_id}/members/is_in_chat"),
                None,
                None,
            )
            .await?;
        body.data.map(|inside| inside.is_in_chat).ok_or(ApiError {
            code: body.code,
            message: body.msg,
            retry_after: None,
        })
    }

    /// One file's bytes, read with a hard cap.
    ///
    /// The resource path needs both the message and the key, which is why the
    /// attachment's `external_id` is `message_id:file_key` rather than the key
    /// alone.
    pub async fn file_bytes(
        &self,
        message_id: &str,
        file_key: &str,
        kind: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, ApiError> {
        let token = self.tenant_token().await?;
        let url = self.config.open_api(&format!(
            "/open-apis/im/v1/messages/{message_id}/resources/{file_key}?type={kind}"
        ));
        let response = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .timeout(FILE_TIMEOUT)
            .send()
            .await
            .map_err(|error| ApiError {
                code: 0,
                message: format!("could not reach Feishu: {error}"),
                retry_after: None,
            })?;
        // A file download answers with the bytes and an HTTP status, unlike every
        // JSON call above.
        if !response.status().is_success() {
            return Err(ApiError {
                code: response.status().as_u16() as i64,
                message: format!("Feishu refused the file download ({})", response.status()),
                retry_after: None,
            });
        }
        if let Some(length) = response.content_length() {
            if length > max_bytes {
                return Err(ApiError {
                    code: 0,
                    message: format!(
                        "the file is {length} bytes, above the {max_bytes}-byte limit"
                    ),
                    retry_after: None,
                });
            }
        }
        let bytes = response.bytes().await.map_err(|error| ApiError {
            code: 0,
            message: format!("the download broke off: {error}"),
            retry_after: None,
        })?;
        if bytes.len() as u64 > max_bytes {
            return Err(ApiError {
                code: 0,
                message: format!("the file grew past the {max_bytes}-byte limit"),
                retry_after: None,
            });
        }
        Ok(bytes.to_vec())
    }

    /// One call, with the token and the envelope read.
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        _unused: Option<()>,
    ) -> Result<T, ApiError> {
        let token = self.tenant_token().await?;
        let url = self.config.open_api(path);
        let mut request = match method {
            "GET" => self.http.get(&url),
            "PATCH" => self.http.patch(&url),
            _ => self.http.post(&url),
        }
        .bearer_auth(token)
        .timeout(REQUEST_TIMEOUT);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| ApiError {
            code: 0,
            message: format!("could not reach Feishu: {error}"),
            retry_after: None,
        })?;

        let retry_after = response
            .headers()
            .get("x-ogw-ratelimit-reset")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let status = response.status().as_u16() as i64;
        let text = response.text().await.unwrap_or_default();
        let body: T = serde_json::from_str(&text).map_err(|error| ApiError {
            code: status,
            message: format!(
                "Feishu answered a shape this adapter cannot read: {error}: {}",
                truncate(&text)
            ),
            retry_after,
        })?;
        Ok(body)
    }
}

/// The endpoint the long connection dials.
#[derive(Debug, Clone, Deserialize)]
pub struct Connection {
    #[serde(rename = "URL")]
    pub url: String,
    #[serde(rename = "ClientConfig", default)]
    pub client_config: ClientConfig,
}

impl Connection {
    /// The `device_id` and `service_id` the URL carries, which every frame needs.
    pub fn service_id(&self) -> i32 {
        query_value(&self.url, "service_id")
            .and_then(|value| value.parse().ok())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClientConfig {
    /// Seconds between pings. Feishu suggests 120.
    #[serde(rename = "PingInterval", default = "default_ping")]
    pub ping_interval: u64,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            ping_interval: default_ping(),
        }
    }
}

fn default_ping() -> u64 {
    120
}

/// One value out of a URL's query string.
fn query_value(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=')?;
        if name == key {
            return Some(value.to_string());
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct ConnectionEnvelope {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: Option<Connection>,
}

/// The bot's own identity.
#[derive(Debug, Clone, Deserialize)]
pub struct BotInfo {
    #[serde(default)]
    pub open_id: String,
    #[serde(default)]
    pub app_name: String,
}

/// A posted message.
#[derive(Debug, Clone, Deserialize)]
pub struct Sent {
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub chat_id: String,
}

/// One chat.
#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub chat_mode: Option<String>,
    #[serde(default)]
    pub chat_type: Option<String>,
}

/// The first part of a body, for an error message.
fn truncate(body: &str) -> &str {
    let trimmed = body.trim();
    match trimmed.char_indices().nth(200) {
        Some((index, _)) => &trimmed[..index],
        None => trimmed,
    }
}

/// A moment from a millisecond timestamp, for a test or a log line.
pub fn moment(millis: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(code: i64) -> ApiError {
        ApiError {
            code,
            message: "whatever".into(),
            retry_after: None,
        }
    }

    #[test]
    fn a_refused_credential_is_terminal_and_a_missing_bot_is_a_binding_fact() {
        for code in [99_971_601, 99_971_602, 401] {
            assert!(error(code).is_auth(), "{code}");
        }
        for code in [230_002, 230_027] {
            assert!(error(code).is_not_in_chat(), "{code}");
            assert!(!error(code).is_auth(), "{code}");
        }
        assert!(error(99_991_400).is_rate_limited());
        // An ordinary business failure is neither.
        assert!(!error(230_025).is_auth());
        assert!(!error(230_025).is_not_in_chat());
    }

    #[test]
    fn a_rate_limit_is_recognised_from_its_code_or_its_header() {
        assert!(error(99_991_400).is_rate_limited());
        let header = ApiError {
            code: 0,
            message: String::new(),
            retry_after: Some(3),
        };
        assert!(header.is_rate_limited());
    }

    #[test]
    fn the_service_id_comes_out_of_the_connection_url() {
        let connection: Connection = serde_json::from_value(serde_json::json!({
            "URL": "wss://host/path?device_id=d1&service_id=42",
            "ClientConfig": { "PingInterval": 30 },
        }))
        .unwrap();
        assert_eq!(connection.service_id(), 42);
        assert_eq!(connection.client_config.ping_interval, 30);

        // A URL with no service id, and one with none at all.
        let bare: Connection = serde_json::from_value(serde_json::json!({
            "URL": "wss://host/path"
        }))
        .unwrap();
        assert_eq!(bare.service_id(), 0);
        assert_eq!(
            bare.client_config.ping_interval, 120,
            "the documented default"
        );
    }

    #[test]
    fn a_long_body_is_truncated_on_a_character_boundary() {
        let body = "é".repeat(400);
        assert_eq!(truncate(&body).chars().count(), 200);
    }

    #[test]
    fn a_moment_from_milliseconds_is_seconds_out() {
        assert_eq!(
            moment(1_700_000_000_000).unwrap().timestamp(),
            1_700_000_000
        );
    }
}
