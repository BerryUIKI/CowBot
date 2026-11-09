use cowbot_bus::EventBus;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::event::{Event, EventPayload};
use cowbot_core::identity::WorkspaceId;
use cowbot_store::Store;

use crate::channels::ChannelService;

/// Editable fields of an agent profile.
#[derive(Debug, Clone)]
pub struct AgentUpdate {
    pub id: AgentId,
    pub name: String,
    pub role: String,
    pub tools: Vec<String>,
}

/// Longest accepted agent name, in characters.
const MAX_AGENT_NAME: usize = 80;

/// Application service for agent profile updates.
#[derive(Clone)]
pub struct AgentService {
    store: Store,
    bus: EventBus,
    channels: ChannelService,
}

impl AgentService {
    pub fn new(store: Store, bus: EventBus) -> Self {
        Self {
            channels: ChannelService::new(store.clone(), bus.clone()),
            store,
            bus,
        }
    }

    /// Update an agent's display name, role, and tool selection.
    ///
    /// Names are unique per workspace, case-insensitively, because mentions and
    /// task assignment resolve agents by name. The agent's direct conversation
    /// follows the new name and role.
    pub async fn update_agent(
        &self,
        workspace_id: &WorkspaceId,
        update: &AgentUpdate,
    ) -> CowResult<AgentProfile> {
        let Some(mut agent) = self.store.get_agent(&update.id).await? else {
            return Err(CowError::Config(format!(
                "agent '{}' was not found",
                update.id.0
            )));
        };

        let name = update.name.trim();
        if name.is_empty() {
            return Err(CowError::Config("agent name cannot be empty".into()));
        }
        if name.chars().count() > MAX_AGENT_NAME {
            return Err(CowError::Config(format!(
                "agent name must be {MAX_AGENT_NAME} characters or fewer"
            )));
        }

        let normalized = name.to_lowercase();
        let agents = self.store.list_agents(workspace_id).await?;
        if agents
            .iter()
            .any(|other| other.id != agent.id && other.name.to_lowercase() == normalized)
        {
            return Err(CowError::Config(format!(
                "an agent named '{name}' already exists"
            )));
        }

        let mut tools: Vec<String> = Vec::new();
        for tool in &update.tools {
            let tool = tool.trim();
            if !tool.is_empty() && !tools.iter().any(|existing| existing == tool) {
                tools.push(tool.to_string());
            }
        }

        agent.name = name.to_string();
        agent.role = update.role.trim().to_string();
        agent.tools = tools;
        self.store.upsert_agent(workspace_id, &agent).await?;
        self.channels
            .ensure_agent_channel(workspace_id, &agent)
            .await?;
        self.bus
            .publish(Event::new(EventPayload::AgentUpdated(agent.clone())));
        Ok(agent)
    }
}
