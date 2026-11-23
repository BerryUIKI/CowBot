//! The `Integration` implementation for Feishu / Lark (ADR-0061).
//!
//! Translation only. What belongs here is what only Feishu knows: that the
//! dedupe key is the message id rather than the envelope's event id, that the
//! text is a JSON string inside the content field, that a mention is a
//! placeholder with the name in a sibling array, and that `p2p` is a direct
//! conversation while `group` needs a mention.

use async_trait::async_trait;
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

use crate::api::{Api, BotInfo};
use crate::config::FeishuConfig;
use crate::event::{mentions_bot, CardAction, Envelope, MessageEvent};
use crate::frame::Frame;

/// One app connection to Feishu.
pub struct FeishuIntegration {
    api: Api,
    /// The bot's own identity, read once at startup.
    bot: std::sync::RwLock<Option<BotInfo>>,
}

impl FeishuIntegration {
    pub fn new(config: FeishuConfig) -> Self {
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

    pub fn bot(&self) -> Option<BotInfo> {
        self.bot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Record the bot's identity. `pub` so a test can skip the network.
    pub fn set_bot(&self, bot: BotInfo) {
        *self
            .bot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(bot);
    }

    /// Turn one `im.message.receive_v1` envelope into the pipeline's shape.
    ///
    /// Returns `None` for anything the pipeline would drop anyway, so the reason
    /// is recorded once rather than twice.
    pub fn inbound(
        &self,
        envelope: &Envelope,
        event: &MessageEvent,
        bot_open_id: &str,
    ) -> Option<cowbot_integrations::InboundMessage> {
        if !envelope.is_v2() {
            return None;
        }
        let message = &event.message;
        // A bot's own message, or another bot's. The shared pipeline drops it too,
        // but doing it here keeps the dedupe table free of bots' traffic.
        let author_id = message.sender_open_id(&event.sender);
        if author_id.is_empty() || author_id == bot_open_id {
            return None;
        }
        if !event.sender.is_person() {
            return None;
        }

        let direct = message.is_direct();
        let addressed = direct || mentions_bot(&message.mentions, bot_open_id);
        let text = message.text().unwrap_or_default();
        let attachments: Vec<Attachment> = message.attachment().into_iter().collect();
        // A message with neither text nor a file is one nothing can be done with.
        // Kept rather than dropped when a file is present: a photo with no caption
        // is still the thing the person sent.
        if text.trim().is_empty() && attachments.is_empty() {
            return None;
        }

        Some(cowbot_integrations::InboundMessage {
            // The message id, not `header.event_id`: Feishu's own documentation
            // says repeated pushes can occur and that idempotency must use the
            // message id. Using the event id produces two turns for one question.
            event_id: message.message_id.clone(),
            external_conversation_id: message.chat_id.clone(),
            external_thread_id: message.topic(),
            author: ExternalAuthor {
                external_id: author_id,
                display_name: String::new(),
                is_bot: false,
            },
            text,
            addresses_bot: addressed,
            // The resource path needs the message id as well as the key, so the
            // two are joined here and split again in `fetch_attachment`.
            attachments: attachments
                .into_iter()
                .map(|mut attachment| {
                    attachment.external_id =
                        format!("{}:{}", message.message_id, attachment.external_id);
                    attachment
                })
                .collect(),
            created_at: message.moment(),
        })
    }
}

impl crate::event::Message {
    /// The author's stable id.
    ///
    /// `open_id` is the only one always present; `user_id` needs an extra scope
    /// and `union_id` is developer-scoped.
    fn sender_open_id(&self, sender: &crate::event::Sender) -> String {
        let _ = self;
        sender.sender_id.open_id.clone()
    }
}

/// Check that the bot is in a chat.
///
/// `pub` so the startup and bind-time paths read the same logic the trait method
/// does.
pub async fn binding_health(api: &Api, bot: Option<&BotInfo>, external_id: &str) -> BindingHealth {
    if bot.is_none() {
        return BindingHealth::Unknown(
            "the bot's own identity was not discovered, so its access to the chat was not checked"
                .into(),
        );
    }
    match api.chat(external_id).await {
        Ok(_) => {}
        Err(error) => {
            return match error.is_not_in_chat() {
                true => BindingHealth::Unreachable(format!(
                    "the bot cannot see Feishu chat '{external_id}'; add it to the chat, or the \
                     binding will receive nothing"
                )),
                // A credential problem or a server fault is not an answer about
                // this binding.
                false => BindingHealth::Unknown(error.to_string()),
            };
        }
    }
    match api.is_in_chat(external_id).await {
        Ok(true) => BindingHealth::Reachable,
        Ok(false) => BindingHealth::Unreachable(format!(
            "the bot is not a member of Feishu chat '{external_id}'; add it to the chat, or the \
             binding will receive nothing"
        )),
        Err(error) => match error.is_not_in_chat() {
            true => BindingHealth::Unreachable(format!(
                "the bot is not a member of Feishu chat '{external_id}'; add it to the chat, or \
                 the binding will receive nothing"
            )),
            false => BindingHealth::Unknown(error.to_string()),
        },
    }
}

#[async_trait]
impl Integration for FeishuIntegration {
    fn kind(&self) -> IntegrationKind {
        IntegrationKind::Feishu
    }

    async fn run(&self, ctx: IntegrationContext, cancel: CancelSignal) -> CowResult<()> {
        // The identity first: without its open_id the adapter cannot tell a
        // mention of itself from a mention of another bot, which is the whole
        // addressing rule.
        let bot = self.api.bot_info().await.map_err(|error| {
            CowError::Config(format!(
                "Feishu refused the app credentials ({}); check the integration's configuration",
                error.message
            ))
        })?;
        if bot.open_id.is_empty() {
            return Err(CowError::Config(
                "Feishu answered the bot information without an open_id, so this adapter cannot \
                 tell its own mentions apart"
                    .into(),
            ));
        }
        let bot_open_id = bot.open_id.clone();
        self.set_bot(bot.clone());
        info!(
            integration = %ctx.integration.id,
            bot = %bot.app_name,
            "the Feishu adapter is running"
        );

        report_binding_health(&ctx, self).await;

        let adapter = self.clone();
        let ctx = ctx.clone();
        crate::socket::run(self.api.clone(), cancel, move |frame| {
            let adapter = adapter.clone();
            let ctx = ctx.clone();
            let bot_open_id = bot_open_id.clone();
            async move { adapter.handle_frame(&ctx, &frame, &bot_open_id).await }
        })
        .await
    }

    async fn deliver(
        &self,
        target: &OutboundTarget,
        content: &OutboundContent,
    ) -> CowResult<DeliveredId> {
        // `text` does not render Markdown on Feishu; a `post` with an `md` tag
        // does. Posting the answer as `text` shows the reader literal `**`.
        let body = serde_json::json!({
            "zh_cn": {
                "content": [[{ "tag": "md", "text": cowbot_integrations::label_chunk(content) }]]
            }
        });
        let sent = self
            .api
            .send_message(&target.external_conversation_id, "post", &body.to_string())
            .await
            .map_err(CowError::from)?;
        Ok(DeliveredId(sent.message_id))
    }

    fn chunk_limit(&self) -> cowbot_integrations::ChunkLimit {
        // A post body is capped at 30 KB of escaped JSON and a Chinese character
        // costs three bytes, so the shared 4 000-character budget is already well
        // inside the limit. Keeping it means one answer reads the same everywhere.
        cowbot_integrations::ChunkLimit::Characters(cowbot_integrations::outbound::MAX_CHUNK_CHARS)
    }

    async fn check_binding(&self, external_id: &str) -> BindingHealth {
        binding_health(&self.api, self.bot().as_ref(), external_id).await
    }

    async fn fetch_attachment(
        &self,
        attachment: &Attachment,
        max_bytes: u64,
    ) -> CowResult<Vec<u8>> {
        // `message_id:file_key`, joined at inbound and split here: the resource
        // path needs both.
        let Some((message_id, file_key)) = attachment.external_id.split_once(':') else {
            return Err(CowError::Config(
                "the attachment is not addressable: its id does not name a message and a file"
                    .into(),
            ));
        };
        let kind = match attachment.kind {
            cowbot_integrations::model::AttachmentKind::Image => "image",
            _ => "file",
        };
        self.api
            .file_bytes(message_id, file_key, kind, max_bytes)
            .await
            .map_err(CowError::from)
    }

    /// Post an approval request as an interactive card with two buttons.
    async fn post_approval(
        &self,
        target: &OutboundTarget,
        prompt: &ApprovalPrompt,
    ) -> CowResult<ApprovalPosting> {
        let card = crate::card::approval(&prompt.describe(), &prompt.approval_id);
        let sent = self
            .api
            .send_message(
                &target.external_conversation_id,
                "interactive",
                &card.to_string(),
            )
            .await
            .map_err(CowError::from)?;
        Ok(ApprovalPosting {
            delivered: DeliveredId(sent.message_id),
            buttons: true,
        })
    }

    async fn update_approval(
        &self,
        _target: &OutboundTarget,
        posting: &ApprovalPosting,
        summary: &str,
    ) -> CowResult<()> {
        let card = crate::card::decided(summary);
        self.api
            .update_card(&posting.delivered.0, &card)
            .await
            .map_err(CowError::from)
    }
}

impl FeishuIntegration {
    /// Handle one frame: an event, a card callback, or something to ignore.
    ///
    /// The return value is what the socket answers the frame with. Feishu closes
    /// a connection whose handlers are slow, so nothing here waits for anything.
    async fn handle_frame(
        &self,
        ctx: &IntegrationContext,
        frame: &Frame,
        bot_open_id: &str,
    ) -> CowResult<Option<String>> {
        let Some(text) = frame.payload_text() else {
            // An encoding this adapter does not read: answer so Feishu does not
            // retry it for ever.
            return Ok(Some(String::new()));
        };
        let Ok(envelope) = serde_json::from_str::<Envelope>(text) else {
            debug!("ignoring a Feishu frame that is not an envelope");
            return Ok(Some(String::new()));
        };
        if !envelope.is_v2() || !envelope.is_for(&self.api.config().app_id) {
            return Ok(Some(String::new()));
        }

        match envelope.event_type() {
            "im.message.receive_v1" => {
                let Some(event) = envelope.event.clone() else {
                    return Ok(Some(String::new()));
                };
                let Ok(event) = serde_json::from_value::<MessageEvent>(event) else {
                    debug!("ignoring a Feishu message event this adapter cannot read");
                    return Ok(Some(String::new()));
                };
                match self.inbound(&envelope, &event, bot_open_id) {
                    Some(message) => {
                        if let Err(error) = ctx.sink.accept(message).await {
                            warn!(%error, "the pipeline refused a Feishu message");
                        }
                    }
                    None => debug!("dropped a Feishu message the pipeline would not store"),
                }
                Ok(Some(String::new()))
            }
            "card.action.trigger" => {
                let Some(event) = envelope.event.clone() else {
                    return Ok(Some(String::new()));
                };
                let Ok(action) = serde_json::from_value::<CardAction>(event) else {
                    return Ok(Some(String::new()));
                };
                let Some((approved, approval_id)) = action.action.decision() else {
                    return Ok(Some(String::new()));
                };
                let author = ExternalAuthor {
                    external_id: action.operator.open_id.clone(),
                    display_name: String::new(),
                    is_bot: false,
                };
                let toast = match cowbot_integrations::approvals::decide_from_im(
                    ctx,
                    &approval_id,
                    approved,
                    &author,
                )
                .await
                {
                    Ok(decided) => {
                        cowbot_integrations::approvals::decision_summary(&decided.approval)
                    }
                    Err(Refused::NotPending(reason)) => format!("Already settled: {reason}"),
                    Err(Refused::NotAllowed(reason)) => reason,
                };
                // The callback answer is a toast the client shows; the card
                // itself is revised through the mirror on `ApprovalDecided`, which
                // is the path every platform shares.
                let body = serde_json::json!({
                    "toast": { "type": "info", "content": toast }
                });
                Ok(Some(body.to_string()))
            }
            _ => Ok(Some(String::new())),
        }
    }
}

impl std::fmt::Debug for FeishuIntegration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeishuIntegration")
            .field("domain", &self.api.config().domain)
            .field("app_id", &self.api.config().app_id)
            .field("bot", &self.bot().map(|bot| bot.app_name))
            .finish_non_exhaustive()
    }
}

impl Clone for FeishuIntegration {
    fn clone(&self) -> Self {
        Self {
            api: self.api.clone(),
            bot: std::sync::RwLock::new(self.bot()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Message, Sender, SenderId};

    fn adapter() -> FeishuIntegration {
        let integration = FeishuIntegration::new(
            FeishuConfig::from_json(r#"{"app_id":"cli_1","app_secret":"s"}"#).unwrap(),
        );
        integration.set_bot(BotInfo {
            open_id: "ou_bot".into(),
            app_name: "CowBot".into(),
        });
        integration
    }

    fn envelope(event: serde_json::Value) -> Envelope {
        Envelope {
            schema: "2.0".into(),
            header: crate::event::Header {
                event_id: "evt-1".into(),
                event_type: "im.message.receive_v1".into(),
                create_time: "1700000000000".into(),
                token: String::new(),
                app_id: "cli_1".into(),
                tenant_key: String::new(),
            },
            event: Some(event),
        }
    }

    fn message_event(chat_type: &str, content: &str, mentions: serde_json::Value) -> MessageEvent {
        serde_json::from_value(serde_json::json!({
            "sender": {
                "sender_type": "user",
                "sender_id": { "open_id": "ou_ada" }
            },
            "message": {
                "message_id": "om_1",
                "chat_id": "oc_1",
                "chat_type": chat_type,
                "message_type": "text",
                "content": content,
                "create_time": "1700000000000",
                "mentions": mentions,
            }
        }))
        .unwrap()
    }

    #[test]
    fn the_dedupe_key_is_the_message_id_not_the_event_id() {
        // Feishu's own documentation says repeated pushes follow and that
        // idempotency must use the message id; the event id would let a repeat
        // through as a second turn.
        let event = message_event("p2p", r#"{"text":"hi"}"#, serde_json::json!([]));
        let message = adapter().inbound(&envelope(serde_json::json!({})), &event, "ou_bot");
        assert_eq!(message.unwrap().event_id, "om_1");
    }

    #[test]
    fn a_p2p_message_is_addressed_without_a_mention_and_a_group_one_is_not() {
        let direct = message_event("p2p", r#"{"text":"hi"}"#, serde_json::json!([]));
        let message = adapter()
            .inbound(&envelope(serde_json::json!({})), &direct, "ou_bot")
            .unwrap();
        assert!(message.addresses_bot);

        let group = message_event("group", r#"{"text":"lunch?"}"#, serde_json::json!([]));
        let message = adapter()
            .inbound(&envelope(serde_json::json!({})), &group, "ou_bot")
            .unwrap();
        assert!(!message.addresses_bot);
    }

    #[test]
    fn a_mention_of_the_bot_by_open_id_addresses_it() {
        let event = message_event(
            "group",
            r#"{"text":"@_user_1 status?"}"#,
            serde_json::json!([{ "key": "@_user_1", "name": "CowBot",
                                 "id": { "open_id": "ou_bot" }, "mentioned_type": "bot" }]),
        );
        let message = adapter()
            .inbound(&envelope(serde_json::json!({})), &event, "ou_bot")
            .unwrap();
        assert!(message.addresses_bot);
        assert_eq!(message.text, "@CowBot status?");
    }

    #[test]
    fn a_mention_of_another_bot_does_not_address_this_one() {
        let event = message_event(
            "group",
            r#"{"text":"@_user_1 hi"}"#,
            serde_json::json!([{ "key": "@_user_1", "name": "Other",
                                 "id": { "open_id": "ou_other" }, "mentioned_type": "bot" }]),
        );
        let message = adapter()
            .inbound(&envelope(serde_json::json!({})), &event, "ou_bot")
            .unwrap();
        assert!(!message.addresses_bot);
    }

    #[test]
    fn the_bots_own_message_and_another_bots_are_dropped() {
        let mut own = message_event("p2p", r#"{"text":"hi"}"#, serde_json::json!([]));
        own.sender.sender_id.open_id = "ou_bot".into();
        assert!(adapter()
            .inbound(&envelope(serde_json::json!({})), &own, "ou_bot")
            .is_none());

        let mut other = message_event("group", r#"{"text":"hi"}"#, serde_json::json!([]));
        other.sender.sender_type = "app".into();
        assert!(adapter()
            .inbound(&envelope(serde_json::json!({})), &other, "ou_bot")
            .is_none());
    }

    #[test]
    fn an_image_with_no_caption_is_still_stored() {
        // A photo is the thing the person sent; dropping it because the text is
        // empty would lose the message entirely.
        let event: MessageEvent = serde_json::from_value(serde_json::json!({
            "sender": { "sender_type": "user", "sender_id": { "open_id": "ou_ada" } },
            "message": {
                "message_id": "om_2", "chat_id": "oc_1", "chat_type": "p2p",
                "message_type": "image", "content": "{\"image_key\":\"img_1\"}",
            }
        }))
        .unwrap();
        let message = adapter()
            .inbound(&envelope(serde_json::json!({})), &event, "ou_bot")
            .unwrap();
        assert_eq!(message.attachments.len(), 1);
        // The resource path needs the message id, so it is joined with the key.
        assert_eq!(message.attachments[0].external_id, "om_2:img_1");
    }

    #[test]
    fn a_v1_envelope_is_refused() {
        let mut v1 = envelope(serde_json::json!({}));
        v1.schema = String::new();
        let event = message_event("p2p", r#"{"text":"hi"}"#, serde_json::json!([]));
        assert!(adapter().inbound(&v1, &event, "ou_bot").is_none());
    }

    #[test]
    fn a_topic_message_keeps_its_thread() {
        let mut event = message_event("group", r#"{"text":"hi"}"#, serde_json::json!([]));
        event.message.thread_id = Some("omt_1".into());
        let message = adapter()
            .inbound(&envelope(serde_json::json!({})), &event, "ou_bot")
            .unwrap();
        assert_eq!(message.external_thread_id.as_deref(), Some("omt_1"));
    }

    #[test]
    fn a_debug_print_never_carries_the_secret() {
        let integration = FeishuIntegration::new(
            FeishuConfig::from_json(r#"{"app_id":"cli_1","app_secret":"super-secret"}"#).unwrap(),
        );
        let printed = format!("{integration:?}");
        assert!(!printed.contains("super-secret"), "{printed}");
    }

    #[test]
    fn an_unused_sender_struct_still_compiles_a_message_id_lookup() {
        // `sender_open_id` reads the sender, not the message; this pins the shape
        // so a refactor cannot silently read the wrong one.
        let sender = Sender {
            sender_id: SenderId {
                open_id: "ou_ada".into(),
                ..SenderId::default()
            },
            sender_type: "user".into(),
        };
        let message: Message = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(message.sender_open_id(&sender), "ou_ada");
    }
}
