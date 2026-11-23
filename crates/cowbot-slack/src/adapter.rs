//! The `Integration` implementation for Slack (ADR-0059).
//!
//! Translation only. The dedupe, the echo suppression, the identity mapping and
//! the outbound mirror belong to the shared layer; what belongs here is what only
//! Slack knows — that a mention is `<@U123>` and becomes a `@Name` only after a
//! lookup, that `&`, `<` and `>` arrive escaped, that a thread is a `ts`, and that
//! a button press arrives on the same socket as a message.

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use cowbot_core::cancel::CancelSignal;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::integration::IntegrationKind;
use cowbot_integrations::adapter::{Integration, IntegrationContext};
use cowbot_integrations::approvals::{ApprovalPosting, ApprovalPrompt, Refused};
use cowbot_integrations::model::{
    Attachment, AttachmentKind, BindingHealth, DeliveredId, ExternalAuthor, OutboundContent,
    OutboundTarget,
};
use cowbot_integrations::notice::report_binding_health;
use tracing::{debug, info, warn};

use crate::api::{Api, FileInfo, Identity, SlackError};
use crate::config::SlackConfig;

/// One bot connection to one Slack workspace.
pub struct SlackIntegration {
    api: Api,
    /// The bot's own identity, read once at startup.
    ///
    /// Without it the adapter cannot tell its own message from a person's, and
    /// both failures are silent.
    identity: std::sync::RwLock<Option<Identity>>,
}

impl SlackIntegration {
    pub fn new(config: SlackConfig) -> Self {
        Self {
            api: Api::new(config),
            identity: std::sync::RwLock::new(None),
        }
    }

    /// A connection over an already-built client, for tests.
    pub fn with_api(api: Api) -> Self {
        Self {
            api,
            identity: std::sync::RwLock::new(None),
        }
    }

    pub fn identity(&self) -> Option<Identity> {
        self.identity
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Record the bot's identity. `pub` so a test can skip the network.
    pub fn set_identity(&self, identity: Identity) {
        *self
            .identity
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(identity);
    }

    /// Turn one `message` event into the pipeline's shape, or `None` to drop it.
    ///
    /// `bot_user_id` is the bot's own id: a bot's own post is dropped here so the
    /// dedupe table stays free of it, and another bot's is passed through marked
    /// as a bot so the shared pipeline records why it was ignored.
    pub fn inbound(
        &self,
        event: &serde_json::Value,
        bot_user_id: &str,
    ) -> Option<cowbot_integrations::InboundMessage> {
        let kind = event.get("type").and_then(|value| value.as_str());
        if kind != Some("message") {
            return None;
        }
        // Slack labels its own housekeeping as a message with a subtype:
        // `bot_message`, `channel_join`, `message_changed`, and others. A
        // `message_changed` also carries a *new* `ts`, so treating it as a
        // message would store an edit as a second turn.
        if event.get("subtype").is_some() {
            return None;
        }

        let channel = event.get("channel")?.as_str()?.to_string();
        let user = event.get("user").and_then(|value| value.as_str());
        let author_id = user
            .or_else(|| event.get("bot_id").and_then(|value| value.as_str()))
            .unwrap_or_default()
            .to_string();
        if author_id.is_empty() {
            return None;
        }
        // The bot's own message. The shared pipeline drops it too, but doing it
        // here keeps the dedupe table free of the bot's own traffic.
        if author_id == bot_user_id {
            return None;
        }

        let raw = event
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let text = unescape(raw);
        let thread = event
            .get("thread_ts")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let is_bot = event.get("bot_id").is_some()
            || event.get("subtype").and_then(|value| value.as_str()) == Some("bot_message");

        Some(cowbot_integrations::InboundMessage {
            // `channel:ts` rather than the bare `ts`: a `ts` is unique within a
            // channel, and two channels can hand out the same value.
            event_id: format!("{channel}:{}", event.get("ts")?.as_str()?),
            external_conversation_id: channel,
            external_thread_id: thread,
            author: ExternalAuthor {
                external_id: author_id,
                display_name: event
                    .get("username")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string(),
                is_bot,
            },
            text: translate_mentions(&text),
            // Slack delivers every message in a channel the bot is in — it needs
            // the `channels:history` scope and to be invited, but not a mention.
            // Answering all of them is the busy-room problem, so addressing means
            // a mention of this bot.
            addresses_bot: mentions_of(raw, bot_user_id),
            attachments: attachments_of(event),
            created_at: moment(event.get("ts").and_then(|value| value.as_str())),
        })
    }
}

/// Undo Slack's mrkdwn escaping.
///
/// Slack sends `&amp;`, `&lt;` and `&gt;` in the text because it uses `<` for its
/// own markup. Storing them would show the entities to every reader in CowBot.
/// The order matters: `&amp;` is undone last, or `&amp;lt;` becomes `<`.
pub fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Whether `raw` mentions `user_id`.
///
/// Read from the raw text, before unescaping: a mention is `<@U123>`, which
/// unescaping would not change but a lookup would lose.
pub fn mentions_of(raw: &str, user_id: &str) -> bool {
    let needle = format!("<@{user_id}");
    let Some(start) = raw.find(&needle) else {
        return false;
    };
    let rest = &raw[start + needle.len()..];
    // `<@U123>`, and Slack's labelled form `<@U123|name>`.
    rest.starts_with('>') || rest.starts_with('|')
}

/// Turn every mention of anybody into a `@Name`.
///
/// CowBot's `parse_mentions` reads `@` + a name, and a Slack id is not a name.
/// Only ids the shared layer can resolve matter for addressing, so the
/// translation keeps the id when no name is known: it reads as an id, which is
/// honest, rather than as a name that might belong to someone else.
pub fn translate_mentions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<@") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('>') else {
            // An unclosed mention stays literal rather than swallowing the rest.
            out.push_str("<@");
            rest = after;
            continue;
        };
        let inner = &after[..end];
        // `<@U123|name>` carries the name Slack shows; `<@U123>` does not.
        let name = inner.split_once('|').map(|(_, name)| name).unwrap_or(inner);
        let name = name.trim();
        if name.is_empty() {
            out.push_str("<@>");
        } else {
            out.push('@');
            out.push_str(name);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The files a message carried.
fn attachments_of(event: &serde_json::Value) -> Vec<Attachment> {
    let Some(files) = event.get("files").and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    files
        .iter()
        .filter_map(|file| {
            let id = file.get("id")?.as_str()?.to_string();
            let mime = file
                .get("mimetype")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            Some(Attachment {
                external_id: id,
                kind: AttachmentKind::from_mime(mime.as_deref()),
                file_name: file
                    .get("name")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                mime_type: mime,
                size_bytes: file.get("size").and_then(|value| value.as_u64()),
            })
        })
        .collect()
}

/// A Slack `ts`, which is seconds with microseconds after a dot.
fn moment(ts: Option<&str>) -> chrono::DateTime<Utc> {
    let seconds = ts
        .and_then(|ts| ts.split('.').next())
        .and_then(|seconds| seconds.parse::<i64>().ok());
    // A `ts` that does not parse falls back to now rather than to the epoch: a
    // message dated 1970 sorts to the top of every history query, and the
    // timestamp is only ever used for display.
    seconds
        .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single())
        .unwrap_or_else(Utc::now)
}

/// Check that the bot can actually post and read in a channel.
///
/// `pub` so the startup and bind-time paths read the same logic the trait method
/// does.
pub async fn binding_health(
    api: &Api,
    identity: Option<&Identity>,
    external_id: &str,
) -> BindingHealth {
    let Some(_identity) = identity else {
        return BindingHealth::Unknown(
            "the bot's own identity was not discovered, so its access to the channel was not \
             checked"
                .into(),
        );
    };
    // `conversations.info` names the channel and says whether it is archived;
    // `conversations.members` is what proves the bot is in it. Slack answers
    // `not_in_channel` for a public channel the bot is not a member of, which is
    // the silent fault this exists for.
    match api.call_conversations_info(external_id).await {
        Ok(_) => {}
        Err(error) if error.is_not_in_channel() => {
            return BindingHealth::Unreachable(format!(
                "'{external_id}' is a Slack channel the bot cannot see; invite it with \
                 /invite, or the binding will receive nothing"
            ));
        }
        Err(error) => return BindingHealth::Unknown(error.to_string()),
    }

    match api.call_conversations_members(external_id).await {
        Ok(members) => {
            let Some(identity) = identity else {
                return BindingHealth::Unknown("the bot's identity is unknown".into());
            };
            if members.iter().any(|member| member == &identity.user_id) {
                BindingHealth::Reachable
            } else {
                BindingHealth::Unreachable(format!(
                    "'{external_id}' does not list the bot as a member; invite it with /invite, \
                     or the binding will receive nothing"
                ))
            }
        }
        Err(error) if error.is_not_in_channel() => BindingHealth::Unreachable(format!(
            "'{external_id}' does not list the bot as a member; invite it with /invite, or the \
             binding will receive nothing"
        )),
        Err(error) => BindingHealth::Unknown(error.to_string()),
    }
}

#[async_trait]
impl Integration for SlackIntegration {
    fn kind(&self) -> IntegrationKind {
        IntegrationKind::Slack
    }

    async fn run(&self, ctx: IntegrationContext, cancel: CancelSignal) -> CowResult<()> {
        // The identity first: without it the adapter cannot recognise its own
        // messages or its own mentions, and both failures are silent.
        let me = self.api.auth_test().await.map_err(|error| {
            CowError::Config(format!(
                "Slack refused the bot token ({}); check the integration's configuration",
                error.error
            ))
        })?;
        let bot_user_id = me.user_id.clone();
        self.set_identity(me.clone());

        info!(
            integration = %ctx.integration.id,
            bot = %me.user,
            team = me.team_id.as_deref().unwrap_or("-"),
            "the Slack adapter is running"
        );

        report_binding_health(&ctx, self).await;

        let sink = ctx.sink.clone();
        let ctx = ctx.clone();
        let adapter = self.clone();
        let bot = bot_user_id.clone();
        crate::socket::run(self.api.clone(), cancel, move |envelope| {
            let sink = sink.clone();
            let ctx = ctx.clone();
            let adapter = adapter.clone();
            let bot = bot.clone();
            async move {
                match envelope.kind.as_str() {
                    "events_api" => {
                        let Some(payload) = &envelope.payload else {
                            return Ok(());
                        };
                        // Only `event_callback` carries a message; `url_verification`
                        // and the app-rate-limit notices do not.
                        if payload.get("type").and_then(|value| value.as_str())
                            != Some("event_callback")
                        {
                            return Ok(());
                        }
                        let Some(event) = payload.get("event") else {
                            return Ok(());
                        };
                        match adapter.inbound(event, &bot) {
                            Some(message) => sink.accept(message).await.map(|_| ()),
                            None => Ok(()),
                        }
                    }
                    "interactive" => {
                        let Some(payload) = &envelope.payload else {
                            return Ok(());
                        };
                        handle_interaction(&ctx, payload).await?;
                        Ok(())
                    }
                    _ => Ok(()),
                }
            }
        })
        .await
    }

    async fn deliver(
        &self,
        target: &OutboundTarget,
        content: &OutboundContent,
    ) -> CowResult<DeliveredId> {
        let text = cowbot_integrations::label_chunk(content);
        let posted = self
            .api
            .post_message(
                &target.external_conversation_id,
                &text,
                target.thread_id.as_deref(),
                None,
            )
            .await
            .map_err(CowError::from)?;
        Ok(DeliveredId(posted.ts))
    }

    fn chunk_limit(&self) -> cowbot_integrations::ChunkLimit {
        // Slack's `text` field accepts 40 000 characters, but a Block Kit message
        // is limited to 50 blocks and a reader loses their place long before
        // that. The shared 4 000-character budget is kept so one answer reads the
        // same in every IM.
        cowbot_integrations::ChunkLimit::Characters(cowbot_integrations::outbound::MAX_CHUNK_CHARS)
    }

    async fn check_binding(&self, external_id: &str) -> BindingHealth {
        binding_health(&self.api, self.identity().as_ref(), external_id).await
    }

    async fn fetch_attachment(
        &self,
        attachment: &Attachment,
        max_bytes: u64,
    ) -> CowResult<Vec<u8>> {
        // Two calls again: `files.info` is what carries the private URL, and the
        // URL is what the download needs. The host check inside `file_bytes` is
        // what stops an event from pointing the bot token somewhere else.
        let info: FileInfo = self
            .api
            .file_info(&attachment.external_id)
            .await
            .map_err(CowError::from)?;
        let Some(url) = info.url_private else {
            return Err(CowError::Config(
                "Slack did not give a download address for the file".into(),
            ));
        };
        self.api
            .file_bytes(&url, max_bytes)
            .await
            .map_err(CowError::from)
    }

    /// Post an approval request as a Block Kit message with two buttons.
    ///
    /// Block Kit is the only Block Kit-shaped thing in this crate, so it is
    /// written inline rather than modelled: a card Slack must render exactly is
    /// JSON, and a Rust type would be a second place to get it wrong.
    async fn post_approval(
        &self,
        target: &OutboundTarget,
        prompt: &ApprovalPrompt,
    ) -> CowResult<ApprovalPosting> {
        let blocks = crate::blocks::approval(&prompt.describe(), &prompt.approval_id);
        let posted = self
            .api
            .post_message(
                &target.external_conversation_id,
                &prompt.describe(),
                target.thread_id.as_deref(),
                Some(blocks),
            )
            .await
            .map_err(CowError::from)?;
        Ok(ApprovalPosting {
            delivered: DeliveredId(posted.ts),
            buttons: true,
        })
    }

    async fn update_approval(
        &self,
        target: &OutboundTarget,
        posting: &ApprovalPosting,
        summary: &str,
    ) -> CowResult<()> {
        // The blocks are dropped rather than edited: a decided card's buttons must
        // go, and `chat.update` replaces the whole block array.
        self.api
            .update_message(
                &target.external_conversation_id,
                &posting.delivered.0,
                summary,
                Some(crate::blocks::decided(summary)),
            )
            .await
            .map_err(CowError::from)
    }
}

/// Handle one `block_actions` payload as an approval decision.
///
/// Slack's payload nests the action under `actions[0].value`, and the account
/// that tapped under `user.id`. A payload from a button this adapter did not
/// compose is ignored rather than reported: there is no toast to show without an
/// inbound request to answer.
async fn handle_interaction(
    ctx: &IntegrationContext,
    payload: &serde_json::Value,
) -> CowResult<()> {
    if payload.get("type").and_then(|value| value.as_str()) != Some("block_actions") {
        return Ok(());
    }
    let Some(value) = payload
        .get("actions")
        .and_then(|actions| actions.as_array())
        .and_then(|actions| actions.first())
        .and_then(|action| action.get("value"))
        .and_then(|value| value.as_str())
    else {
        return Ok(());
    };
    let Some((approved, approval_id)) = parse_action(value) else {
        return Ok(());
    };
    let Some(user_id) = payload
        .get("user")
        .and_then(|user| user.get("id"))
        .and_then(|value| value.as_str())
    else {
        return Ok(());
    };
    let display_name = payload
        .get("user")
        .and_then(|user| user.get("username"))
        .and_then(|value| value.as_str())
        .unwrap_or(user_id);

    let author = ExternalAuthor {
        external_id: user_id.to_string(),
        display_name: display_name.to_string(),
        is_bot: false,
    };
    match cowbot_integrations::approvals::decide_from_im(ctx, approval_id, approved, &author).await
    {
        Ok(_) => debug!(approval = %approval_id, "an approval was decided from Slack"),
        // A refusal is logged and not surfaced: Slack's interaction socket has no
        // channel for a toast, so the person sees the card unchanged and the
        // operator sees why here.
        Err(Refused::NotPending(reason)) | Err(Refused::NotAllowed(reason)) => warn!(
            approval = %approval_id,
            %reason,
            "a Slack approval tap did not count"
        ),
    }
    Ok(())
}

/// Read a button value this adapter composed.
pub fn parse_action(value: &str) -> Option<(bool, &str)> {
    let (answer, approval_id) = value.split_once(':')?;
    let approved = match answer {
        "approve" => true,
        "reject" => false,
        _ => return None,
    };
    (!approval_id.is_empty()).then_some((approved, approval_id))
}

/// The value one approval button carries.
///
/// Slack has no documented byte cap on an action value, so the shape matches
/// Telegram's for uniformity — but it is not under pressure the way a 64-byte
/// `callback_data` is.
pub fn action_value(approval_id: &str, approved: bool) -> String {
    format!(
        "{}:{approval_id}",
        if approved { "approve" } else { "reject" }
    )
}

impl std::fmt::Debug for SlackIntegration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SlackIntegration")
            .field("api_base", &self.api.config().api_base)
            .field("bot", &self.identity().map(|identity| identity.user))
            .finish_non_exhaustive()
    }
}

impl Clone for SlackIntegration {
    fn clone(&self) -> Self {
        Self {
            api: self.api.clone(),
            identity: std::sync::RwLock::new(self.identity()),
        }
    }
}

/// A channel id is `C`/`G`/`D` followed by the rest, so a pasted name is a typo
/// worth reporting at bind time.
pub fn channel_id_shape_is_valid(external_id: &str) -> bool {
    let trimmed = external_id.trim();
    let mut characters = trimmed.chars();
    matches!(characters.next(), Some('C' | 'G' | 'D'))
        && characters.all(|character| character.is_ascii_alphanumeric())
}

/// The error a Slack call would have produced, for a test asserting on a mapping.
pub fn is_auth_error(error: &SlackError) -> bool {
    error.is_auth()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(json: serde_json::Value) -> serde_json::Value {
        json
    }

    fn adapter() -> SlackIntegration {
        let integration = SlackIntegration::new(
            SlackConfig::from_json(r#"{"bot_token":"xoxb-1","app_token":"xapp-1"}"#).unwrap(),
        );
        integration.set_identity(Identity {
            user_id: "UBOT".into(),
            user: "cowbot".into(),
            team_id: Some("T1".into()),
        });
        integration
    }

    fn message(text: &str, channel: &str) -> serde_json::Value {
        event(serde_json::json!({
            "type": "message",
            "channel": channel,
            "user": "U1",
            "text": text,
            "ts": "1700000000.000100",
        }))
    }

    #[test]
    fn a_mention_becomes_a_name_and_escaping_is_undone() {
        // `<@U1>` is a Slack id, not a name, and `&amp;` is what Slack sends for
        // `&`. Storing either unchanged shows the reader a machine token.
        assert_eq!(translate_mentions("hi <@U1>"), "hi @U1");
        assert_eq!(translate_mentions("hi <@U1|cowbot>"), "hi @cowbot");
        assert_eq!(unescape("a &amp; b &lt;c&gt;"), "a & b <c>");
        // Order matters: undoing `&amp;` first would turn `&amp;lt;` into `<`.
        assert_eq!(unescape("&amp;lt;"), "&lt;");
    }

    #[test]
    fn an_unclosed_mention_stays_literal_rather_than_swallowing_the_text() {
        assert_eq!(translate_mentions("see <@U1 and more"), "see <@U1 and more");
        assert_eq!(translate_mentions("plain"), "plain");
    }

    #[test]
    fn a_mention_of_the_bot_addresses_it_and_another_mention_does_not() {
        assert!(mentions_of("hello <@UBOT> there", "UBOT"));
        assert!(mentions_of("<@UBOT|name> hi", "UBOT"));
        // A prefix is not a mention of this bot.
        assert!(!mentions_of("<@UBOT2> hi", "UBOT"));
        assert!(!mentions_of("no mention", "UBOT"));
    }

    #[test]
    fn a_message_event_becomes_an_inbound_message() {
        let message = adapter()
            .inbound(&message("<@UBOT> status?", "C1"), "UBOT")
            .unwrap();
        assert_eq!(message.external_conversation_id, "C1");
        assert_eq!(message.event_id, "C1:1700000000.000100");
        assert!(message.addresses_bot);
        assert_eq!(message.text, "@UBOT status?");
        assert!(!message.author.is_bot);
    }

    #[test]
    fn the_bots_own_message_and_a_subtyped_one_are_dropped() {
        let own = event(serde_json::json!({
            "type": "message", "channel": "C1", "user": "UBOT", "text": "hi",
            "ts": "1700000000.000100",
        }));
        assert!(adapter().inbound(&own, "UBOT").is_none());

        // An edit carries a new `ts`, so treating it as a message would store it
        // as a second turn.
        let edited = event(serde_json::json!({
            "type": "message", "subtype": "message_changed", "channel": "C1",
            "user": "U1", "text": "hi", "ts": "1700000000.000200",
        }));
        assert!(adapter().inbound(&edited, "UBOT").is_none());

        let join = event(serde_json::json!({
            "type": "message", "subtype": "channel_join", "channel": "C1",
            "user": "U1", "text": "joined", "ts": "1700000000.000300",
        }));
        assert!(adapter().inbound(&join, "UBOT").is_none());
    }

    #[test]
    fn another_bots_message_is_passed_through_marked_as_a_bot() {
        let other = event(serde_json::json!({
            "type": "message", "channel": "C1", "bot_id": "B9", "subtype": "bot_message",
            "text": "<@UBOT> ping", "ts": "1700000000.000400",
        }));
        // The subtype filter drops it before the bot flag is read, which is what
        // the shared pipeline would do anyway.
        assert!(adapter().inbound(&other, "UBOT").is_none());
    }

    #[test]
    fn a_thread_reply_keeps_its_thread() {
        let reply = event(serde_json::json!({
            "type": "message", "channel": "C1", "user": "U1", "text": "hello",
            "ts": "1700000000.000500", "thread_ts": "1700000000.000100",
        }));
        let message = adapter().inbound(&reply, "UBOT").unwrap();
        assert_eq!(
            message.external_thread_id.as_deref(),
            Some("1700000000.000100")
        );
    }

    #[test]
    fn a_file_on_a_message_becomes_an_attachment() {
        let with_file = event(serde_json::json!({
            "type": "message", "channel": "C1", "user": "U1", "text": "see attached",
            "ts": "1700000000.000600",
            "files": [{
                "id": "F1", "name": "rows.csv", "mimetype": "text/csv",
                "size": 2048, "url_private": "https://files.slack.com/x"
            }],
        }));
        let message = adapter().inbound(&with_file, "UBOT").unwrap();
        assert_eq!(message.attachments.len(), 1);
        assert_eq!(message.attachments[0].external_id, "F1");
        assert_eq!(message.attachments[0].kind, AttachmentKind::File);
        assert_eq!(message.attachments[0].size_bytes, Some(2048));
    }

    #[test]
    fn a_button_value_is_read_back_and_a_foreign_one_is_ignored() {
        assert_eq!(parse_action("approve:a1"), Some((true, "a1")));
        assert_eq!(parse_action("reject:a1"), Some((false, "a1")));
        assert_eq!(parse_action("a1"), None);
        assert_eq!(parse_action("other:a1"), None);
        assert_eq!(parse_action("approve:"), None);
        assert_eq!(action_value("a1", true), "approve:a1");
    }

    #[test]
    fn a_channel_id_is_a_letter_then_an_id() {
        assert!(channel_id_shape_is_valid("C024BE91L"));
        assert!(channel_id_shape_is_valid("G0123"));
        assert!(channel_id_shape_is_valid("D0123"));
        assert!(!channel_id_shape_is_valid("#general"));
        assert!(!channel_id_shape_is_valid(""));
        assert!(!channel_id_shape_is_valid("C-1"));
    }

    #[test]
    fn a_slack_ts_is_read_as_seconds() {
        let read = moment(Some("1700000000.123456"));
        assert_eq!(read.timestamp(), 1_700_000_000);
        assert!(moment(None).timestamp() > 1_600_000_000);
        assert!(moment(Some("nonsense")).timestamp() > 1_600_000_000);
    }

    #[test]
    fn a_debug_print_never_carries_a_token() {
        let integration = SlackIntegration::new(
            SlackConfig::from_json(r#"{"bot_token":"xoxb-secret","app_token":"xapp-secret"}"#)
                .unwrap(),
        );
        let printed = format!("{integration:?}");
        assert!(!printed.contains("secret"), "{printed}");
    }
}
