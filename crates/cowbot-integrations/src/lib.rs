//! IM integrations: the seam between an IM platform and a CowBot channel
//! (ADR-0056).
//!
//! Nothing here talks to a platform. This crate is the part every adapter shares:
//!
//! * [`adapter`] — the [`Integration`](adapter::Integration) contract and the
//!   registry a deployment fills with the platforms it can start;
//! * [`inbound`] — one platform delivery into one CowBot message, including the
//!   dedupe and the mention translation;
//! * [`outbound`] — one channel message back into the IM, including chunking to a
//!   platform's message limit;
//! * [`principal`] — a platform account as a CowBot user;
//! * [`notice`] — starting every binding check and saying, in the CowBot channel,
//!   which ones cannot receive;
//! * [`approvals`] — an approval request as a question for a person in the IM,
//!   and their answer attributed to them rather than to the bot (ADR-0060);
//! * [`attachments`] — a file somebody sent, on disk where a tool can reach it;
//! * [`supervisor`] — running an adapter's inbound loop beside its mirror.
//!
//! An adapter that prefers an inbound HTTP surface to a socket overrides
//! [`adapter::Integration::webhook`]; the route that calls it lives in the server
//! (ADR-0062).
//!
//! The four platform crates (S2–S6) implement [`adapter::Integration`] and
//! nothing else. Splitting it this way means the rules that are easy to get
//! wrong — a duplicate that runs a second turn, an echo loop between two bots, a
//! reply addressed to the wrong room — are written once and tested once.

pub mod adapter;
pub mod approvals;
pub mod attachments;
pub mod inbound;
pub mod model;
pub mod notice;
pub mod outbound;
pub mod principal;
pub mod route;
pub mod sink;
pub mod supervisor;

pub use adapter::{
    Factory, InboundRequest, Integration, IntegrationContext, MessageSink, Registry, WebhookReply,
};
pub use approvals::{ApprovalPosting, ApprovalPrompt, Decided, Refused};
pub use model::{
    Attachment, AttachmentKind, BindingHealth, ChunkLimit, ChunkPart, DeliveredId, ExternalAuthor,
    InboundMessage, Ingested, OutboundContent, OutboundTarget, Route, Skipped,
};
pub use notice::report_binding_health;
pub use outbound::{build_posts_with, chunk, chunk_units, label_chunk};
pub use principal::{resolve_principal, Principal};
pub use route::resolve_route;
pub use sink::{ChannelSink, RecordingTurns, TurnStart};
pub use supervisor::{start, start_workspace, DiscardingSink, Running, Started};

#[cfg(test)]
pub(crate) mod tests {
    //! A workspace, a channel, and a bound integration, built the same way for
    //! every test in this crate.

    use cowbot_bus::EventBus;
    use cowbot_core::agent::{AgentId, AgentProfile};
    use cowbot_core::channel::Channel;
    use cowbot_core::identity::WorkspaceId;
    use cowbot_core::integration::{
        IntegrationBinding, IntegrationKind, IntegrationRecord, NewIntegration,
    };
    use cowbot_service::ChannelService;
    use cowbot_store::Store;

    pub(crate) mod fixture {
        use super::*;
        use crate::model::{ExternalAuthor, InboundMessage};

        pub(crate) struct Fixture {
            pub store: Store,
            pub workspace_id: WorkspaceId,
            pub integration: IntegrationRecord,
        }

        /// An in-memory workspace with one direct channel `c1`, one group
        /// channel `c-group`, and an integration bound to `c1`.
        pub(crate) async fn in_memory() -> Fixture {
            let store = Store::connect_in_memory().await.unwrap();
            store.migrate().await.unwrap();
            let workspace = cowbot_core::identity::Workspace::new("Team");
            store.create_workspace(&workspace).await.unwrap();
            let workspace_id = workspace.id.clone();

            let mut direct = Channel::new("direct", "");
            direct.id = cowbot_core::channel::ChannelId::new("c1");
            store.create_channel(&workspace_id, &direct).await.unwrap();
            let mut group = Channel::new("c-group", "");
            group.id = cowbot_core::channel::ChannelId::new("c-group");
            store.create_channel(&workspace_id, &group).await.unwrap();

            let new = NewIntegration::new(&workspace_id, IntegrationKind::Slack, "Support", "{}");
            store.create_integration(&new).await.unwrap();
            store
                .bind_integration_channel(&IntegrationBinding {
                    integration_id: new.id.clone(),
                    external_id: "C024".into(),
                    channel_id: direct.id.clone(),
                    agent_id: None,
                })
                .await
                .unwrap();
            let integration = store.get_integration(&new.id).await.unwrap().unwrap();

            Fixture {
                store,
                workspace_id,
                integration,
            }
        }

        pub(crate) fn bus() -> EventBus {
            EventBus::default_bus()
        }

        impl Fixture {
            pub(crate) fn channels(&self) -> ChannelService {
                ChannelService::new(self.store.clone(), bus())
            }

            pub(crate) async fn add_agent(&self, name: &str) -> AgentId {
                let agent = AgentProfile::new(name, "assistant", "answer", "a test agent");
                self.store
                    .upsert_agent(&self.workspace_id, &agent)
                    .await
                    .unwrap();
                agent.id
            }

            pub(crate) async fn group_channel(&self) -> cowbot_core::channel::ChannelId {
                cowbot_core::channel::ChannelId::new("c-group")
            }
        }

        /// A message from one person in the bound conversation.
        pub(crate) fn sample(event_id: &str, text: &str, addresses_bot: bool) -> InboundMessage {
            InboundMessage {
                event_id: event_id.into(),
                external_conversation_id: "C024".into(),
                external_thread_id: None,
                author: ExternalAuthor {
                    external_id: "U024".into(),
                    display_name: "Ada".into(),
                    is_bot: false,
                },
                text: text.into(),
                addresses_bot,
                attachments: Vec::new(),
                created_at: chrono::Utc::now(),
            }
        }
    }
}
