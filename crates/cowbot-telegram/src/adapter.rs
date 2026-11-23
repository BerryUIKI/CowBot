//! The `Integration` implementation for Telegram (ADR-0058).
//!
//! Translation only. The dedupe, the echo suppression, the identity mapping and
//! the outbound mirror belong to the shared layer; what belongs here is what
//! only Telegram knows — that a mention is an entity offset counted in UTF-16
//! units, that a private chat needs no mention, that a forum topic is the only
//! real thread, and that the poll confirms itself by advancing an offset.
//!
//! ```text
//! private chat  → every message is addressed  → a direct conversation
//! group/channel → privacy mode shows mentions → a mention starts a turn
//! ```

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use cowbot_core::cancel::CancelSignal;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::integration::IntegrationKind;
use cowbot_integrations::adapter::{Integration, IntegrationContext};
use cowbot_integrations::approvals::{ApprovalPosting, ApprovalPrompt, Refused};
use cowbot_integrations::model::{
    Attachment, BindingHealth, DeliveredId, ExternalAuthor, OutboundContent, OutboundTarget,
};
use cowbot_integrations::notice::report_binding_health;
use tracing::{debug, info, warn};

use crate::api::{chat_unreachable_reason, parse_callback_data, Api, ApiError};

/// Telegram's documented `sendMessage` limit, in UTF-16 code units.
pub const MAX_MESSAGE_UNITS: usize = 4_096;
use crate::config::TelegramConfig;
use crate::render::to_html;
use crate::update::{addresses_bot, BotHandle, Update};

/// How long one `getUpdates` call may wait for a message.
///
/// Thirty seconds is Telegram's own suggestion for long polling: long enough
/// that an idle bot makes two calls a minute, short enough that a cancellation
/// is noticed promptly.
pub const POLL_TIMEOUT_SECS: u32 = 30;

/// Wait before re-polling after a failure the loop did not cause.
const RETRY_INITIAL: std::time::Duration = std::time::Duration::from_secs(1);

/// The longest backoff between failed polls.
const RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(60);

/// One bot connection to Telegram.
pub struct TelegramIntegration {
    api: Api,
    /// The bot's own identity, read once at startup.
    ///
    /// Held in a plain lock rather than configured: a bot that does not know its
    /// own id cannot tell its own messages from a person's, and the id is not
    /// something an operator should have to type.
    bot: std::sync::RwLock<Option<BotHandle>>,
}

impl TelegramIntegration {
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            api: Api::new(config),
            bot: std::sync::RwLock::new(None),
        }
    }

    /// A connection over an already-built client, for tests.
    pub fn with_api(api: Api) -> Self {
        Self {
            api,
            bot: std::sync::RwLock::new(None),
        }
    }

    /// The bot's own handle, once `run` has read it.
    pub fn bot(&self) -> Option<BotHandle> {
        self.bot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Record the bot's identity. `pub` so a test can skip the network.
    pub fn set_bot(&self, bot: BotHandle) {
        *self
            .bot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(bot);
    }

    /// Turn one update into the pipeline's shape, or `None` to drop it.
    pub fn inbound(&self, update: &Update) -> Option<cowbot_integrations::InboundMessage> {
        let bot = self.bot()?;
        let message = update.message.as_ref()?;

        // The bot's own message. The shared pipeline drops it too, but doing it
        // here keeps the dedupe table free of the bot's own traffic.
        let Some(author) = message.author() else {
            // A channel post made as the channel, or a service message: nothing
            // there to attribute or answer.
            return None;
        };
        if author.is_bot && author.id.to_string() == bot.user_id {
            return None;
        }

        let text = message.text.clone().unwrap_or_default();
        let replied_to_bot = message
            .reply_to_message
            .as_ref()
            .and_then(|quoted| quoted.author())
            .is_some_and(|quoted| quoted.id.to_string() == bot.user_id);
        let addressed = addresses_bot(&text, &message.entities, replied_to_bot, &bot);
        // A bot's message is dropped by the shared pipeline as well; passing the
        // platform's own flag through means the reason a second bot was ignored
        // is recorded where every other platform records it.
        let is_bot = author.is_bot;

        Some(cowbot_integrations::InboundMessage {
            // The update id, not the message id: Telegram re-delivers a batch
            // whose offset was never confirmed, and two updates can carry the
            // same message id after an edit.
            event_id: update.update_id.to_string(),
            external_conversation_id: message.chat.id.to_string(),
            external_thread_id: message.topic(),
            author: ExternalAuthor {
                external_id: author.id.to_string(),
                display_name: author.display_name(),
                is_bot,
            },
            // Telegram markup is entity offsets into the raw text, which is
            // already what a CowBot channel stores; the mention of the bot is
            // turned into `@AgentName` by the shared pipeline.
            text,
            // A private chat is addressed by definition — that is CowBot's
            // direct-conversation mode. In a group, privacy mode means the bot
            // sees only what addresses it, and the entity check says so.
            addresses_bot: message.chat.is_private() || addressed,
            attachments: message.attachments(),
            created_at: moment(message.date),
        })
    }

    /// Read updates until cancelled.
    ///
    /// The offset is the confirmation: advancing it past an update is what tells
    /// Telegram the batch was handled. So a crash between storing a message and
    /// the next poll replays that batch, which is why the pipeline claims the
    /// update id before posting.
    async fn poll(&self, ctx: IntegrationContext, cancel: CancelSignal) -> CowResult<()> {
        let mut offset = 0i64;
        let mut delay = RETRY_INITIAL;

        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }

            let outcome = tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                outcome = self.api.get_updates(offset, POLL_TIMEOUT_SECS) => outcome,
            };

            match outcome {
                Ok(updates) => {
                    delay = RETRY_INITIAL;
                    for update in updates {
                        // Advance before handling: whether the message is
                        // stored, skipped or refused, this delivery has been
                        // dealt with, and the dedupe table is what protects a
                        // replay from becoming a second turn.
                        offset = offset.max(update.update_id + 1);
                        dispatch(self, &ctx, &update).await;
                    }
                }
                Err(error) if error.is_conflict() => {
                    // A webhook is registered, or another process polls the same
                    // token. Both are configuration, and neither fixes itself:
                    // a retry loop here would spin for ever and fill the log.
                    return Err(CowError::Config(format!(
                        "Telegram refused the update stream: {}. A webhook is registered for \
                         this bot, or another process is polling it; only one consumer may \
                         use getUpdates. Delete the webhook with deleteWebhook, or stop the \
                         other process",
                        error.description
                    )));
                }
                Err(error) if error.is_unauthorized() => {
                    // A revoked token never starts working.
                    return Err(CowError::Config(format!(
                        "Telegram rejected the bot token ({}); check the integration's \
                         configuration",
                        error.description
                    )));
                }
                Err(error) => {
                    // A rate limit says how long to wait; anything else backs off.
                    warn!(%error, "a Telegram poll failed");
                    delay = error
                        .retry_after
                        .map(std::time::Duration::from_secs)
                        .unwrap_or(delay);
                    tokio::select! {
                        () = tokio::time::sleep(delay) => {}
                        () = cancel.cancelled() => return Ok(()),
                    }
                    delay = (delay * 2).min(RETRY_MAX);
                }
            }
        }
    }

    /// Post one chunk, falling back to plain text when Telegram refuses our HTML.
    async fn post(
        &self,
        target: &OutboundTarget,
        content: &OutboundContent,
    ) -> Result<DeliveredId, ApiError> {
        let thread = target.thread_id.as_deref();
        let html = to_html(&cowbot_integrations::label_chunk(content));
        match self
            .api
            .send_message(&target.external_conversation_id, &html, thread)
            .await
        {
            Ok(id) => Ok(DeliveredId(id.0.to_string())),
            Err(error) if error.description.to_lowercase().contains("parse") => {
                // Our renderer produced something Telegram's parser refuses.
                // Plain text always posts, and an answer the user can read beats
                // one lost to a formatting quirk.
                debug!(%error, "Telegram refused the markup; posting plain text");
                let plain = cowbot_integrations::label_chunk(content);
                self.api
                    .send_plain_message(&target.external_conversation_id, &plain, thread)
                    .await
                    .map(|id| DeliveredId(id.0.to_string()))
            }
            Err(error) => Err(error),
        }
    }
}

/// Hand one update to the pipeline.
///
/// A refused message is logged and the poll continues: one message the pipeline
/// would not store must not stop the bot.
async fn dispatch(integration: &TelegramIntegration, ctx: &IntegrationContext, update: &Update) {
    // A button press is a decision, not a message (ADR-0060). It is answered in
    // every path, including a refusal: an unanswered callback leaves the person
    // looking at a spinner until the client gives up.
    if let Some(query) = &update.callback_query {
        dispatch_callback(integration, ctx, query).await;
        return;
    }
    let Some(message) = integration.inbound(update) else {
        return;
    };
    match ctx.sink.accept(message).await {
        Ok(outcome) => debug!(
            integration = %ctx.integration.id,
            update = update.update_id,
            ?outcome,
            "handled a Telegram update"
        ),
        Err(error) => warn!(
            integration = %ctx.integration.id,
            update = update.update_id,
            %error,
            "the pipeline refused a Telegram message"
        ),
    }
}

/// Turn one button press into an approval decision, and answer it.
///
/// Everything that can go wrong is reported back to the person who tapped,
/// because that is the only human in the loop: a refusal, an expired request, and
/// a card from somebody else's message all look the same from a chat window.
async fn dispatch_callback(
    integration: &TelegramIntegration,
    ctx: &IntegrationContext,
    query: &crate::update::CallbackQuery,
) {
    let Some(author) = query.from.as_ref() else {
        // Telegram always sends one; without it there is nobody to attribute a
        // decision to.
        return;
    };
    let chat_id = query
        .message
        .as_ref()
        .map(|message| message.chat.id.to_string())
        .unwrap_or_default();

    let outcome = match query.data.as_deref().and_then(parse_callback_data) {
        Some((approved, approval_id)) => {
            let external = ExternalAuthor {
                external_id: author.id.to_string(),
                display_name: author.display_name(),
                is_bot: author.is_bot,
            };
            Some(
                cowbot_integrations::approvals::decide_from_im(
                    ctx,
                    approval_id,
                    approved,
                    &external,
                )
                .await,
            )
        }
        // A button this adapter did not compose. Answering quietly is better
        // than reporting a failure the person cannot act on.
        None => None,
    };

    let (text, alert, settled) = match &outcome {
        Some(Ok(decided)) => (
            format!(
                "{}.",
                cowbot_integrations::approvals::decision_summary(&decided.approval)
            ),
            false,
            Some(cowbot_integrations::approvals::decision_summary(
                &decided.approval,
            )),
        ),
        Some(Err(Refused::NotPending(reason))) => {
            (format!("This is already settled: {reason}."), true, None)
        }
        Some(Err(Refused::NotAllowed(reason))) => (reason.clone(), true, None),
        None => (
            "This button is not one I know about.".to_string(),
            false,
            None,
        ),
    };

    // Revise the card, so a second person does not tap buttons whose decision
    // already landed. Only when the callback names the message it belongs to.
    if let (Some(summary), Some(message_id)) = (
        settled,
        query.message.as_ref().map(|message| message.message_id),
    ) {
        if let Err(error) = integration
            .api
            .edit_message(&chat_id, message_id, &to_html(&summary))
            .await
        {
            debug!(%error, "could not revise an approval card");
        }
    }

    if let Err(error) = integration
        .api
        .answer_callback(&query.id, &text, alert)
        .await
    {
        // The decision already landed; failing to answer only means the client
        // spinner times out. Worth a log line, not a retry.
        debug!(%error, "could not answer a Telegram callback");
    }
}

/// A Telegram timestamp, in seconds since the epoch.
fn moment(date: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(date, 0).single().unwrap_or_else(Utc::now)
}

/// Check that the bot can actually read and post in a chat.
///
/// `pub` so the startup and bind-time paths read the same logic the trait method
/// does: a test through a differently written path would prove less than it
/// looks.
pub async fn binding_health(
    api: &Api,
    bot: Option<&BotHandle>,
    external_id: &str,
) -> BindingHealth {
    let Some(bot) = bot else {
        // Without the bot's own id there is nothing to ask about, and a "fine"
        // here would be a claim the adapter cannot support.
        return BindingHealth::Unknown(
            "the bot's own identity was not discovered, so its access to the chat was not \
             checked"
                .into(),
        );
    };

    // `getChat` is the check that works for every chat kind, and its refusal is
    // specific: Telegram answers "chat not found" for a group the bot is not in
    // and for a person who has never messaged it.
    let chat = match api.get_chat(external_id).await {
        Ok(chat) => chat,
        Err(error) => {
            return match chat_unreachable_reason(&error) {
                Some(reason) => BindingHealth::Unreachable(reason),
                // A token problem or a server fault is not an answer about this
                // binding; saying "unreachable" would send an operator to fix
                // something that is fine.
                None => BindingHealth::Unknown(error.to_string()),
            };
        }
    };

    // A channel is the one chat kind where being present is not enough: without
    // admin rights a bot can read a channel and its posts are refused.
    if chat.kind == "channel" {
        match api.get_chat_member(external_id, &bot.user_id).await {
            Ok(member) if member.can_post() => {}
            Ok(_) => {
                return BindingHealth::Unreachable(format!(
                    "'{}' is a Telegram channel and the bot is not an administrator, so it \
                     cannot post there; promote it to administrator",
                    chat.display_name()
                ));
            }
            Err(error) => return BindingHealth::Unknown(error.to_string()),
        }
    }

    BindingHealth::Reachable
}

#[async_trait]
impl Integration for TelegramIntegration {
    fn kind(&self) -> IntegrationKind {
        IntegrationKind::Telegram
    }

    async fn run(&self, ctx: IntegrationContext, cancel: CancelSignal) -> CowResult<()> {
        // The identity first: without it the adapter cannot recognise its own
        // messages or its own mentions, and both failures are silent.
        let me = self.api.get_me().await.map_err(|error| {
            CowError::Config(format!(
                "Telegram refused the bot token ({}); check the integration's configuration",
                error.description
            ))
        })?;
        if me.is_bot {
            self.set_bot(BotHandle::new(
                me.id.to_string(),
                me.username.clone().unwrap_or_default(),
            ));
        } else {
            // A human account's token would make the adapter answer as that
            // person. Refusing is the only safe reading.
            return Err(CowError::Config(
                "the configured Telegram token belongs to a person, not a bot; talk to \
                 BotFather to create a bot account"
                    .into(),
            ));
        }

        info!(
            integration = %ctx.integration.id,
            bot = %me.display_name(),
            "the Telegram adapter is running"
        );

        // Check the bindings before the first poll: a binding the bot cannot
        // reach receives nothing and logs nothing, so each one gets a notice in
        // the CowBot channel it points at.
        report_binding_health(&ctx, self).await;

        self.poll(ctx, cancel).await
    }

    async fn deliver(
        &self,
        target: &OutboundTarget,
        content: &OutboundContent,
    ) -> CowResult<DeliveredId> {
        self.post(target, content).await.map_err(CowError::from)
    }

    fn chunk_limit(&self) -> cowbot_integrations::ChunkLimit {
        // `sendMessage` counts UTF-16 code units, so an emoji-heavy answer is
        // twice as long to Telegram as a character budget would think.
        cowbot_integrations::ChunkLimit::Units(MAX_MESSAGE_UNITS)
    }

    async fn check_binding(&self, external_id: &str) -> BindingHealth {
        binding_health(&self.api, self.bot().as_ref(), external_id).await
    }

    async fn fetch_attachment(
        &self,
        attachment: &Attachment,
        max_bytes: u64,
    ) -> cowbot_core::error::CowResult<Vec<u8>> {
        // Two calls: `getFile` names a path, and the path is what the download
        // address is built from. Telegram refuses `getFile` above 20 MB, so a
        // larger file fails here with Telegram's own reason.
        let reference = self
            .api
            .get_file(&attachment.external_id)
            .await
            .map_err(cowbot_core::error::CowError::from)?;
        let Some(path) = reference.file_path else {
            return Err(cowbot_core::error::CowError::Config(
                "Telegram did not say where the file is".into(),
            ));
        };
        self.api
            .download_file(&path, max_bytes)
            .await
            .map_err(cowbot_core::error::CowError::from)
    }

    /// Post an approval request with two inline-keyboard buttons.
    ///
    /// The card carries the approval id and nothing else: Telegram caps
    /// `callback_data` at 64 bytes, so the payload stays in CowBot and the button
    /// is a pointer to it.
    async fn post_approval(
        &self,
        target: &OutboundTarget,
        prompt: &ApprovalPrompt,
    ) -> cowbot_core::error::CowResult<ApprovalPosting> {
        let html = to_html(&prompt.describe());
        let sent = self
            .api
            .send_approval(
                &target.external_conversation_id,
                &html,
                &prompt.approval_id,
                target.thread_id.as_deref(),
            )
            .await
            .map_err(cowbot_core::error::CowError::from)?;
        Ok(ApprovalPosting {
            delivered: DeliveredId(sent.0.to_string()),
            buttons: true,
        })
    }

    async fn update_approval(
        &self,
        target: &OutboundTarget,
        posting: &ApprovalPosting,
        summary: &str,
    ) -> cowbot_core::error::CowResult<()> {
        let Ok(message_id) = posting.delivered.0.parse::<i64>() else {
            // A delivery id that is not a Telegram message id means the posting
            // came from somewhere else; there is nothing to edit.
            return Ok(());
        };
        self.api
            .edit_message(
                &target.external_conversation_id,
                message_id,
                &to_html(summary),
            )
            .await
            .map_err(cowbot_core::error::CowError::from)
    }
}

impl std::fmt::Debug for TelegramIntegration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TelegramIntegration")
            .field("api_base", &self.api.config().api_base)
            .field("bot", &self.bot().map(|bot| bot.username))
            .finish_non_exhaustive()
    }
}

impl Clone for TelegramIntegration {
    fn clone(&self) -> Self {
        Self {
            api: self.api.clone(),
            bot: std::sync::RwLock::new(self.bot()),
        }
    }
}

/// A binding row is keyed by the platform's conversation id, and Telegram's is a
/// signed integer. Anything else is a typo worth reporting at bind time.
pub fn chat_id_shape_is_valid(external_id: &str) -> bool {
    !external_id.trim().is_empty()
        && external_id
            .trim()
            .trim_start_matches('-')
            .chars()
            .all(|character| character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::Update;

    fn update(json: serde_json::Value) -> Update {
        serde_json::from_value(json).unwrap()
    }

    fn adapter() -> TelegramIntegration {
        let integration = TelegramIntegration::new(
            TelegramConfig::from_json(r#"{"bot_token":"42:secret"}"#).unwrap(),
        );
        integration.set_bot(BotHandle::new("42", "cowbot_help_bot"));
        integration
    }

    fn direct(text: &str) -> serde_json::Value {
        serde_json::json!({
            "update_id": 100,
            "message": {
                "message_id": 11,
                "date": 1_700_000_000i64,
                "chat": { "id": 7, "type": "private", "username": "ada" },
                "from": { "id": 7, "is_bot": false, "first_name": "Ada", "username": "ada" },
                "text": text,
                "entities": [],
            }
        })
    }

    #[test]
    fn a_private_chat_is_addressed_without_a_mention() {
        let message = adapter()
            .inbound(&update(direct("what is the status?")))
            .unwrap();
        assert!(message.addresses_bot);
        assert_eq!(message.external_conversation_id, "7");
        assert_eq!(message.author.display_name, "@ada");
        assert_eq!(message.event_id, "100");
        assert_eq!(message.text, "what is the status?");
    }

    #[test]
    fn a_group_message_is_addressed_only_when_it_mentions_the_bot() {
        let overheard = update(serde_json::json!({
            "update_id": 101,
            "message": {
                "message_id": 12,
                "date": 1_700_000_000i64,
                "chat": { "id": -1001234, "type": "supergroup", "title": "Deploys" },
                "from": { "id": 7, "is_bot": false, "first_name": "Ada", "username": "ada" },
                "text": "lunch?",
                "entities": [],
            }
        }));
        assert!(!adapter().inbound(&overheard).unwrap().addresses_bot);

        let addressed = update(serde_json::json!({
            "update_id": 102,
            "message": {
                "message_id": 13,
                "date": 1_700_000_000i64,
                "chat": { "id": -1001234, "type": "supergroup", "title": "Deploys" },
                "from": { "id": 7, "is_bot": false, "first_name": "Ada", "username": "ada" },
                "text": "@cowbot_help_bot status?",
                "entities": [{ "type": "mention", "offset": 0, "length": 16 }],
            }
        }));
        let message = adapter().inbound(&addressed).unwrap();
        assert!(message.addresses_bot);
        // The mention is left as the person typed it; the shared pipeline turns
        // it into `@AgentName` when the binding names an agent.
        assert_eq!(message.text, "@cowbot_help_bot status?");
    }

    #[test]
    fn the_bots_own_message_is_dropped() {
        let own = update(serde_json::json!({
            "update_id": 103,
            "message": {
                "message_id": 14,
                "date": 1_700_000_000i64,
                "chat": { "id": 7, "type": "private", "username": "ada" },
                "from": { "id": 42, "is_bot": true, "first_name": "CowBot", "username": "cowbot_help_bot" },
                "text": "Deploy is green.",
            }
        }));
        assert!(adapter().inbound(&own).is_none());
    }

    #[test]
    fn another_bots_message_is_passed_through_marked_as_a_bot() {
        // The shared pipeline drops it — that is where the rule lives — but the
        // adapter has to say so rather than silently hiding it, or the reason is
        // recorded nowhere.
        let other = update(serde_json::json!({
            "update_id": 104,
            "message": {
                "message_id": 15,
                "date": 1_700_000_000i64,
                "chat": { "id": -1001234, "type": "supergroup", "title": "Deploys" },
                "from": { "id": 99, "is_bot": true, "first_name": "Other", "username": "other_bot" },
                "text": "@cowbot_help_bot ping",
                "entities": [{ "type": "mention", "offset": 0, "length": 16 }],
            }
        }));
        let message = adapter().inbound(&other).unwrap();
        assert!(message.author.is_bot);
    }

    #[test]
    fn a_channel_post_with_no_author_is_dropped() {
        let anonymous = update(serde_json::json!({
            "update_id": 105,
            "message": {
                "message_id": 16,
                "date": 1_700_000_000i64,
                "chat": { "id": -1009876, "type": "channel", "title": "Announcements" },
                "text": "release notes",
            }
        }));
        assert!(adapter().inbound(&anonymous).is_none());
    }

    #[test]
    fn a_forum_topic_becomes_the_thread() {
        let topic = update(serde_json::json!({
            "update_id": 106,
            "message": {
                "message_id": 17,
                "date": 1_700_000_000i64,
                "chat": { "id": -1001234, "type": "supergroup", "title": "Deploys" },
                "from": { "id": 7, "is_bot": false, "first_name": "Ada", "username": "ada" },
                "text": "@cowbot_help_bot in topic",
                "entities": [{ "type": "mention", "offset": 0, "length": 16 }],
                "is_topic_message": true,
                "message_thread_id": 88,
            }
        }));
        assert_eq!(
            adapter()
                .inbound(&topic)
                .unwrap()
                .external_thread_id
                .as_deref(),
            Some("88")
        );
    }

    #[test]
    fn a_reply_to_the_bots_message_addresses_it() {
        let reply = update(serde_json::json!({
            "update_id": 107,
            "message": {
                "message_id": 18,
                "date": 1_700_000_000i64,
                "chat": { "id": -1001234, "type": "supergroup", "title": "Deploys" },
                "from": { "id": 7, "is_bot": false, "first_name": "Ada", "username": "ada" },
                "text": "and the rest?",
                "entities": [],
                "reply_to_message": {
                    "message_id": 17,
                    "date": 1_699_999_000i64,
                    "chat": { "id": -1001234, "type": "supergroup", "title": "Deploys" },
                    "from": { "id": 42, "is_bot": true, "first_name": "CowBot",
                              "username": "cowbot_help_bot" },
                    "text": "Deploy is green.",
                },
            }
        }));
        assert!(adapter().inbound(&reply).unwrap().addresses_bot);
    }

    #[test]
    fn an_update_before_the_identity_is_known_is_dropped() {
        // Without the bot's own id there is no safe way to tell its message
        // from a person's.
        let unknown = TelegramIntegration::new(
            TelegramConfig::from_json(r#"{"bot_token":"42:secret"}"#).unwrap(),
        );
        assert!(unknown.inbound(&update(direct("hi"))).is_none());
    }

    #[test]
    fn a_mention_after_an_emoji_is_read_at_the_right_offset() {
        let with_emoji = update(serde_json::json!({
            "update_id": 108,
            "message": {
                "message_id": 19,
                "date": 1_700_000_000i64,
                "chat": { "id": -1001234, "type": "supergroup", "title": "Deploys" },
                "from": { "id": 7, "is_bot": false, "first_name": "Ada", "username": "ada" },
                "text": "🚀 @cowbot_help_bot go",
                "entities": [{ "type": "mention", "offset": 3, "length": 16 }],
            }
        }));
        assert!(adapter().inbound(&with_emoji).unwrap().addresses_bot);
    }

    #[test]
    fn a_chat_id_is_a_signed_integer() {
        assert!(chat_id_shape_is_valid("7"));
        assert!(chat_id_shape_is_valid("-1001234"));
        assert!(!chat_id_shape_is_valid("@cowbot_help_bot"));
        assert!(!chat_id_shape_is_valid(""));
    }

    #[tokio::test]
    async fn a_binding_checked_before_the_identity_is_known_is_unknown_not_reachable() {
        let api = Api::new(TelegramConfig::from_json(r#"{"bot_token":"42:secret"}"#).unwrap());
        let health = binding_health(&api, None, "7").await;
        assert!(!health.is_reachable());
        assert!(health.problem().unwrap().contains("was not discovered"));
    }

    #[test]
    fn a_debug_print_never_carries_the_token() {
        let integration = TelegramIntegration::new(
            TelegramConfig::from_json(r#"{"bot_token":"42:super-secret"}"#).unwrap(),
        );
        let printed = format!("{integration:?}");
        assert!(!printed.contains("super-secret"), "{printed}");
    }
}
