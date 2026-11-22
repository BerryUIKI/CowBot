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
use cowbot_integrations::model::{DeliveredId, ExternalAuthor, OutboundContent, OutboundTarget};
use tracing::{debug, info};

use crate::config::MattermostConfig;
use crate::rest::{Rest, DEFAULT_MAX_POST_BYTES};
use crate::socket::Post;

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
}

/// Prefix a multi-part answer with its position.
///
/// A chunked reply arrives as several messages in a row; saying which is which
/// is the difference between a long answer and what reads as the bot talking to
/// itself. A single-chunk answer is left untouched, because "(1/1)" on every
/// reply would be noise.
fn label_chunk(content: &OutboundContent) -> String {
    if !content.part.is_multi() {
        return content.text.clone();
    }
    format!(
        "({}/{})\n{}",
        content.part.index + 1,
        content.part.total,
        content.text
    )
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
