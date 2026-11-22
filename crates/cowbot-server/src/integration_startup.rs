//! Starting the IM integrations a deployment configured (ADR-0057).
//!
//! Startup, not a route: a bot has to be running before anyone messages it, and a
//! route that starts one would need a supervisor of its own to notice a restart.
//!
//! Nothing here runs when the deployment configured no model provider. An
//! integration whose messages are stored but never answered is worse than one
//! that is not running: the IM user sees their question accepted and waits.
//!
//! The inbound half and the outgoing mirror both go through the shared
//! supervisor, so this module is the wiring and nothing else.

use std::sync::Arc;

use cowbot_bus::EventBus;
use cowbot_core::channel::ChannelId;
use cowbot_core::error::CowResult;
use cowbot_integrations::sink::{ChannelSink, TurnStart};
use cowbot_integrations::{Registry, Started};
use cowbot_service::ChannelService;
use cowbot_store::Store;
use tracing::{info, warn};

use crate::turn_executor::TurnExecutor;

/// Starts a turn for a message the pipeline just stored.
struct ExecutorTurns {
    store: Store,
    executor: Arc<TurnExecutor>,
}

impl TurnStart for ExecutorTurns {
    fn start(&self, channel_id: ChannelId, message_id: String) {
        let store = self.store.clone();
        let executor = self.executor.clone();
        // Not awaited by the caller: an adapter has to acknowledge the platform
        // promptly, and a model turn takes far longer than any platform's
        // patience.
        tokio::spawn(async move {
            match store.get_message(&message_id).await {
                Ok(Some(message)) => executor.spawn_reply(channel_id, message),
                Ok(None) => warn!(
                    message = %message_id,
                    "a stored message vanished before its turn could start"
                ),
                Err(error) => warn!(
                    message = %message_id,
                    %error,
                    "could not read a stored message to start its turn"
                ),
            }
        });
    }
}

/// Start every active integration in every workspace this database holds.
///
/// A workspace whose adapters all fail to start does not stop the others: one
/// misconfigured bot must not take three working ones offline.
pub async fn start_all(
    store: &Store,
    bus: &EventBus,
    executor: Arc<TurnExecutor>,
) -> CowResult<Started> {
    let mut started = Started::default();
    let registry = crate::adapters::registry();

    for workspace in store.list_workspaces().await? {
        let records = store.list_active_integrations(&workspace.id).await?;
        if records.is_empty() {
            continue;
        }

        // The sink is per integration because it carries the integration's
        // identity, and an adapter's inbound message has to be attributed to the
        // right workspace.
        let channels = ChannelService::new(store.clone(), bus.clone());

        for record in records {
            let Some(config) = store.integration_config(&record.id).await? else {
                continue;
            };
            let adapter = match registry.build(record.kind, &config) {
                Ok(Some(adapter)) => adapter,
                Ok(None) => {
                    warn!(
                        integration = %record.id,
                        kind = record.kind.as_str(),
                        "this build has no adapter for the integration kind"
                    );
                    started.unsupported.push(record.id.clone());
                    continue;
                }
                Err(error) => {
                    // A configuration that does not parse is the operator's to
                    // fix, and the message names the field.
                    warn!(
                        integration = %record.id,
                        kind = record.kind.as_str(),
                        %error,
                        "the integration configuration was refused"
                    );
                    continue;
                }
            };

            let sink = Arc::new(
                ChannelSink::new(store.clone(), channels.clone(), record.clone())
                    .with_turns(turns_for(store, executor.clone())),
            );
            started.running.push(cowbot_integrations::start(
                store.clone(),
                bus.clone(),
                record,
                adapter,
                sink,
            ));
        }
    }

    info!(
        running = started.running.len(),
        unsupported = started.unsupported.len(),
        "IM integrations started"
    );
    Ok(started)
}

/// A turn starter over the deployment's executor.
fn turns_for(store: &Store, executor: Arc<TurnExecutor>) -> Box<dyn TurnStart> {
    Box::new(ExecutorTurns {
        store: store.clone(),
        executor,
    })
}

/// An empty registry, so a test can assert that a build with no adapters reports
/// a stored integration as unsupported rather than starting nothing silently.
pub fn empty_registry() -> Registry {
    Registry::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::integration::{IntegrationKind, NewIntegration};
    use cowbot_service::bootstrap_local;

    async fn store_with_workspace() -> (Store, cowbot_core::identity::WorkspaceId) {
        let store = Store::connect_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let context = bootstrap_local(&store).await.unwrap();
        (store, context.workspace.id)
    }

    #[tokio::test]
    async fn an_integration_with_a_bad_configuration_is_reported_and_skipped() {
        // A configuration that does not parse is the operator's to fix. It must
        // not stop the server, and it must not be silently accepted either.
        let (store, workspace_id) = store_with_workspace().await;
        let new = NewIntegration::new(
            &workspace_id,
            IntegrationKind::Mattermost,
            "Support",
            // No bot_token: the adapter's own parser refuses it.
            r#"{"base_url":"https://chat.example.com"}"#,
        );
        store.create_integration(&new).await.unwrap();

        let executor = crate::TurnExecutor::new(
            store.clone(),
            EventBus::default_bus(),
            &crate::config::ServerLlmConfig {
                protocol: crate::config::ServerLlmProtocol::Mock,
                base_url: None,
                api_key: None,
                model: "mock".into(),
                context_window: None,
                max_output_tokens: None,
            },
            &tempfile::tempdir().unwrap().keep(),
        )
        .await
        .unwrap();

        let started = start_all(&store, &EventBus::default_bus(), executor)
            .await
            .unwrap();
        assert!(started.running.is_empty());
        assert!(
            started.unsupported.is_empty(),
            "a bad config is not unsupported"
        );
    }

    #[tokio::test]
    async fn a_revoked_integration_is_not_started() {
        let (store, workspace_id) = store_with_workspace().await;
        let new = NewIntegration::new(
            &workspace_id,
            IntegrationKind::Mattermost,
            "Support",
            r#"{"base_url":"https://chat.example.com","bot_token":"abc"}"#,
        );
        store.create_integration(&new).await.unwrap();
        store.revoke_integration(&new.id).await.unwrap();

        let executor = crate::TurnExecutor::new(
            store.clone(),
            EventBus::default_bus(),
            &crate::config::ServerLlmConfig {
                protocol: crate::config::ServerLlmProtocol::Mock,
                base_url: None,
                api_key: None,
                model: "mock".into(),
                context_window: None,
                max_output_tokens: None,
            },
            &tempfile::tempdir().unwrap().keep(),
        )
        .await
        .unwrap();

        let started = start_all(&store, &EventBus::default_bus(), executor)
            .await
            .unwrap();
        assert!(started.running.is_empty());
        started.stop_all();
    }

    #[tokio::test]
    async fn mattermost_is_a_supported_kind_in_this_build() {
        assert!(crate::adapters::registry()
            .kinds()
            .contains(&IntegrationKind::Mattermost));
        // The other three still have no adapter, which is what their rows report.
        assert!(!crate::adapters::registry()
            .kinds()
            .contains(&IntegrationKind::Telegram));
    }
}
