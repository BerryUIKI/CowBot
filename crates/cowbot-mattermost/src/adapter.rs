//! The `Integration` implementation for Mattermost (ADR-0057).
//!
//! Translation only. The dedupe, the echo suppression, the identity mapping, and
//! the outbound mirror belong to the shared layer; what belongs here is what only
//! Mattermost knows — that a `posted` event nests its post as a JSON string, that
//! a mention is `@username`, and that a thread is a `root_id`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use cowbot_core::cancel::CancelSignal;
use cowbot_core::error::CowResult;
use cowbot_core::integration::IntegrationKind;
use cowbot_integrations::adapter::{Integration, IntegrationContext};
use cowbot_integrations::approvals::{ApprovalPosting, ApprovalPrompt};
use cowbot_integrations::model::{
    Attachment, AttachmentKind, BindingHealth, DeliveredId, ExternalAuthor, OutboundContent,
    OutboundTarget,
};
use tracing::{debug, info};

use crate::config::MattermostConfig;
use crate::rest::{Rest, DEFAULT_MAX_POST_BYTES};
use crate::socket::Post;
use cowbot_integrations::outbound::label_chunk;

/// What the bot learns about itself at startup.
///
/// Everything here is discovered rather than configured: a value the server can
/// answer is a value an operator can get wrong.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub user_id: String,
    pub username: String,
    /// The server's post size limit.
    pub max_post_bytes: usize,
}

/// One bot connection to one Mattermost server.
///
/// The state sits behind an `Arc` so the adapter is cheap to clone into the
/// socket callback. It is shared, not copied: the socket learns the bot's
/// identity once and every later message sees it.
#[derive(Clone)]
pub struct MattermostIntegration {
    config: MattermostConfig,
    rest: Rest,
    identity: Arc<RwLock<Option<Identity>>>,
    /// Display names resolved so far, keyed by Mattermost user id.
    ///
    /// A mention has to become a CowBot `@DisplayName`, and the only way to know
    /// a display name is to ask. Caching it keeps a busy channel from issuing a
    /// lookup per message.
    names: Arc<Mutex<HashMap<String, String>>>,
}

impl MattermostIntegration {
    pub fn new(config: MattermostConfig) -> Self {
        Self {
            rest: Rest::new(config.clone()),
            config,
            identity: Arc::new(RwLock::new(None)),
            names: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// A connection over an already-built REST client, for tests.
    pub fn with_rest(config: MattermostConfig, rest: Rest) -> Self {
        Self {
            rest,
            config,
            identity: Arc::new(RwLock::new(None)),
            names: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// What the bot knows about itself, once `run` has discovered it.
    pub fn identity(&self) -> Option<Identity> {
        self.identity
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_identity(&self, identity: Identity) {
        *self
            .identity
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(identity);
    }

    /// The post limit in force, whether or not `run` has read the server.
    pub fn max_post_bytes(&self) -> usize {
        self.identity()
            .map(|identity| identity.max_post_bytes)
            .filter(|limit| *limit > 0)
            .unwrap_or(DEFAULT_MAX_POST_BYTES)
    }

    /// Resolve a Mattermost user id to its username, asking once.
    ///
    /// A lookup failure is not worth failing a message over: the text is stored
    /// either way, and the id is a usable stand-in for a name.
    pub async fn username_of(&self, user_id: &str) -> String {
        if let Some(name) = self.cached_name(user_id) {
            return name;
        }
        match self.rest.user(user_id).await {
            Ok(user) => {
                self.cache_name(user_id, &user.username);
                user.username
            }
            Err(error) => {
                debug!(%error, user = %user_id, "could not resolve a Mattermost user");
                user_id.to_string()
            }
        }
    }

    fn cached_name(&self, user_id: &str) -> Option<String> {
        self.names
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(user_id)
            .cloned()
    }

    fn cache_name(&self, user_id: &str, username: &str) {
        self.names
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(user_id.to_string(), username.to_string());
    }

    /// Turn one socket post into the pipeline's shape, or `None` to drop it.
    pub async fn inbound(&self, post: &Post) -> Option<cowbot_integrations::InboundMessage> {
        let identity = self.identity()?;
        if post.is_system() {
            // "alice joined the channel" has no author worth answering.
            return None;
        }
        if post.user_id == identity.user_id {
            // The pipeline drops a bot's message too, but dropping it here keeps
            // the shared dedupe table free of the bot's own traffic.
            return None;
        }

        let author = self.username_of(&post.user_id).await;
        // The author's own name goes in `display_name`; a mention of *the bot* is
        // what becomes `@AgentName`. Translating a teammate's handle into an
        // agent name would address the wrong reader.
        let text = rewrite_mention(
            &post.message,
            &identity.username,
            &bot_display(&identity.username),
        );

        // The post names its files by id; the size and type come from a second
        // call, which is what lets the pipeline refuse an oversized file without
        // downloading it. A lookup that fails leaves the attachment described by
        // its id alone, so the message is still stored.
        let mut attachments = Vec::with_capacity(post.file_ids.len());
        for file_id in &post.file_ids {
            let info = self.rest.file_info(file_id).await.ok();
            let mime = info
                .as_ref()
                .map(|info| info.mime_type.clone())
                .filter(|mime| !mime.is_empty());
            attachments.push(Attachment {
                external_id: file_id.clone(),
                kind: AttachmentKind::from_mime(mime.as_deref()),
                file_name: info
                    .as_ref()
                    .map(|info| info.name.clone())
                    .filter(|name| !name.is_empty()),
                mime_type: mime,
                size_bytes: info.as_ref().map(|info| info.size),
            });
        }

        Some(cowbot_integrations::InboundMessage {
            event_id: post.id.clone(),
            external_conversation_id: post.channel_id.clone(),
            external_thread_id: post.thread_id(),
            author: ExternalAuthor {
                external_id: post.user_id.clone(),
                display_name: author,
                // Mattermost does not mark a post as coming from a bot; the
                // bot's own posts are already gone, and another bot in the
                // channel is unusual enough to treat as a person.
                is_bot: false,
            },
            text,
            // Mattermost delivers every post in a channel the bot belongs to.
            // Answering all of them is the busy-room problem, so addressing
            // means a mention of the bot.
            addresses_bot: contains_mention(&post.message, &identity.username),
            attachments,
            created_at: moment(post.create_at),
        })
    }
}

/// The name a bot's username stands for, with Mattermost's suffix removed.
fn bot_display(username: &str) -> String {
    username
        .strip_suffix(" [bot]")
        .unwrap_or(username)
        .to_string()
}

/// Whether `message` mentions `username` as a word.
pub fn contains_mention(message: &str, username: &str) -> bool {
    mention_spans(message, username).next().is_some()
}

/// Replace every `@username` in `message` with `@display`.
pub fn rewrite_mention(message: &str, username: &str, display: &str) -> String {
    let spans: Vec<(usize, usize)> = mention_spans(message, username).collect();
    if spans.is_empty() {
        return message.to_string();
    }
    let mut rebuilt = String::with_capacity(message.len());
    let mut cursor = 0;
    for (start, end) in spans {
        rebuilt.push_str(&message[cursor..start]);
        rebuilt.push('@');
        rebuilt.push_str(display);
        cursor = end;
    }
    rebuilt.push_str(&message[cursor..]);
    rebuilt
}

/// Every `@username` span in `message`, as byte ranges.
fn mention_spans<'a>(
    message: &'a str,
    username: &'a str,
) -> impl Iterator<Item = (usize, usize)> + 'a {
    let needle = format!("@{username}");
    let mut cursor = 0;
    let mut found = Vec::new();
    while let Some(offset) = message[cursor..].find(&needle) {
        let start = cursor + offset;
        let end = start + needle.len();
        // A mention starts at the beginning or after a non-word character, so
        // `user@example.com` is not one, and ends at a word boundary, so
        // `@botanist` does not match `bot`.
        let before_ok = message[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_alphanumeric() && character != '_');
        let after_ok = message[end..]
            .chars()
            .next()
            .is_none_or(|character| !character.is_alphanumeric() && character != '_');
        if before_ok && after_ok {
            found.push((start, end));
        }
        cursor = end;
    }
    found.into_iter()
}

/// A Mattermost timestamp, in milliseconds since the epoch.
fn moment(create_at: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(create_at)
        .single()
        .unwrap_or_else(Utc::now)
}

#[async_trait]
impl Integration for MattermostIntegration {
    fn kind(&self) -> IntegrationKind {
        IntegrationKind::Mattermost
    }

    async fn run(&self, ctx: IntegrationContext, cancel: CancelSignal) -> CowResult<()> {
        // Discover the bot's identity and the server's limit before reading a
        // message. A failure here is fatal for this integration: without a user
        // id the adapter cannot tell its own post from a person's, and without
        // the limit it would post messages the server refuses.
        let me = self.rest.me().await?;
        let max_post_bytes = self
            .rest
            .server_config()
            .await
            .map(|server| server.max_post_bytes())
            .unwrap_or(DEFAULT_MAX_POST_BYTES);
        self.set_identity(Identity {
            user_id: me.id,
            username: me.username.clone(),
            max_post_bytes,
        });
        info!(
            integration = %ctx.integration.id,
            username = %me.username,
            max_post_bytes,
            "the Mattermost adapter is running"
        );

        // Check the bindings while the identity is fresh and before any event
        // arrives. A binding whose channel the bot never joined receives
        // nothing and logs nothing — the single hardest fault in this adapter to
        // find — so each one that cannot reach the bot gets a notice in the
        // CowBot channel it points at, where the person who would go and add the
        // bot is already looking.
        cowbot_integrations::notice::report_binding_health(&ctx, self).await;

        let sink = ctx.sink;
        let adapter = self.clone();
        crate::socket::run(self.config.clone(), cancel, move |post| {
            let sink = sink.clone();
            let adapter = adapter.clone();
            async move {
                match adapter.inbound(&post).await {
                    Some(message) => sink.accept(message).await.map(|_| ()),
                    None => Ok(()),
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
        let post = self
            .rest
            .create_post(
                &target.external_conversation_id,
                &label_chunk(content),
                target.thread_id.as_deref(),
            )
            .await?;
        Ok(DeliveredId(post.id))
    }

    async fn check_binding(&self, external_id: &str) -> BindingHealth {
        binding_health(&self.rest, self.identity().as_ref(), external_id).await
    }

    async fn fetch_attachment(
        &self,
        attachment: &Attachment,
        max_bytes: u64,
    ) -> cowbot_core::error::CowResult<Vec<u8>> {
        self.rest
            .file_bytes(&attachment.external_id, max_bytes)
            .await
    }

    /// Post an approval request with its two answers.
    ///
    /// Mattermost has no native buttons outside an interactive integration, so
    /// the question is a post and the answer is a reply the inbound loop reads.
    /// That keeps the whole round trip on the socket the adapter already holds.
    async fn post_approval(
        &self,
        target: &OutboundTarget,
        prompt: &ApprovalPrompt,
    ) -> cowbot_core::error::CowResult<ApprovalPosting> {
        let post = self
            .rest
            .create_post(
                &target.external_conversation_id,
                &approval_body(prompt),
                target.thread_id.as_deref(),
            )
            .await?;
        Ok(ApprovalPosting {
            delivered: DeliveredId(post.id),
            buttons: true,
        })
    }

    async fn update_approval(
        &self,
        _target: &OutboundTarget,
        posting: &ApprovalPosting,
        summary: &str,
    ) -> cowbot_core::error::CowResult<()> {
        // The post is replaced rather than appended to: the buttons are prose,
        // so the only way to retire them is to say what happened in their place.
        self.rest.update_post(&posting.delivered.0, summary).await
    }
}

/// The body of an approval post.
///
/// The two answers are spelled out because there is no button to tap: a person
/// replies `approve` or `reject`, and the inbound loop reads that as a decision
/// rather than as a message for an agent.
pub fn approval_body(prompt: &ApprovalPrompt) -> String {
    format!(
        "{}

Reply `approve` or `reject` to decide this.",
        prompt.describe()
    )
}

/// What a reply to an approval post decided, if it is one.
///
/// Only a bare `approve`/`reject` counts. A sentence that happens to contain the
/// word is a message to the agent, and treating it as a decision would let
/// somebody approve a destructive write by saying "do not approve this".
pub fn decision_of(text: &str) -> Option<bool> {
    match text.trim().trim_matches('`').to_ascii_lowercase().as_str() {
        "approve" | "approved" | "yes" => Some(true),
        "reject" | "rejected" | "no" => Some(false),
        _ => None,
    }
}

/// Check whether the bot is a member of a bound channel.
///
/// `pub` so the startup and bind-time checks read the same logic the trait
/// method does: a test that went through a differently written path would prove
/// less than it looks.
pub async fn binding_health(
    rest: &Rest,
    identity: Option<&Identity>,
    external_id: &str,
) -> BindingHealth {
    let Some(identity) = identity else {
        // Without the bot's own id there is nothing to ask about, and a "fine"
        // here would be a claim the adapter cannot support.
        return BindingHealth::Unknown(
            "the bot's own identity was not discovered, so its channel membership was not checked"
                .into(),
        );
    };
    match rest.is_channel_member(external_id, &identity.user_id).await {
        Ok(true) => BindingHealth::Reachable,
        Ok(false) => BindingHealth::Unreachable(format!(
            "the bot is not a member of Mattermost channel '{external_id}'; add it to the channel              or the binding will receive nothing"
        )),
        Err(error) => BindingHealth::Unknown(error.to_string()),
    }
}

impl std::fmt::Debug for MattermostIntegration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MattermostIntegration")
            .field("base_url", &self.config.base_url)
            .field(
                "identity",
                &self.identity().map(|identity| identity.username),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_integrations::model::ChunkPart;

    fn post(message: &str, user_id: &str) -> Post {
        Post {
            id: "p1".into(),
            channel_id: "c1".into(),
            user_id: user_id.into(),
            message: message.into(),
            root_id: String::new(),
            kind: String::new(),
            create_at: 1_700_000_000_000,
            file_ids: Vec::new(),
        }
    }

    #[test]
    fn a_mention_is_recognised_at_the_start_and_mid_sentence() {
        assert!(contains_mention("@bot hello", "bot"));
        assert!(contains_mention("hello @bot", "bot"));
        assert!(contains_mention("hey, @bot please", "bot"));
    }

    #[test]
    fn an_address_and_a_longer_name_are_not_mentions() {
        // The two ways a naive `contains` gets this wrong.
        assert!(!contains_mention("write to user@bot.example.com", "bot"));
        assert!(!contains_mention("@botanist hello", "bot"));
        assert!(!contains_mention("@bot_2 hello", "bot"));
    }

    #[test]
    fn every_mention_of_the_bot_is_rewritten() {
        assert_eq!(
            rewrite_mention("@bot status of @bot", "bot", "CowBot"),
            "@CowBot status of @CowBot"
        );
    }

    #[test]
    fn another_persons_mention_is_left_alone() {
        // Only the bot's own handle is translated: rewriting a teammate's would
        // address the wrong reader.
        assert_eq!(
            rewrite_mention("@bot ask @alice", "bot", "CowBot"),
            "@CowBot ask @alice"
        );
    }

    #[test]
    fn the_bot_suffix_is_removed_for_display() {
        assert_eq!(bot_display("cowbot [bot]"), "cowbot");
        assert_eq!(bot_display("cowbot"), "cowbot");
    }

    #[tokio::test]
    async fn a_system_post_is_dropped() {
        let adapter = adapter_with_identity();
        let mut system = post("alice joined", "u1");
        system.kind = "system_join_channel".into();
        assert!(adapter.inbound(&system).await.is_none());
    }

    #[tokio::test]
    async fn the_bots_own_post_is_dropped_without_a_lookup() {
        let adapter = adapter_with_identity();
        assert!(adapter.inbound(&post("hi", "bot-id")).await.is_none());
    }

    #[tokio::test]
    async fn a_mention_addresses_the_bot_and_a_plain_message_does_not() {
        let adapter = adapter_with_identity();
        let addressed = adapter
            .inbound(&post("@cowbot [bot] status?", "u1"))
            .await
            .unwrap();
        assert!(addressed.addresses_bot);
        assert_eq!(addressed.text, "@cowbot status?");

        let overheard = adapter.inbound(&post("lunch?", "u1")).await.unwrap();
        assert!(!overheard.addresses_bot);
    }

    #[tokio::test]
    async fn a_reply_keeps_its_thread() {
        let adapter = adapter_with_identity();
        let mut reply = post("@cowbot [bot] follow up", "u1");
        reply.root_id = "p0".into();
        let message = adapter.inbound(&reply).await.unwrap();
        assert_eq!(message.external_thread_id.as_deref(), Some("p0"));
    }

    #[tokio::test]
    async fn the_event_id_is_the_post_id() {
        let adapter = adapter_with_identity();
        let message = adapter.inbound(&post("hello", "u1")).await.unwrap();
        assert_eq!(message.event_id, "p1");
    }

    #[tokio::test]
    async fn an_adapter_that_never_ran_has_no_identity_and_drops_everything() {
        // The socket learns the identity in `run`; before that there is no safe
        // way to tell the bot's own post from a person's.
        let adapter = MattermostIntegration::new(MattermostConfig {
            base_url: "https://chat.example.com".into(),
            bot_token: "abc".into(),
        });
        assert!(adapter.inbound(&post("hello", "u1")).await.is_none());
        assert_eq!(adapter.max_post_bytes(), DEFAULT_MAX_POST_BYTES);
    }

    #[test]
    fn a_single_chunk_is_not_labelled() {
        let content = OutboundContent {
            text: "Hello".into(),
            thread_id: None,
            part: ChunkPart { index: 0, total: 1 },
        };
        assert_eq!(label_chunk(&content), "Hello");
    }

    #[test]
    fn a_long_answer_says_which_part_it_is() {
        let content = OutboundContent {
            text: "second half".into(),
            thread_id: None,
            part: ChunkPart { index: 1, total: 3 },
        };
        assert_eq!(label_chunk(&content), "(2/3)\nsecond half");
    }

    fn adapter_with_identity() -> MattermostIntegration {
        let adapter = MattermostIntegration::new(MattermostConfig {
            base_url: "https://chat.example.com".into(),
            bot_token: "abc".into(),
        });
        adapter.set_identity(Identity {
            user_id: "bot-id".into(),
            username: "cowbot [bot]".into(),
            max_post_bytes: DEFAULT_MAX_POST_BYTES,
        });
        // The author lookup would otherwise reach the network; pre-seeding the
        // cache keeps these tests about translation.
        adapter.cache_name("u1", "alice");
        adapter
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;
    use cowbot_core::agent::AgentProfile;
    use cowbot_core::channel::{Channel, ChannelId, Sender};
    use cowbot_core::identity::Workspace;
    use cowbot_core::integration::{IntegrationBinding, IntegrationKind, NewIntegration};
    use cowbot_integrations::adapter::MessageSink;
    use cowbot_integrations::model::{InboundMessage, Ingested, Skipped};
    use cowbot_store::Store;

    /// A sink that accepts nothing; the notice path does not use it.
    struct NoSink;

    #[async_trait::async_trait]
    impl MessageSink for NoSink {
        async fn accept(&self, _message: InboundMessage) -> CowResult<Ingested> {
            Ok(Ingested::Skipped(Skipped::Duplicate))
        }
    }

    /// A workspace, a channel, and an integration bound to one external id.
    async fn fixture(external_id: &str) -> (Store, IntegrationContext, ChannelId) {
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let workspace = Workspace::new("Team");
        store.create_workspace(&workspace).await.unwrap();
        let channel = Channel::new("support", "");
        store.create_channel(&workspace.id, &channel).await.unwrap();
        // The pipeline attributes a system notice to nobody, but the binding
        // needs an agent row only when it names one; none is needed here.
        let _ = AgentProfile::new("CowBot", "assistant", "goal", "backstory");

        let new = NewIntegration::new(&workspace.id, IntegrationKind::Mattermost, "Bot", "{}");
        store.create_integration(&new).await.unwrap();
        store
            .bind_integration_channel(&IntegrationBinding {
                integration_id: new.id.clone(),
                external_id: external_id.into(),
                channel_id: channel.id.clone(),
                agent_id: None,
            })
            .await
            .unwrap();

        let integration = store.get_integration(&new.id).await.unwrap().unwrap();
        let ctx = IntegrationContext {
            store: store.clone(),
            bus: cowbot_bus::EventBus::default_bus(),
            integration,
            sink: std::sync::Arc::new(NoSink),
            attachments_dir: None,
        };
        (store, ctx, channel.id)
    }

    #[tokio::test]
    async fn an_unreachable_binding_posts_a_notice_into_its_cowbot_channel() {
        // The whole point: the operator is looking at the CowBot channel, not at
        // a log file. A silent binding reads as a broken product.
        let (store, ctx, channel_id) = fixture("mm-channel-1").await;
        // Port 1 refuses connections, so the membership check cannot answer and
        // the binding is `Unknown` — which is still not a green light.
        let adapter = MattermostIntegration::new(MattermostConfig {
            base_url: "http://127.0.0.1:1".into(),
            bot_token: "abc".into(),
        });
        adapter.set_identity(Identity {
            user_id: "bot-id".into(),
            username: "cowbot [bot]".into(),
            max_post_bytes: 1_000,
        });

        cowbot_integrations::notice::report_binding_health(&ctx, &adapter).await;

        let messages = store.list_messages(&channel_id, 10).await.unwrap();
        assert_eq!(messages.len(), 1, "no notice was posted");
        assert_eq!(messages[0].sender, Sender::System);
        assert!(
            messages[0].content.contains("cannot receive messages"),
            "{}",
            messages[0].content
        );
    }

    #[tokio::test]
    async fn a_binding_that_was_never_checked_still_says_so() {
        // Without an identity there is nothing to ask, and saying nothing would
        // let an operator believe the binding works.
        let (store, ctx, channel_id) = fixture("mm-channel-1").await;
        let adapter = MattermostIntegration::new(MattermostConfig {
            base_url: "http://127.0.0.1:1".into(),
            bot_token: "abc".into(),
        });

        cowbot_integrations::notice::report_binding_health(&ctx, &adapter).await;

        let messages = store.list_messages(&channel_id, 10).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].content.contains("was not discovered"),
            "{}",
            messages[0].content
        );
    }
}
