use crate::agent::{AgentId, AgentProfile};
use crate::conversation::ChannelKind;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a collaboration channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChannelId(pub String);

impl ChannelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn random() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Message sender identity (Agent, Human user, or System).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id")]
pub enum Sender {
    Agent(AgentId),
    User(String),
    System,
}

/// A collaboration channel where agents and users interact (inspired by Mattermost).
///
/// A channel with `agent_id` set is that agent's direct conversation: it has a
/// single addressee, so messages do not need an `@mention` to reach the agent.
/// User-created conversations have no `agent_id`; their participants live in
/// `channel_participants` and their context is independent (ADR-0030).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: ChannelId,
    pub name: String,
    pub topic: String,
    pub created_at: DateTime<Utc>,
    /// Agent that owns this direct conversation, if any.
    #[serde(default)]
    pub agent_id: Option<AgentId>,
    /// Whether the user pinned this channel or conversation.
    #[serde(default)]
    pub pinned: bool,
    /// What this row represents. Legacy rows default to a group channel; the
    /// migration marks agent-owned rows as direct.
    #[serde(default)]
    pub kind: ChannelKind,
}

impl Channel {
    pub fn new(name: impl Into<String>, topic: impl Into<String>) -> Self {
        Self {
            id: ChannelId::random(),
            name: name.into(),
            topic: topic.into(),
            created_at: Utc::now(),
            agent_id: None,
            pinned: false,
            kind: ChannelKind::Channel,
        }
    }

    /// Create the direct conversation channel for `agent`.
    pub fn direct(agent: &AgentProfile, topic: impl Into<String>) -> Self {
        Self {
            id: ChannelId::random(),
            name: agent.name.clone(),
            topic: topic.into(),
            created_at: Utc::now(),
            agent_id: Some(agent.id.clone()),
            pinned: false,
            kind: ChannelKind::Direct,
        }
    }

    /// Create a user-created conversation channel.
    pub fn conversation(name: impl Into<String>, topic: impl Into<String>) -> Self {
        Self {
            id: ChannelId::random(),
            name: name.into(),
            topic: topic.into(),
            created_at: Utc::now(),
            agent_id: None,
            pinned: false,
            kind: ChannelKind::Conversation,
        }
    }

    pub fn is_direct(&self) -> bool {
        self.kind == ChannelKind::Direct
    }

    pub fn is_conversation(&self) -> bool {
        self.kind == ChannelKind::Conversation
    }
}

/// A structured message posted to a channel or thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessage {
    pub id: String,
    pub channel_id: ChannelId,
    pub sender: Sender,
    pub content: String,
    pub thread_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl ChannelMessage {
    pub fn new(channel_id: ChannelId, sender: Sender, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            channel_id,
            sender,
            content: content.into(),
            thread_id: None,
            created_at: Utc::now(),
        }
    }

    pub fn with_thread(mut self, thread_id: impl Into<String>) -> Self {
        self.thread_id = Some(thread_id.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_without_agent_id_still_deserialize() {
        let json = r#"{"id":"c1","name":"general","topic":"","created_at":"2026-01-01T00:00:00Z"}"#;
        let channel: Channel = serde_json::from_str(json).unwrap();
        assert_eq!(channel.agent_id, None);
        assert_eq!(channel.kind, ChannelKind::Channel);
    }

    #[test]
    fn conversation_channels_have_no_agent_id_and_a_kind() {
        let channel = Channel::conversation("Design review", "Week 12");
        assert!(channel.is_conversation());
        assert!(channel.agent_id.is_none());
        let json = serde_json::to_string(&channel).unwrap();
        assert!(json.contains("\"kind\":\"conversation\""));
        let restored: Channel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.kind, ChannelKind::Conversation);
    }

    #[test]
    fn direct_channels_keep_the_agent_id() {
        let agent = AgentProfile::new("Planner", "Architect", "Plan", "Careful");
        let channel = Channel::direct(&agent, "Architect");
        assert_eq!(channel.agent_id.as_ref(), Some(&agent.id));

        let json = serde_json::to_string(&channel).unwrap();
        let restored: Channel = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.agent_id.as_ref(), Some(&agent.id));
    }
}
