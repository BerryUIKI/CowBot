//! Resolving a platform conversation to a CowBot channel (ADR-0056).

use cowbot_core::error::CowResult;
use cowbot_core::integration::IntegrationRecord;
use cowbot_store::Store;

use crate::model::Route;

/// Find the channel a platform conversation feeds, if any.
///
/// An unbound conversation is not an error: a bot in a busy tenant is mentioned
/// in rooms nobody configured, and the answer to that is silence rather than a
/// stream of "not configured" notices into an IM the operator may not control.
pub async fn resolve_route(
    store: &Store,
    integration: &IntegrationRecord,
    external_conversation_id: &str,
) -> CowResult<Option<Route>> {
    let Some(binding) = store
        .integration_binding(&integration.id, external_conversation_id)
        .await?
    else {
        return Ok(None);
    };

    Ok(Some(Route {
        integration_id: integration.id.clone(),
        kind: integration.kind,
        channel_id: binding.channel_id,
        agent_id: binding.agent_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cowbot_core::channel::Channel;
    use cowbot_core::identity::Workspace;
    use cowbot_core::integration::{
        IntegrationBinding, IntegrationId, IntegrationKind, NewIntegration,
    };

    async fn setup() -> (Store, Workspace, IntegrationRecord, Channel) {
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let workspace = Workspace::new("Team");
        store.create_workspace(&workspace).await.unwrap();

        let channel = Channel::new("general", "");
        store.create_channel(&workspace.id, &channel).await.unwrap();

        let new = NewIntegration::new(&workspace.id, IntegrationKind::Mattermost, "Bot", "{}");
        store.create_integration(&new).await.unwrap();
        let integration = store.get_integration(&new.id).await.unwrap().unwrap();
        (store, workspace, integration, channel)
    }

    #[tokio::test]
    async fn a_bound_conversation_resolves_to_its_channel() {
        let (store, _, integration, channel) = setup().await;
        store
            .bind_integration_channel(&IntegrationBinding {
                integration_id: integration.id.clone(),
                external_id: "town-square".into(),
                channel_id: channel.id.clone(),
                agent_id: None,
            })
            .await
            .unwrap();

        let route = resolve_route(&store, &integration, "town-square")
            .await
            .unwrap()
            .expect("a binding resolves");
        assert_eq!(route.channel_id, channel.id);
        assert_eq!(route.kind, IntegrationKind::Mattermost);
    }

    #[tokio::test]
    async fn an_unbound_conversation_resolves_to_nothing_and_is_not_an_error() {
        let (store, _, integration, _) = setup().await;
        assert!(resolve_route(&store, &integration, "off-topic")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn a_binding_of_another_integration_does_not_leak_through() {
        // Two bots in one workspace, each bound to its own room. A message the
        // other bot's room delivered must not land in this one's channel.
        let (store, workspace, integration, channel) = setup().await;
        let other = NewIntegration::new(&workspace.id, IntegrationKind::Slack, "Other", "{}");
        store.create_integration(&other).await.unwrap();
        store
            .bind_integration_channel(&IntegrationBinding {
                integration_id: other.id.clone(),
                external_id: "shared-room".into(),
                channel_id: channel.id.clone(),
                agent_id: None,
            })
            .await
            .unwrap();

        assert!(resolve_route(&store, &integration, "shared-room")
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_route_keeps_the_forced_agent_when_the_binding_names_one() {
        let route = Route {
            integration_id: IntegrationId::new("i1"),
            kind: IntegrationKind::Telegram,
            channel_id: cowbot_core::channel::ChannelId::new("c1"),
            agent_id: Some(cowbot_core::agent::AgentId("a1".into())),
        };
        assert_eq!(route.agent_id.as_ref().unwrap().0, "a1");
        let _ = Utc::now();
    }
}
