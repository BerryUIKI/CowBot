use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::AgentId;
use crate::channel::ChannelId;
use crate::task::TaskId;
use crate::usage::estimate_tokens;

/// Fewest participants that make a conversation a group.
pub const MIN_GROUP_PARTICIPANTS: usize = 2;

/// Most participants a group conversation may have.
pub const MAX_GROUP_PARTICIPANTS: usize = 8;

/// Most participants a router may select for one turn.
pub const MAX_ROUTED_AGENTS: usize = 3;

/// What a channel in the workspace represents.
///
/// `Direct` is the agent-owned contact conversation provisioned automatically
/// for every agent (ADR-0024). `Conversation` is a user-created session with an
/// independent append-only context branch (ADR-0030).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// A group channel whose messages address agents with `@mentions`.
    #[default]
    Channel,
    /// An agent's direct conversation, created and bounded by the runtime.
    Direct,
    /// A user-created conversation with its own context branch.
    Conversation,
}

impl ChannelKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Direct => "direct",
            Self::Conversation => "conversation",
        }
    }
}

impl std::str::FromStr for ChannelKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "channel" => Ok(Self::Channel),
            "direct" => Ok(Self::Direct),
            "conversation" => Ok(Self::Conversation),
            _ => Err(()),
        }
    }
}

/// Who takes part in a conversation channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelParticipant {
    pub channel_id: ChannelId,
    pub agent_id: AgentId,
    /// Position in the group's ordered participant roster.
    #[serde(default)]
    pub position: u32,
    pub joined_at: DateTime<Utc>,
}

impl ChannelParticipant {
    pub fn new(channel_id: ChannelId, agent_id: AgentId) -> Self {
        Self {
            channel_id,
            agent_id,
            position: 0,
            joined_at: Utc::now(),
        }
    }

    /// Set the position in the group's ordered participant roster.
    pub fn with_position(mut self, position: u32) -> Self {
        self.position = position;
        self
    }
}

/// Whether a conversation has one responding agent or several.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationMode {
    #[default]
    Single,
    Group,
}

impl ConversationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Group => "group",
        }
    }
}

impl std::str::FromStr for ConversationMode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "single" => Ok(Self::Single),
            "group" => Ok(Self::Group),
            _ => Err(()),
        }
    }
}

impl ConversationMode {
    /// The mode implied by a participant count.
    ///
    /// One participant is a single conversation; two or more are a group. The
    /// service validates bounds separately because zero is invalid for a
    /// conversation.
    pub fn for_participant_count(count: usize) -> Self {
        if count >= MIN_GROUP_PARTICIPANTS {
            Self::Group
        } else {
            Self::Single
        }
    }
}

/// Default automatic compaction threshold, as a percentage of the usable window.
pub const DEFAULT_COMPACT_THRESHOLD: u32 = 80;

/// Tokens reserved for the response and tokenizer drift when deciding how much
/// of a model window is usable for input.
pub const CONTEXT_WINDOW_BUFFER_TOKENS: u32 = 1024;

/// Per-conversation settings stored independently from the channel row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSettings {
    pub channel_id: ChannelId,
    pub mode: ConversationMode,
    /// Provider that supplies the fixed model, when one was recorded.
    pub provider_id: Option<String>,
    /// Model every turn of this conversation uses.
    pub model: String,
    /// Participant that answers an unaddressed message, when one is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_responder: Option<AgentId>,
    /// Whether an unaddressed group message may be routed by the model.
    pub router_enabled: bool,
    /// Whether the runtime may compact the context automatically.
    pub auto_compact: bool,
    /// Percentage of the usable window that triggers automatic compaction.
    pub compact_threshold: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConversationSettings {
    pub fn new(
        channel_id: ChannelId,
        provider_id: Option<String>,
        model: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            channel_id,
            mode: ConversationMode::Single,
            provider_id,
            model: model.into(),
            default_responder: None,
            router_enabled: false,
            auto_compact: false,
            compact_threshold: DEFAULT_COMPACT_THRESHOLD,
            created_at: now,
            updated_at: now,
        }
    }

    /// Create settings for a group conversation.
    ///
    /// The default responder must already be one of the group's participants;
    /// the service validates the roster before persisting the settings.
    pub fn group(
        channel_id: ChannelId,
        provider_id: Option<String>,
        model: impl Into<String>,
        default_responder: Option<AgentId>,
    ) -> Self {
        let mut settings = Self::new(channel_id, provider_id, model);
        settings.mode = ConversationMode::Group;
        settings.default_responder = default_responder;
        settings
    }
}

/// Editable subset of conversation settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSettingsUpdate {
    pub auto_compact: Option<bool>,
    pub compact_threshold: Option<u32>,
    /// Turn automatic routing on or off; single conversations ignore it.
    pub router_enabled: Option<bool>,
    /// Replace the participant that answers unaddressed messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_responder: Option<AgentId>,
}

/// Role of one context entry when it is assembled into a model request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    System,
    User,
    Assistant,
    Tool,
}

impl ContextRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

impl std::str::FromStr for ContextRole {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "system" => Ok(Self::System),
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "tool" => Ok(Self::Tool),
            _ => Err(()),
        }
    }
}

/// What produced an entry: an ordinary message or a compaction summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextEntryKind {
    Message,
    Summary,
}

impl ContextEntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Summary => "summary",
        }
    }
}

impl std::str::FromStr for ContextEntryKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "message" => Ok(Self::Message),
            "summary" => Ok(Self::Summary),
            _ => Err(()),
        }
    }
}

/// A tool call stored inside a context entry.
///
/// `cowbot-core` does not depend on the LLM crate, so the shape is mirrored
/// here and converted at the service boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// One append-only entry in an agent's branch of a conversation context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextEntry {
    pub id: String,
    pub channel_id: ChannelId,
    pub agent_id: AgentId,
    /// Speaker when this entry mirrors another participant's reply.
    ///
    /// Mirrored entries use the user role so they fit a chat transcript, but
    /// they are not the user's own words. Keeping the speaker separate lets
    /// projection and summarization treat them as history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_agent_id: Option<AgentId>,
    /// Monotonic position inside the `(channel, agent)` branch.
    ///
    /// Compaction renumbers the kept tail to free the cutoff, so this is an
    /// ordering key, not a stable identifier.
    pub seq: i64,
    pub role: ContextRole,
    pub kind: ContextEntryKind,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ContextToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Excluded entries stay on disk but are no longer sent to the model.
    pub excluded: bool,
    /// Estimated tokens of this entry.
    pub tokens: u32,
    pub created_at: DateTime<Utc>,
}

impl ContextEntry {
    pub fn new(
        channel_id: ChannelId,
        agent_id: AgentId,
        role: ContextRole,
        content: impl Into<String>,
    ) -> Self {
        let content = content.into();
        let tokens = estimate_tokens(&content);
        Self {
            id: Uuid::new_v4().to_string(),
            channel_id,
            agent_id,
            author_agent_id: None,
            seq: 0,
            role,
            kind: ContextEntryKind::Message,
            content,
            tool_calls: None,
            tool_call_id: None,
            excluded: false,
            tokens,
            created_at: Utc::now(),
        }
    }

    pub fn summary(channel_id: ChannelId, agent_id: AgentId, content: impl Into<String>) -> Self {
        let mut entry = Self::new(channel_id, agent_id, ContextRole::System, content);
        entry.kind = ContextEntryKind::Summary;
        entry
    }

    /// Mirror another participant's reply into `agent_id`'s branch.
    ///
    /// The entry belongs to the reader's branch, uses the user role, and
    /// records the speaker separately from the user. The content carries the
    /// speaker's display name so the model can attribute it.
    pub fn peer(
        channel_id: ChannelId,
        agent_id: AgentId,
        author: AgentId,
        author_name: &str,
        content: impl Into<String>,
    ) -> Self {
        let mut entry = Self::new(
            channel_id,
            agent_id,
            ContextRole::User,
            format!("[{author_name}] {}", content.into().trim()),
        );
        entry.author_agent_id = Some(author);
        entry
    }

    /// Recompute [`Self::tokens`], including attached tool calls.
    pub fn recount(&mut self) {
        let mut tokens = estimate_tokens(&self.content);
        if let Some(tool_calls) = &self.tool_calls {
            if let Ok(serialized) = serde_json::to_string(tool_calls) {
                tokens = tokens.saturating_add(estimate_tokens(&serialized));
            }
        }
        if let Some(tool_call_id) = &self.tool_call_id {
            tokens = tokens.saturating_add(estimate_tokens(tool_call_id));
        }
        self.tokens = tokens;
    }
}

/// Why a context event was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextEventKind {
    Compact,
    Clear,
}

impl ContextEventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Clear => "clear",
        }
    }
}

impl std::str::FromStr for ContextEventKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "compact" => Ok(Self::Compact),
            "clear" => Ok(Self::Clear),
            _ => Err(()),
        }
    }
}

/// Audit record for a change that replaced or removed context entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextEvent {
    pub id: String,
    pub channel_id: ChannelId,
    pub agent_id: AgentId,
    pub kind: ContextEventKind,
    pub detail: Option<String>,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub created_at: DateTime<Utc>,
}

impl ContextEvent {
    pub fn new(
        channel_id: ChannelId,
        agent_id: AgentId,
        kind: ContextEventKind,
        tokens_before: u32,
        tokens_after: u32,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            channel_id,
            agent_id,
            kind,
            detail: None,
            tokens_before,
            tokens_after,
            created_at: Utc::now(),
        }
    }
}

/// Token breakdown of a conversation's next model request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextProjection {
    /// System prompt assembled for the participant agent.
    pub system_tokens: u32,
    /// Tool schemas the agent may call.
    pub tool_tokens: u32,
    /// Active branch entries before the latest user message.
    pub history_tokens: u32,
    /// The latest active user message.
    pub latest_tokens: u32,
    pub active_entries: u32,
    pub excluded_entries: u32,
}

impl ContextProjection {
    pub fn total_tokens(&self) -> u32 {
        self.system_tokens
            .saturating_add(self.tool_tokens)
            .saturating_add(self.history_tokens)
            .saturating_add(self.latest_tokens)
    }
}

/// Build the token breakdown for a conversation branch.
///
/// The latest active user entry is reported separately from the rest of the
/// history so the meter can show what the newest message contributes.
pub fn project_context(
    system_tokens: u32,
    tool_tokens: u32,
    entries: &[ContextEntry],
) -> ContextProjection {
    let active = entries
        .iter()
        .filter(|entry| !entry.excluded)
        .collect::<Vec<_>>();
    let latest_index = active
        .iter()
        .rposition(|entry| entry.role == ContextRole::User && entry.author_agent_id.is_none());
    let latest_tokens = latest_index.map(|index| active[index].tokens).unwrap_or(0);
    let history_tokens = active
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != latest_index)
        .fold(0_u32, |total, (_, entry)| {
            total.saturating_add(entry.tokens)
        });

    ContextProjection {
        system_tokens,
        tool_tokens,
        history_tokens,
        latest_tokens,
        active_entries: active.len() as u32,
        excluded_entries: (entries.len() - active.len()) as u32,
    }
}

/// One participant's context meter inside a group conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationParticipantContext {
    pub agent_id: AgentId,
    pub agent_name: String,
    pub projection: ContextProjection,
    /// Estimated tokens the next turn's memory block would add.
    #[serde(default)]
    pub memory_tokens: u32,
    /// Cache summary of the agent's latest run in this conversation.
    pub cache_last_run: CacheUsageSummary,
    /// Cache summary of every call in this agent's branch.
    pub cache_conversation: CacheUsageSummary,
}

/// Everything the desktop needs to render a conversation's context meter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationContext {
    pub channel_id: ChannelId,
    /// First participant; kept for single-conversation clients.
    pub agent_id: AgentId,
    pub model: String,
    pub provider_id: Option<String>,
    pub projection: ContextProjection,
    pub cache_last_turn: CacheUsageSummary,
    pub cache_conversation: CacheUsageSummary,
    /// One entry per participant, in participant order.
    #[serde(default)]
    pub participants: Vec<ConversationParticipantContext>,
}

/// Result of a manual or automatic compaction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompactionOutcome {
    pub compacted: bool,
    pub summarized_entries: u32,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub summary: Option<String>,
    /// True when another compaction already held the branch.
    #[serde(default)]
    pub in_progress: bool,
    /// Why nothing was written, for the in-progress, stale, empty, and
    /// nothing-to-summarize paths.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Result of the guarded store write behind one compaction.
///
/// The service decides what to summarize; the store decides whether the
/// branch still matches that decision and applies the summary at the cutoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionWrite {
    /// The summary was inserted at the cutoff.
    Applied {
        summary_seq: i64,
        /// Active tokens at write time, including entries appended during the
        /// model call.
        tokens_before: u32,
        tokens_after: u32,
    },
    /// The summarized prefix is no longer active, or the cutoff no longer
    /// matches the branch. Nothing was written.
    Stale,
    /// The branch was cleared while the summary was being generated. Nothing
    /// was written.
    Empty,
}

/// Why a group message selected the participants it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteSource {
    /// The user mentioned the selected participants.
    Mention,
    /// The router selected the participants.
    Router,
    /// No participant was mentioned; the default responder answers.
    Default,
    /// No participant was mentioned and no default responder is configured.
    Silent,
    /// The message mentioned only names outside the participant roster.
    Unmatched,
}

impl RouteSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mention => "mention",
            Self::Router => "router",
            Self::Default => "default",
            Self::Silent => "silent",
            Self::Unmatched => "unmatched",
        }
    }
}

impl std::str::FromStr for RouteSource {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "mention" => Ok(Self::Mention),
            "router" => Ok(Self::Router),
            "default" => Ok(Self::Default),
            "silent" => Ok(Self::Silent),
            "unmatched" => Ok(Self::Unmatched),
            _ => Err(()),
        }
    }
}

/// Unique identifier for one routed group message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(pub String);

impl TurnId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for TurnId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Persisted routing decision for one group user message.
///
/// The row is the group turn record: it links the message, the selection
/// source, the selected participants, and the tasks created for them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub id: TurnId,
    pub channel_id: ChannelId,
    pub message_id: String,
    pub source: RouteSource,
    #[serde(default)]
    pub selected_agents: Vec<AgentId>,
    #[serde(default)]
    pub unknown_mentions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_call_id: Option<String>,
    #[serde(default)]
    pub task_ids: Vec<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirrored_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl ConversationTurn {
    pub fn new(channel_id: ChannelId, message_id: impl Into<String>, source: RouteSource) -> Self {
        Self {
            id: TurnId::random(),
            channel_id,
            message_id: message_id.into(),
            source,
            selected_agents: Vec::new(),
            unknown_mentions: Vec::new(),
            reason: None,
            router_call_id: None,
            task_ids: Vec::new(),
            mirrored_at: None,
            created_at: Utc::now(),
        }
    }
}

/// The context-window input a model can actually use for a request.
///
/// The window excludes the model's maximum output and a small buffer for
/// tokenizer drift. Unknown metadata yields `None`, and the UI then shows
/// absolute token counts without a percentage.
pub fn usable_window(context_window: Option<u32>, max_output_tokens: Option<u32>) -> Option<u32> {
    let window = context_window?;
    let usable = window
        .saturating_sub(max_output_tokens.unwrap_or(0))
        .saturating_sub(CONTEXT_WINDOW_BUFFER_TOKENS);
    (usable > 0).then_some(usable)
}

/// How full the context is, used for colouring the meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextUsageState {
    /// Below 70% of the usable window.
    Neutral,
    /// At or above 70% and below 90%.
    Warning,
    /// At or above 90% and below 100%.
    Danger,
    /// At or above 100%: sending is blocked.
    Full,
    /// No context window metadata is available.
    Unknown,
}

/// Percent used and its state, or `None` when the window is unknown.
pub fn context_usage_state(
    used_tokens: u32,
    window: Option<u32>,
) -> (Option<f32>, ContextUsageState) {
    let Some(window) = window else {
        return (None, ContextUsageState::Unknown);
    };
    if window == 0 {
        return (None, ContextUsageState::Unknown);
    }
    let percent = used_tokens as f32 * 100.0 / window as f32;
    let state = if percent >= 100.0 {
        ContextUsageState::Full
    } else if percent >= 90.0 {
        ContextUsageState::Danger
    } else if percent >= 70.0 {
        ContextUsageState::Warning
    } else {
        ContextUsageState::Neutral
    };
    (Some(percent), state)
}

/// How many tokens of a branch are kept verbatim by a compaction.
pub const CONTEXT_KEEP_RECENT_TOKENS: u32 = 2_000;

/// Largest transcript sent to the summarizer in one compaction call.
pub const CONTEXT_SUMMARY_INPUT_TOKENS: u32 = 24_000;

/// Where a compaction splits the active branch.
///
/// Entries before the returned index are summarized; entries from it onwards
/// are kept. The split keeps the most recent [`CONTEXT_KEEP_RECENT_TOKENS`],
/// at least the last two entries, and never separates a tool result from the
/// assistant message that requested it.
pub fn compaction_split(entries: &[ContextEntry]) -> usize {
    if entries.len() <= 2 {
        return 0;
    }

    let mut index = entries.len();
    let mut kept_tokens = 0_u32;
    while index > 0 && kept_tokens < CONTEXT_KEEP_RECENT_TOKENS {
        index -= 1;
        kept_tokens = kept_tokens.saturating_add(entries[index].tokens);
    }

    let minimum_kept = entries.len().saturating_sub(2);
    index = index.min(minimum_kept);
    while index > 0 && entries[index].role == ContextRole::Tool {
        index -= 1;
    }

    // A previous summary already compresses older history. Summarizing it
    // alone would replace one summary with another without incorporating new
    // messages; this is the state right after a compaction, when the summary
    // is the oldest active entry.
    if index > 0
        && entries[..index]
            .iter()
            .all(|entry| entry.kind == ContextEntryKind::Summary)
    {
        return 0;
    }

    index
}

/// Cache hit information for a set of calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheUsageSummary {
    pub calls: u64,
    pub cached_input_tokens: u64,
    pub total_input_tokens: u64,
}

impl CacheUsageSummary {
    /// Cached share of all input tokens, or `None` when no input was reported.
    pub fn hit_rate(&self) -> Option<f32> {
        if self.total_input_tokens == 0 {
            return None;
        }
        Some(self.cached_input_tokens as f32 / self.total_input_tokens as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: ContextRole, tokens: u32) -> ContextEntry {
        let mut entry =
            ContextEntry::new(ChannelId::new("channel"), AgentId::new("agent"), role, "x");
        entry.tokens = tokens;
        entry
    }

    #[test]
    fn channel_kinds_round_trip_through_strings() {
        for kind in [
            ChannelKind::Channel,
            ChannelKind::Direct,
            ChannelKind::Conversation,
        ] {
            assert_eq!(kind.as_str().parse::<ChannelKind>(), Ok(kind));
        }
        assert!("unknown".parse::<ChannelKind>().is_err());
    }

    #[test]
    fn entry_tokens_include_tool_calls() {
        let mut entry = ContextEntry::new(
            ChannelId::new("c"),
            AgentId::new("a"),
            ContextRole::Assistant,
            "working",
        );
        assert_eq!(entry.tokens, estimate_tokens("working"));
        entry.tool_calls = Some(vec![ContextToolCall {
            id: "call-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({ "path": "src/main.rs" }),
        }]);
        entry.recount();
        assert!(entry.tokens > estimate_tokens("working"));
    }

    #[test]
    fn compaction_keeps_the_recent_tail_and_tool_pairs() {
        let entries = vec![
            entry(ContextRole::User, 1_000),
            entry(ContextRole::Assistant, 1_000),
            entry(ContextRole::User, 1_000),
            entry(ContextRole::Assistant, 1_000),
            entry(ContextRole::Tool, 1_000),
            entry(ContextRole::Tool, 1_000),
            entry(ContextRole::Assistant, 500),
        ];
        // The tail budget reaches the first tool result; the split moves back
        // so the assistant that requested those tools is also summarized only
        // when the whole pair group moves together.
        let split = compaction_split(&entries);
        assert!(split <= entries.len() - 2);
        assert_ne!(entries[split].role, ContextRole::Tool);
    }

    #[test]
    fn compaction_keeps_at_least_two_entries() {
        let entries = vec![
            entry(ContextRole::User, 5_000),
            entry(ContextRole::Assistant, 5_000),
            entry(ContextRole::User, 5_000),
        ];
        assert_eq!(compaction_split(&entries), 1);
        assert_eq!(compaction_split(&entries[..2]), 0);
    }

    #[test]
    fn usable_window_excludes_output_and_buffer() {
        assert_eq!(usable_window(Some(128_000), Some(8_000)), Some(118_976));
        assert_eq!(usable_window(Some(500), None), None);
        assert_eq!(usable_window(None, Some(4_000)), None);
    }

    #[test]
    fn usage_states_follow_thresholds() {
        assert_eq!(
            context_usage_state(69, Some(100)),
            (Some(69.0), ContextUsageState::Neutral)
        );
        assert_eq!(
            context_usage_state(70, Some(100)),
            (Some(70.0), ContextUsageState::Warning)
        );
        assert_eq!(
            context_usage_state(90, Some(100)),
            (Some(90.0), ContextUsageState::Danger)
        );
        assert_eq!(
            context_usage_state(100, Some(100)),
            (Some(100.0), ContextUsageState::Full)
        );
        assert_eq!(
            context_usage_state(10, None),
            (None, ContextUsageState::Unknown)
        );
    }

    #[test]
    fn cache_hit_rate_is_cached_share_of_input() {
        let summary = CacheUsageSummary {
            calls: 2,
            cached_input_tokens: 30,
            total_input_tokens: 100,
        };
        assert_eq!(summary.hit_rate(), Some(0.3));
        assert_eq!(CacheUsageSummary::default().hit_rate(), None);
    }

    #[test]
    fn projection_splits_the_latest_user_message_from_history() {
        let mut entries = vec![
            entry(ContextRole::User, 10),
            entry(ContextRole::Assistant, 20),
            entry(ContextRole::User, 30),
            entry(ContextRole::Assistant, 40),
        ];
        entries[0].excluded = true;

        let projection = project_context(100, 50, &entries);
        assert_eq!(projection.system_tokens, 100);
        assert_eq!(projection.tool_tokens, 50);
        assert_eq!(projection.latest_tokens, 30);
        assert_eq!(projection.history_tokens, 60);
        assert_eq!(projection.active_entries, 3);
        assert_eq!(projection.excluded_entries, 1);
        assert_eq!(projection.total_tokens(), 240);
    }

    #[test]
    fn projection_without_a_user_message_counts_all_entries_as_history() {
        let entries = vec![
            entry(ContextRole::System, 5),
            entry(ContextRole::Assistant, 7),
        ];
        let projection = project_context(0, 0, &entries);
        assert_eq!(projection.latest_tokens, 0);
        assert_eq!(projection.history_tokens, 12);
    }

    #[test]
    fn participant_positions_round_trip_and_default_for_old_payloads() {
        let participant =
            ChannelParticipant::new(ChannelId::new("c"), AgentId::new("a")).with_position(3);
        assert_eq!(participant.position, 3);

        let json = serde_json::to_string(&participant).unwrap();
        assert!(json.contains("\"position\":3"));
        let restored: ChannelParticipant = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.position, 3);

        let old: ChannelParticipant = serde_json::from_str(
            r#"{"channel_id":"c","agent_id":"a","joined_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(old.position, 0);
    }

    #[test]
    fn conversation_mode_follows_participant_count() {
        assert_eq!(
            ConversationMode::for_participant_count(1),
            ConversationMode::Single
        );
        assert_eq!(
            ConversationMode::for_participant_count(MIN_GROUP_PARTICIPANTS),
            ConversationMode::Group
        );
        assert_eq!(
            ConversationMode::for_participant_count(MAX_GROUP_PARTICIPANTS),
            ConversationMode::Group
        );
    }

    #[test]
    fn group_settings_record_the_mode_and_default_responder() {
        let settings = ConversationSettings::group(
            ChannelId::new("c"),
            Some("provider-1".into()),
            "model-1",
            Some(AgentId::new("main")),
        );

        assert_eq!(settings.mode, ConversationMode::Group);
        assert_eq!(
            settings
                .default_responder
                .as_ref()
                .map(|agent| agent.0.as_str()),
            Some("main")
        );
        assert!(!settings.router_enabled);
        assert!(!settings.auto_compact);
    }

    #[test]
    fn settings_update_accepts_router_and_default_responder() {
        let update = ConversationSettingsUpdate {
            router_enabled: Some(true),
            default_responder: Some(AgentId::new("main")),
            ..ConversationSettingsUpdate::default()
        };

        assert_eq!(update.router_enabled, Some(true));
        assert_eq!(
            update
                .default_responder
                .as_ref()
                .map(|agent| agent.0.as_str()),
            Some("main")
        );
    }

    #[test]
    fn peer_entries_are_history_not_the_latest_user_message() {
        let user = entry(ContextRole::User, 10);
        let mut peer = entry(ContextRole::User, 30);
        peer.author_agent_id = Some(AgentId::new("teammate"));

        let projection = project_context(0, 0, &[user, peer]);
        assert_eq!(projection.latest_tokens, 10);
        assert_eq!(projection.history_tokens, 30);
    }

    #[test]
    fn peer_entries_carry_an_attributed_speaker() {
        let peer = ContextEntry::peer(
            ChannelId::new("c"),
            AgentId::new("reader"),
            AgentId::new("author"),
            "Planner",
            "The plan is ready.",
        );

        assert_eq!(peer.role, ContextRole::User);
        assert_eq!(peer.author_agent_id, Some(AgentId::new("author")));
        assert_eq!(peer.content, "[Planner] The plan is ready.");
        assert!(peer.tokens > 0);
    }

    #[test]
    fn context_entries_default_their_author_for_old_payloads() {
        let json = r#"{"id":"e","channel_id":"c","agent_id":"a","seq":1,
            "role":"user","kind":"message","content":"hi","excluded":false,
            "tokens":1,"created_at":"2026-01-01T00:00:00Z"}"#;
        let entry: ContextEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.author_agent_id, None);
    }

    #[test]
    fn conversation_turns_round_trip_and_sources_parse() {
        let turn = ConversationTurn::new(ChannelId::new("c"), "m1", RouteSource::Router);
        let json = serde_json::to_string(&turn).unwrap();
        assert!(json.contains("\"source\":\"router\""));
        let restored: ConversationTurn = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, turn.id);
        assert_eq!(restored.message_id, "m1");
        assert_eq!(restored.source, RouteSource::Router);

        for source in [
            RouteSource::Mention,
            RouteSource::Router,
            RouteSource::Default,
            RouteSource::Silent,
            RouteSource::Unmatched,
        ] {
            assert_eq!(source.as_str().parse::<RouteSource>(), Ok(source));
        }
        assert!("unknown".parse::<RouteSource>().is_err());
    }

    #[test]
    fn participant_contexts_default_to_empty_for_old_payloads() {
        let json = serde_json::json!({
            "channel_id": "c",
            "agent_id": "a",
            "model": "m",
            "provider_id": null,
            "projection": {
                "system_tokens": 0,
                "tool_tokens": 0,
                "history_tokens": 0,
                "latest_tokens": 0,
                "active_entries": 0,
                "excluded_entries": 0
            },
            "cache_last_turn": {
                "calls": 0,
                "cached_input_tokens": 0,
                "total_input_tokens": 0
            },
            "cache_conversation": {
                "calls": 0,
                "cached_input_tokens": 0,
                "total_input_tokens": 0
            }
        });
        let context: ConversationContext = serde_json::from_value(json).unwrap();
        assert!(context.participants.is_empty());
    }

    #[test]
    fn compaction_outcome_defaults_new_fields_for_old_payloads() {
        let json = r#"{"compacted":true,"summarized_entries":2,"tokens_before":10,
            "tokens_after":4,"summary":"earlier history"}"#;
        let outcome: CompactionOutcome = serde_json::from_str(json).unwrap();
        assert!(outcome.compacted);
        assert!(!outcome.in_progress);
        assert_eq!(outcome.reason, None);
    }

    #[test]
    fn compaction_split_does_not_resummarize_a_lone_summary() {
        let mut summary = ContextEntry::summary(
            ChannelId::new("channel"),
            AgentId::new("agent"),
            "earlier history",
        );
        summary.tokens = 10;
        let first = entry(ContextRole::User, 1_000);
        let second = entry(ContextRole::Assistant, 1_000);
        let third = entry(ContextRole::User, 1_000);

        // A summary plus the two kept entries: only the summary is old.
        assert_eq!(
            compaction_split(&[summary.clone(), first.clone(), second.clone()]),
            0
        );

        // A summary with an older message and two kept entries: there is new
        // material to fold into the next summary.
        assert_eq!(compaction_split(&[summary, first, second, third]), 2);
    }
}
