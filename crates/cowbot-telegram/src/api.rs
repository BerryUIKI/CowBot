//! The Bot API calls the adapter makes (ADR-0058).
//!
//! Telegram answers every method with the same envelope — `{"ok":true,"result":…}`
//! or `{"ok":false,"error_code":…,"description":…}` — which is what
//! [`call`] unwraps. The token is in the URL, so no request carries an
//! `Authorization` header and no error may echo the URL.

use std::time::Duration;

use cowbot_core::error::CowError;
use reqwest::Client as HttpClient;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;

use crate::config::TelegramConfig;
use crate::update::{Chat, Update, User};

/// Longest a `sendMessage` may take before it is treated as failed.
const SEND_TIMEOUT: Duration = Duration::from_secs(15);

/// Slack on top of the long-poll timeout, so a poll that returns empty at its
/// own deadline is not also killed by the HTTP client at the same instant.
const POLL_GRACE: Duration = Duration::from_secs(10);

/// How long a file download may take. The size cap bounds it, not the clock.
const FILE_TIMEOUT_SECS: u64 = 120;

/// A failed call, kept richer than [`CowError`].
///
/// The poll loop has to tell three cases apart that a string cannot: a conflict
/// (a webhook is set, which will never fix itself), a rate limit (wait exactly
/// as long as Telegram says), and everything else (retry with backoff).
#[derive(Debug, Clone)]
pub struct ApiError {
    /// The HTTP status.
    pub status: u16,
    /// Telegram's own message, which names the problem precisely.
    pub description: String,
    /// Seconds to wait, when Telegram said how long.
    pub retry_after: Option<u64>,
}

impl ApiError {
    /// A `getUpdates` conflict: another consumer holds the update stream.
    ///
    /// Not retryable. It happens when a webhook is registered or a second
    /// process polls the same token, and both are configuration to fix rather
    /// than a fault to wait out.
    pub fn is_conflict(&self) -> bool {
        self.status == 409
    }

    /// Whether this is a problem with what was *said*, not with the connection.
    ///
    /// A `400` will fail identically on a retry; a `429` or `5xx` will not.
    pub fn is_permanent(&self) -> bool {
        (400..500).contains(&self.status) && self.status != 429
    }

    /// Whether the failure names a rejected token.
    pub fn is_unauthorized(&self) -> bool {
        self.status == 401
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Telegram's own description, never the request URL: the URL carries the
        // token.
        write!(formatter, "telegram: {} {}", self.status, self.description)
    }
}

impl From<ApiError> for CowError {
    fn from(error: ApiError) -> Self {
        CowError::Llm(error.to_string())
    }
}

/// The update kinds this adapter asks Telegram for.
///
/// A typo here removes a whole feature without an error: the poll simply never
/// delivers that kind.
const ALLOWED_UPDATES: &[&str] = &["message", "callback_query"];

/// The calls one adapter makes.
#[derive(Clone)]
pub struct Api {
    http: HttpClient,
    config: TelegramConfig,
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
///
/// `result` stays a `Value` here rather than being typed with the call, because a
/// generic envelope would demand `Default` of every result type for no reason.
/// The caller deserializes the value it asked for.
#[derive(Debug, serde::Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error_code: Option<u16>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    parameters: Option<ErrorParameters>,
}

#[derive(Debug, serde::Deserialize)]
struct ErrorParameters {
    #[serde(default)]
    retry_after: Option<u64>,
}

impl Api {
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            http: HttpClient::new(),
            config,
        }
    }

    /// A client over an already-built HTTP client, for a test pointing it at a
    /// local fake.
    pub fn with_http(config: TelegramConfig, http: HttpClient) -> Self {
        Self { http, config }
    }

    pub fn config(&self) -> &TelegramConfig {
        &self.config
    }

    /// Who the token belongs to.
    pub async fn get_me(&self) -> Result<User, ApiError> {
        self.call("getMe", &()).await
    }

    /// Read updates, waiting up to `timeout` seconds for one to arrive.
    ///
    /// `offset` is the confirmation: sending `update_id + 1` tells Telegram the
    /// earlier batch was handled, and it will not deliver those again.
    pub async fn get_updates(&self, offset: i64, timeout: u32) -> Result<Vec<Update>, ApiError> {
        // `message` only: the rest — edits, callback queries, poll answers —
        // would need handling this adapter does not have, and asking for them
        // just fills the poll with updates it drops.
        let body = GetUpdates {
            offset,
            timeout,
            allowed_updates: ALLOWED_UPDATES,
        };
        self.call_with_timeout("getUpdates", &body, timeout as u64 + POLL_GRACE.as_secs())
            .await
    }

    /// Post a message, optionally into a forum topic.
    ///
    /// `html` is our own escaped rendering, not the caller's text; when Telegram
    /// still refuses to parse it the caller retries as plain text.
    pub async fn send_message(
        &self,
        chat_id: &str,
        html: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageId, ApiError> {
        let thread = thread_id.and_then(|id| id.parse::<i64>().ok());
        let body = SendMessage {
            chat_id,
            text: html,
            parse_mode: Some("HTML"),
            message_thread_id: thread,
            link_preview_options: Some(LinkPreview { is_disabled: false }),
        };
        let sent: SentMessage = self.call("sendMessage", &body).await?;
        Ok(MessageId(sent.message_id))
    }

    /// Post a message with no formatting at all.
    ///
    /// The fallback for a body Telegram's HTML parser refused. It cannot fail
    /// for markup reasons, so an answer is never lost to a formatting quirk.
    pub async fn send_plain_message(
        &self,
        chat_id: &str,
        text: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageId, ApiError> {
        let thread = thread_id.and_then(|id| id.parse::<i64>().ok());
        let body = SendPlain {
            chat_id,
            text,
            message_thread_id: thread,
        };
        let sent: SentMessage = self.call("sendMessage", &body).await?;
        Ok(MessageId(sent.message_id))
    }

    /// One chat, as Telegram currently describes it.
    pub async fn get_chat(&self, chat_id: &str) -> Result<Chat, ApiError> {
        self.call("getChat", &ChatById { chat_id }).await
    }

    /// One member of a chat, by account id.
    pub async fn get_chat_member(
        &self,
        chat_id: &str,
        user_id: &str,
    ) -> Result<ChatMember, ApiError> {
        self.call("getChatMember", &ChatMemberById { chat_id, user_id })
            .await
    }

    /// Ask Telegram where a file lives, so it can be downloaded.
    ///
    /// The file is not in this answer: `file_path` is a suffix the caller appends
    /// to the file address. Telegram refuses files over 20 MB here, which is the
    /// ceiling this adapter can reach.
    pub async fn get_file(&self, file_id: &str) -> Result<FileRef, ApiError> {
        self.call("getFile", &FileById { file_id }).await
    }

    /// Download one file's bytes from the address `getFile` returned.
    ///
    /// The URL carries the token, so a failure here is reported without it.
    pub async fn download_file(
        &self,
        file_path: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, ApiError> {
        let response = self
            .http
            .get(self.config.file_url(file_path))
            .timeout(Duration::from_secs(FILE_TIMEOUT_SECS))
            .send()
            .await
            .map_err(|error| ApiError {
                status: 0,
                description: format!("could not reach Telegram: {error}"),
                retry_after: None,
            })?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(ApiError {
                status,
                description: format!("Telegram refused the file download ({status})"),
                retry_after: None,
            });
        }
        if let Some(length) = response.content_length() {
            if length > max_bytes {
                return Err(ApiError {
                    status,
                    description: format!(
                        "the file is {length} bytes, above the {max_bytes}-byte limit"
                    ),
                    retry_after: None,
                });
            }
        }
        let bytes = response.bytes().await.map_err(|error| ApiError {
            status,
            description: format!("the download broke off: {error}"),
            retry_after: None,
        })?;
        if bytes.len() as u64 > max_bytes {
            return Err(ApiError {
                status,
                description: format!(
                    "the file grew past the {max_bytes}-byte limit and was not kept"
                ),
                retry_after: None,
            });
        }
        Ok(bytes.to_vec())
    }

    /// Answer a button press, so the client stops showing a spinner.
    ///
    /// Telegram requires this within a few seconds. `text` is shown as a toast
    /// over the chat, which is how a refusal is reported to the person who
    /// tapped.
    pub async fn answer_callback(
        &self,
        callback_id: &str,
        text: &str,
        alert: bool,
    ) -> Result<(), ApiError> {
        let answered: bool = self
            .call(
                "answerCallbackQuery",
                &AnswerCallback {
                    callback_query_id: callback_id,
                    text,
                    show_alert: alert,
                },
            )
            .await?;
        let _ = answered;
        Ok(())
    }

    /// Post a message with an inline keyboard of approval buttons.
    ///
    /// Each button carries the answer and the approval id, which is all a
    /// 64-byte `callback_data` can hold and all a decision needs (ADR-0060).
    pub async fn send_approval(
        &self,
        chat_id: &str,
        html: &str,
        approval_id: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageId, ApiError> {
        let thread = thread_id.and_then(|id| id.parse::<i64>().ok());
        let body = SendWithKeyboard {
            chat_id,
            text: html,
            parse_mode: Some("HTML"),
            message_thread_id: thread,
            reply_markup: InlineKeyboard {
                inline_keyboard: vec![vec![
                    Button {
                        text: "Approve",
                        callback_data: approval_id_for(approval_id, true),
                    },
                    Button {
                        text: "Reject",
                        callback_data: approval_id_for(approval_id, false),
                    },
                ]],
            },
        };
        let sent: SentMessage = self.call("sendMessage", &body).await?;
        Ok(MessageId(sent.message_id))
    }

    /// Replace a posted message's text, dropping its keyboard.
    ///
    /// Used once a request is decided: the card stays where the question was, so
    /// a person scrolling back learns the outcome instead of finding buttons that
    /// no longer do anything.
    pub async fn edit_message(
        &self,
        chat_id: &str,
        message_id: i64,
        html: &str,
    ) -> Result<(), ApiError> {
        let edited: serde_json::Value = self
            .call(
                "editMessageText",
                &EditMessage {
                    chat_id,
                    message_id,
                    text: html,
                    parse_mode: Some("HTML"),
                },
            )
            .await?;
        let _ = edited;
        Ok(())
    }

    async fn call<T: DeserializeOwned, B: Serialize>(
        &self,
        method: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        self.call_with_timeout(method, body, SEND_TIMEOUT.as_secs())
            .await
    }

    async fn call_with_timeout<T: DeserializeOwned, B: Serialize>(
        &self,
        method: &str,
        body: &B,
        seconds: u64,
    ) -> Result<T, ApiError> {
        let response = self
            .http
            .post(self.config.method_url(method))
            .timeout(Duration::from_secs(seconds))
            .json(body)
            .send()
            .await
            .map_err(|error| ApiError {
                // A transport failure has no status. Zero reads as "not the
                // server's answer" here, which is what the callers need.
                status: 0,
                description: format!("could not reach Telegram: {error}"),
                retry_after: None,
            })?;

        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        let Ok(envelope) = serde_json::from_str::<Envelope>(&body) else {
            return Err(ApiError {
                status,
                description: format!("unexpected answer from Telegram: {}", truncate(&body)),
                retry_after: None,
            });
        };
        if envelope.ok {
            if let Some(result) = envelope.result {
                return serde_json::from_value(result).map_err(|error| ApiError {
                    status,
                    description: format!(
                        "Telegram answered a shape this adapter cannot read: {error}"
                    ),
                    retry_after: None,
                });
            }
        }
        Err(ApiError {
            // Telegram answers a refusal with a 200 or with its own error code;
            // the envelope's number is the more precise of the two.
            status: envelope.error_code.unwrap_or(status),
            description: envelope
                .description
                .unwrap_or_else(|| "no description".to_string()),
            retry_after: envelope
                .parameters
                .and_then(|parameters| parameters.retry_after),
        })
    }
}

/// The id Telegram assigned to a posted message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageId(pub i64);

/// A freshly posted message, of which only the id is used.
#[derive(Debug, Clone, serde::Deserialize)]
struct SentMessage {
    message_id: i64,
}

/// A member of a chat.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ChatMember {
    /// `creator`, `administrator`, `member`, `restricted`, `left`, `kicked`.
    pub status: String,
    /// Whether a restricted member is still in the chat.
    #[serde(default)]
    pub is_member: Option<bool>,
}

impl ChatMember {
    /// Whether this member may post.
    ///
    /// A channel is the case that needs the distinction: a bot added to one
    /// without admin rights can read it and its posts are refused.
    pub fn can_post(&self) -> bool {
        matches!(self.status.as_str(), "creator" | "administrator")
    }
}

#[derive(Debug, Serialize)]
struct GetUpdates {
    offset: i64,
    timeout: u32,
    allowed_updates: &'static [&'static str],
}

#[derive(Debug, Serialize)]
struct SendMessage<'a> {
    chat_id: &'a str,
    text: &'a str,
    parse_mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    link_preview_options: Option<LinkPreview>,
}

#[derive(Debug, Serialize)]
struct SendPlain<'a> {
    chat_id: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
}

#[derive(Debug, Serialize)]
struct LinkPreview {
    is_disabled: bool,
}

#[derive(Debug, Serialize)]
struct ChatById<'a> {
    chat_id: &'a str,
}

#[derive(Debug, Serialize)]
struct ChatMemberById<'a> {
    chat_id: &'a str,
    user_id: &'a str,
}

#[derive(Debug, Serialize)]
struct FileById<'a> {
    file_id: &'a str,
}

#[derive(Debug, Serialize)]
struct AnswerCallback<'a> {
    callback_query_id: &'a str,
    text: &'a str,
    show_alert: bool,
}

#[derive(Debug, Serialize)]
struct SendWithKeyboard<'a> {
    chat_id: &'a str,
    text: &'a str,
    parse_mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
    reply_markup: InlineKeyboard,
}

#[derive(Debug, Serialize)]
struct InlineKeyboard {
    inline_keyboard: Vec<Vec<Button>>,
}

#[derive(Debug, Serialize)]
struct Button {
    text: &'static str,
    callback_data: String,
}

#[derive(Debug, Serialize)]
struct EditMessage<'a> {
    chat_id: &'a str,
    message_id: i64,
    text: &'a str,
    parse_mode: Option<&'a str>,
}

/// Where Telegram says a file is.
#[derive(Debug, Clone, Deserialize)]
pub struct FileRef {
    /// The suffix to append to the API's file path; absent when Telegram refused.
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
}

/// The `callback_data` for one answer to one approval.
///
/// `approve:` plus a UUID is 44 bytes, inside Telegram's 64-byte limit. The
/// prefix is what tells the inbound loop which answer was chosen, so the id
/// alone is not enough.
pub fn approval_id_for(approval_id: &str, approved: bool) -> String {
    format!(
        "{}:{approval_id}",
        if approved { "approve" } else { "reject" }
    )
}

/// Read a `callback_data` this adapter wrote.
///
/// Returns `None` for anything else, so an interaction the adapter did not
/// compose — a button from another bot's message in a group — is ignored rather
/// than read as a decision about whatever id it happens to contain.
pub fn parse_callback_data(data: &str) -> Option<(bool, &str)> {
    let (answer, approval_id) = data.split_once(':')?;
    let approved = match answer {
        "approve" => true,
        "reject" => false,
        _ => return None,
    };
    (!approval_id.is_empty()).then_some((approved, approval_id))
}

/// The first part of a body, for an error message.
fn truncate(body: &str) -> &str {
    let trimmed = body.trim();
    match trimmed.char_indices().nth(200) {
        Some((index, _)) => &trimmed[..index],
        None => trimmed,
    }
}

/// Map a `getChat` refusal onto whether the binding can work.
///
/// Telegram answers `400 chat not found` for both a group the bot is not in and
/// a person who has never messaged it, and `403 bot is not a member` for a chat
/// it was removed from. Those three are answers about the binding. Anything else
/// — a 401, a 5xx, a transport failure — is not, and reporting it as
/// "unreachable" would send an operator to fix something that is fine.
pub fn chat_unreachable_reason(error: &ApiError) -> Option<String> {
    match error.status {
        400 if error.description.to_lowercase().contains("not found") => Some(
            "Telegram has no such chat for this bot. In a private chat the person has to \
             message the bot once before the bot can post to them; in a group or channel the \
             bot has to be added"
                .into(),
        ),
        403 => Some(format!(
            "the bot is not allowed in this Telegram chat: {}",
            error.description
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_three_chat_refusals_are_answers_about_the_binding() {
        let not_found = ApiError {
            status: 400,
            description: "Bad Request: chat not found".into(),
            retry_after: None,
        };
        assert!(chat_unreachable_reason(&not_found).is_some());

        let forbidden = ApiError {
            status: 403,
            description: "Forbidden: bot is not a member of the chat".into(),
            retry_after: None,
        };
        assert!(chat_unreachable_reason(&forbidden).is_some());

        // A rejected token, a server fault, and a transport failure are all
        // *not* answers about the chat; calling them unreachable would tell an
        // operator to fix a binding that is fine.
        for status in [401, 500, 0] {
            let other = ApiError {
                status,
                description: "whatever".into(),
                retry_after: None,
            };
            assert!(chat_unreachable_reason(&other).is_none(), "status {status}");
        }
        // A 400 about something else is not this 400.
        let malformed = ApiError {
            status: 400,
            description: "Bad Request: message text is empty".into(),
            retry_after: None,
        };
        assert!(chat_unreachable_reason(&malformed).is_none());
    }

    #[test]
    fn a_conflict_is_not_retryable_and_a_rate_limit_is_not_permanent() {
        let conflict = ApiError {
            status: 409,
            description: "Conflict: can't use getUpdates method while webhook is active".into(),
            retry_after: None,
        };
        assert!(conflict.is_conflict());

        let limit = ApiError {
            status: 429,
            description: "Too Many Requests".into(),
            retry_after: Some(7),
        };
        assert!(!limit.is_permanent(), "a 429 clears up on its own");

        let malformed = ApiError {
            status: 400,
            description: "Bad Request".into(),
            retry_after: None,
        };
        assert!(malformed.is_permanent());

        assert!(ApiError {
            status: 401,
            description: "Unauthorized".into(),
            retry_after: None,
        }
        .is_unauthorized());
    }

    #[test]
    fn only_an_administrator_may_post_to_a_channel() {
        // The distinction `getChatMember` exists for: a plain member can read a
        // channel and its posts are refused.
        for status in ["member", "restricted", "left", "kicked"] {
            assert!(
                !ChatMember {
                    status: status.into(),
                    is_member: Some(true),
                }
                .can_post(),
                "{status} cannot post to a channel"
            );
        }
        for status in ["creator", "administrator"] {
            assert!(ChatMember {
                status: status.into(),
                is_member: None,
            }
            .can_post());
        }
    }

    #[test]
    fn an_error_never_prints_the_token() {
        // The token lives in the URL, so a message built from a status and a
        // description is the only safe shape.
        let error = ApiError {
            status: 401,
            description: "Unauthorized".into(),
            retry_after: None,
        };
        let printed = error.to_string();
        assert!(printed.contains("401"));
        assert!(!printed.contains("bot"));
    }

    #[test]
    fn a_long_body_is_truncated_on_a_character_boundary() {
        let body = "é".repeat(400);
        let cut = truncate(&body);
        assert_eq!(cut.chars().count(), 200);
    }
}
