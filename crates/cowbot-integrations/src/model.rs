//! The platform-neutral shapes an adapter reads and writes (ADR-0056).
//!
//! An adapter's whole job is translation: it turns a platform's event into an
//! [`InboundMessage`], and an [`OutboundContent`] into the platform's own
//! payload. Nothing here mentions a platform, so the pipeline that consumes it
//! is written once and tested against a fake adapter.

use chrono::{DateTime, Utc};

use cowbot_core::agent::AgentId;
use cowbot_core::channel::ChannelId;
use cowbot_core::integration::{IntegrationId, IntegrationKind};

/// A message a platform delivered, already verified and stripped of envelopes.
///
/// It is deliberately *not* a [`cowbot_core::channel::ChannelMessage`]: the
/// external ids, the platform's event id, and the author's platform identity are
/// all still needed to dedupe, to map the author, and to reply to the right
/// conversation.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// The platform's id for this delivery, used to drop a retry.
    ///
    /// Slack retries a delivery up to three times and Feishu retries on any
    /// non-2xx, so this is not optional in practice. An adapter that genuinely
    /// has no id must synthesise a stable one — a Telegram update is its update
    /// id, not a timestamp — or a retry becomes a second turn.
    pub event_id: String,
    /// The platform conversation the message came from.
    pub external_conversation_id: String,
    /// Thread or topic within that conversation, when the platform has one.
    pub external_thread_id: Option<String>,
    /// The platform account that wrote it.
    pub author: ExternalAuthor,
    /// The message text with platform markup already translated to CowBot's
    /// `@Name` form. Adapters do this, not the pipeline: only the adapter knows
    /// what a mention looks like on its platform.
    pub text: String,
    /// Whether this message addresses the bot: a direct message, or a mention of
    /// it in a shared conversation.
    ///
    /// An adapter reports what the platform told it. When the platform is silent
    /// on the point (Telegram's privacy mode, for instance, simply omits
    /// messages that do not address the bot), an adapter infers it and says so
    /// in its own documentation rather than leaving the pipeline to guess.
    pub addresses_bot: bool,
    pub created_at: DateTime<Utc>,
}

/// A platform account, as the platform names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAuthor {
    /// Stable id within the platform's tenant.
    pub external_id: String,
    /// Name to show if the account has not been seen before.
    pub display_name: String,
    /// Whether the account is a bot — this one or another.
    ///
    /// Another bot's message must not be answered: two bots mentioning each
    /// other is an infinite loop, and the platform will happily deliver it.
    pub is_bot: bool,
}

/// A reply the pipeline asks an adapter to post.
#[derive(Debug, Clone)]
pub struct OutboundContent {
    /// One chunk of an answer, already split to the platform's limit.
    ///
    /// Chunking is the pipeline's job because every platform has a different
    /// limit but the same requirement, and doing it in one place is what keeps a
    /// long answer ordered and split on paragraph boundaries.
    pub text: String,
    /// The thread to answer in, when the inbound message had one.
    pub thread_id: Option<String>,
    /// Which chunk of how many, so an adapter that can say so labels a long
    /// answer instead of posting several unrelated-looking messages.
    pub part: ChunkPart,
}

/// Where one outbound chunk sits in a longer answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPart {
    /// 0-based index of this chunk.
    pub index: usize,
    /// How many chunks the answer was split into.
    pub total: usize,
}

impl ChunkPart {
    /// Whether this chunk is part of a longer answer.
    pub fn is_multi(self) -> bool {
        self.total > 1
    }
}

/// The platform conversation a reply goes to.
#[derive(Debug, Clone)]
pub struct OutboundTarget {
    pub integration_id: IntegrationId,
    pub external_conversation_id: String,
    /// Thread to answer in, when the inbound message had one.
    pub thread_id: Option<String>,
}

impl OutboundTarget {
    pub fn new(integration: &IntegrationId, external_conversation_id: impl Into<String>) -> Self {
        Self {
            integration_id: integration.clone(),
            external_conversation_id: external_conversation_id.into(),
            thread_id: None,
        }
    }

    pub fn in_thread(mut self, thread_id: Option<String>) -> Self {
        self.thread_id = thread_id;
        self
    }
}

/// An id the platform assigned to a posted message.
///
/// Carried back so a later milestone can edit or replace a message — a Telegram
/// card that is updated rather than reposted — without the pipeline knowing the
/// platform's id shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveredId(pub String);

/// Everything the binding decides about one inbound conversation.
#[derive(Debug, Clone)]
pub struct Route {
    pub integration_id: IntegrationId,
    pub kind: IntegrationKind,
    /// The CowBot channel this platform conversation feeds.
    pub channel_id: ChannelId,
    /// The agent an unqualified message addresses, when the binding forces one.
    ///
    /// A direct conversation already answers every message and needs none. A
    /// group channel needs it to turn "the bot was mentioned" into
    /// "@AgentName", which is the only mention form `parse_mentions` understands.
    pub agent_id: Option<AgentId>,
}

/// Whether a bound conversation can actually reach the adapter.
///
/// A binding is a row that says "this external conversation feeds that channel".
/// It does not say the platform will deliver anything, and every platform has a
/// requirement the row cannot express: Mattermost delivers posts only for
/// channels the bot has joined. A binding that fails that requirement is the most
/// likely first support question — nothing is broken, nothing is logged, and the
/// channel is simply silent — so an adapter is asked to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingHealth {
    /// The adapter can deliver and receive on this conversation.
    Reachable,
    /// It cannot, and why. The reason is shown to an operator as written, so it
    /// should name the fix rather than the symptom.
    Unreachable(String),
    /// The adapter cannot tell — an unreachable server, or a platform with no
    /// way to ask.
    ///
    /// Distinct from [`BindingHealth::Reachable`] on purpose: reporting "fine"
    /// when the check itself failed is the failure mode this whole type exists
    /// to avoid.
    Unknown(String),
}

impl BindingHealth {
    pub fn is_reachable(&self) -> bool {
        matches!(self, BindingHealth::Reachable)
    }

    /// A short reason, when the binding is not confirmed reachable.
    pub fn problem(&self) -> Option<&str> {
        match self {
            BindingHealth::Reachable => None,
            BindingHealth::Unreachable(reason) | BindingHealth::Unknown(reason) => Some(reason),
        }
    }
}

/// Why the pipeline did nothing with a delivery.
///
/// Returned rather than logged so a caller — a test, or a supervisor deciding
/// whether to keep the connection — can tell an ordinary skip from a fault. A
/// skip is never an error: a retry, an echo, or a message for a channel nobody
/// bound is normal traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// The platform delivered this event before.
    Duplicate,
    /// The author is a bot, including this bot.
    FromBot,
    /// The platform conversation is not bound to a channel in this workspace.
    Unbound,
    /// The platform reported no text after markup was translated.
    Empty,
    /// The bound channel no longer exists, or moved to another workspace.
    ///
    /// Kept distinct from [`Skipped::Unbound`] because it means the binding
    /// should be removed, not that one should be added.
    ChannelGone,
}

impl Skipped {
    /// A short reason, for a log line or an operator-facing diagnostic.
    pub fn reason(&self) -> &'static str {
        match self {
            Skipped::Duplicate => "the platform delivered this event before",
            Skipped::FromBot => "the author is a bot",
            Skipped::Unbound => "the platform conversation is not bound to a channel",
            Skipped::Empty => "the message had no text",
            Skipped::ChannelGone => "the bound channel no longer exists",
        }
    }
}

/// What the pipeline did with one delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ingested {
    /// The message was stored and may start a turn.
    Posted {
        channel_id: ChannelId,
        message_id: String,
        /// The CowBot user the platform account spoke as.
        user_id: String,
        /// Whether a turn will answer it. False for a bound conversation whose
        /// message does not address an agent — it is shown but not answered,
        /// which is what a manager reading a channel expects.
        starts_turn: bool,
    },
    /// Nothing was stored, for the stated reason.
    Skipped(Skipped),
}

impl Ingested {
    pub fn is_posted(&self) -> bool {
        matches!(self, Ingested::Posted { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_chunk_is_not_labelled_as_part_of_an_answer() {
        assert!(!ChunkPart { index: 0, total: 1 }.is_multi());
        assert!(ChunkPart { index: 1, total: 2 }.is_multi());
    }

    #[test]
    fn unknown_is_not_reachable() {
        // The distinction the type exists for: a check that could not run must
        // not read as a green light.
        assert!(!BindingHealth::Unknown("the server did not answer".into()).is_reachable());
        assert!(!BindingHealth::Unreachable("not a member".into()).is_reachable());
        assert!(BindingHealth::Reachable.is_reachable());
        assert_eq!(BindingHealth::Reachable.problem(), None);
    }

    #[test]
    fn a_skip_names_its_reason() {
        // The wording is what an operator reads when a delivery is dropped, so
        // it has to say what happened rather than name an enum variant.
        assert!(Skipped::Duplicate.reason().contains("before"));
        assert!(Skipped::Unbound.reason().contains("not bound"));
    }
}
