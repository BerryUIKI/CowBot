//! The whole path, through a fake adapter (ADR-0056).
//!
//! The unit tests cover each rule on its own. This drives the supervisor the way
//! a deployment would: an adapter that hands the pipeline what a platform would,
//! and a bus that a channel message arrives on, with the mirror posting it back.

use std::sync::{Arc, Mutex};

use cowbot_bus::EventBus;
use cowbot_core::agent::AgentId;
use cowbot_core::cancel::CancelSignal;
use cowbot_core::channel::{Channel, ChannelId, Sender};
use cowbot_core::error::CowResult;

use cowbot_core::identity::Workspace;
use cowbot_core::integration::{
    IntegrationBinding, IntegrationKind, IntegrationRecord, NewIntegration,
};
use cowbot_integrations::adapter::{Integration, IntegrationContext, Registry};
use cowbot_integrations::model::{
    ChunkPart, DeliveredId, ExternalAuthor, InboundMessage, Ingested, OutboundContent,
    OutboundTarget, Skipped,
};
use cowbot_integrations::sink::ChannelSink;
use cowbot_integrations::MessageSink;
use cowbot_integrations::{start, start_workspace};
use cowbot_service::ChannelService;
use cowbot_store::Store;

/// A platform that does not exist: it records what it was asked to post and
/// hands the pipeline whatever messages a test gives it.
#[derive(Clone)]
struct FakeAdapter {
    delivered: Arc<Mutex<Vec<(OutboundTarget, OutboundContent)>>>,
    outbound: Arc<Mutex<Vec<InboundMessage>>>,
}

impl FakeAdapter {
    fn new() -> Self {
        Self {
            delivered: Arc::default(),
            outbound: Arc::default(),
        }
    }
}

impl std::fmt::Debug for FakeAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FakeAdapter")
            .field("delivered", &self.delivered.lock().unwrap().len())
            .finish()
    }
}

#[async_trait::async_trait]
impl Integration for FakeAdapter {
    fn kind(&self) -> IntegrationKind {
        IntegrationKind::Mattermost
    }

    async fn run(&self, ctx: IntegrationContext, cancel: CancelSignal) -> CowResult<()> {
        // Deliver everything the test queued, then wait to be stopped, which is
        // what a real inbound loop does between messages.
        let pending = self.outbound.lock().unwrap().clone();
        for message in pending {
            ctx.sink.accept(message).await?;
        }
        cancel.cancelled().await;
        Ok(())
    }

    async fn deliver(
        &self,
        target: &OutboundTarget,
        content: &OutboundContent,
    ) -> CowResult<DeliveredId> {
        self.delivered
            .lock()
            .unwrap()
            .push((target.clone(), content.clone()));
        Ok(DeliveredId(format!("post-{}", content.part.index)))
    }
}

struct Fixture {
    store: Store,
    bus: EventBus,
    workspace: Workspace,
    integration: IntegrationRecord,
    channel: Channel,
}

async fn fixture() -> Fixture {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let workspace = Workspace::new("Team");
    store.create_workspace(&workspace).await.unwrap();
    let channel = Channel::new("support", "");
    store.create_channel(&workspace.id, &channel).await.unwrap();

    let new = NewIntegration::new(
        &workspace.id,
        IntegrationKind::Mattermost,
        "Support bot",
        "{}",
    );
    store.create_integration(&new).await.unwrap();
    store
        .bind_integration_channel(&IntegrationBinding {
            integration_id: new.id.clone(),
            external_id: "town-square".into(),
            channel_id: channel.id.clone(),
            agent_id: None,
        })
        .await
        .unwrap();
    let integration = store.get_integration(&new.id).await.unwrap().unwrap();

    Fixture {
        store,
        bus: EventBus::default_bus(),
        workspace,
        integration,
        channel,
    }
}

fn inbound(event_id: &str, text: &str) -> InboundMessage {
    InboundMessage {
        event_id: event_id.into(),
        external_conversation_id: "town-square".into(),
        external_thread_id: None,
        author: ExternalAuthor {
            external_id: "mm-user-1".into(),
            display_name: "Grace".into(),
            is_bot: false,
        },
        text: text.into(),
        addresses_bot: true,
        created_at: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn an_inbound_message_becomes_a_channel_message_from_the_person_in_the_im() {
    let fixture = fixture().await;
    let adapter = FakeAdapter::new();
    adapter
        .outbound
        .lock()
        .unwrap()
        .push(inbound("evt-1", "What is the deploy status?"));

    let sink = Arc::new(ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    ));
    let mut registry = Registry::new();
    let adapter_handle = adapter.clone();
    registry.register(
        IntegrationKind::Mattermost,
        Arc::new(move |_config| Ok(Arc::new(adapter_handle.clone()) as Arc<dyn Integration>)),
    );

    let started = start_workspace(
        &fixture.store,
        &fixture.bus,
        &registry,
        sink,
        &fixture.workspace.id,
    )
    .await
    .unwrap();
    assert_eq!(started.running.len(), 1);

    // Wait for the inbound loop to store the message.
    wait_for_messages(&fixture.store, &fixture.channel.id, 1).await;

    let messages = fixture
        .store
        .list_messages(&fixture.channel.id, 10)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "What is the deploy status?");
    let Sender::User(user_id) = &messages[0].sender else {
        panic!("expected a user sender, got {:?}", messages[0].sender);
    };

    // The provisioned user is the IM person, is a member, and cannot sign in.
    let user = fixture
        .store
        .get_user(&cowbot_core::identity::UserId(user_id.clone()))
        .await
        .unwrap()
        .expect("the sender is a real user row");
    assert_eq!(user.name, "Grace");
    assert!(user.password_hash.is_empty());
    assert!(fixture
        .store
        .get_membership(&user.id, &fixture.workspace.id)
        .await
        .unwrap()
        .is_some());

    started.stop_all();
}

#[tokio::test]
async fn an_agent_reply_is_posted_back_to_the_conversation_it_came_from() {
    let fixture = fixture().await;
    let adapter = FakeAdapter::new();
    let mut registry = Registry::new();
    let handle = adapter.clone();
    registry.register(
        IntegrationKind::Mattermost,
        Arc::new(move |_config| Ok(Arc::new(handle.clone()) as Arc<dyn Integration>)),
    );
    let sink = Arc::new(ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    ));

    let started = start_workspace(
        &fixture.store,
        &fixture.bus,
        &registry,
        sink,
        &fixture.workspace.id,
    )
    .await
    .unwrap();

    // An agent answers in the bound channel.
    ChannelService::new(fixture.store.clone(), fixture.bus.clone())
        .post_message(
            &fixture.channel.id,
            Sender::Agent(AgentId("agent-1".into())),
            "Deploy is green.",
        )
        .await
        .unwrap();
    wait_for_deliveries(&adapter, 1).await;

    let delivered = adapter.delivered.lock().unwrap().clone();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0.external_conversation_id, "town-square");
    assert_eq!(delivered[0].1.text, "Deploy is green.");
    assert_eq!(delivered[0].1.part, ChunkPart { index: 0, total: 1 });

    started.stop_all();
}

#[tokio::test]
async fn a_users_own_message_is_not_echoed_back_into_the_im() {
    let fixture = fixture().await;
    let adapter = FakeAdapter::new();
    let mut registry = Registry::new();
    let handle = adapter.clone();
    registry.register(
        IntegrationKind::Mattermost,
        Arc::new(move |_config| Ok(Arc::new(handle.clone()) as Arc<dyn Integration>)),
    );
    let sink = Arc::new(ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    ));
    let started = start_workspace(
        &fixture.store,
        &fixture.bus,
        &registry,
        sink,
        &fixture.workspace.id,
    )
    .await
    .unwrap();

    // A user writes in CowBot itself: the IM already shows their own typing only
    // if they typed there, so this must not be mirrored.
    ChannelService::new(fixture.store.clone(), fixture.bus.clone())
        .post_message(
            &fixture.channel.id,
            Sender::User("someone".into()),
            "I typed this in CowBot.",
        )
        .await
        .unwrap();

    // An agent message afterwards proves the mirror is running and the user
    // message was skipped rather than merely slow.
    ChannelService::new(fixture.store.clone(), fixture.bus.clone())
        .post_message(
            &fixture.channel.id,
            Sender::Agent(AgentId("agent-1".into())),
            "Understood.",
        )
        .await
        .unwrap();
    wait_for_deliveries(&adapter, 1).await;

    let delivered = adapter.delivered.lock().unwrap().clone();
    assert_eq!(delivered.len(), 1, "{delivered:?}");
    assert_eq!(delivered[0].1.text, "Understood.");

    started.stop_all();
}

#[tokio::test]
async fn a_long_agent_answer_arrives_as_ordered_chunks() {
    let fixture = fixture().await;
    let adapter = FakeAdapter::new();
    let mut registry = Registry::new();
    let handle = adapter.clone();
    registry.register(
        IntegrationKind::Mattermost,
        Arc::new(move |_config| Ok(Arc::new(handle.clone()) as Arc<dyn Integration>)),
    );
    let sink = Arc::new(ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    ));
    let started = start_workspace(
        &fixture.store,
        &fixture.bus,
        &registry,
        sink,
        &fixture.workspace.id,
    )
    .await
    .unwrap();

    let long = format!("{}\n\n{}", "a".repeat(3_000), "b".repeat(3_000));
    ChannelService::new(fixture.store.clone(), fixture.bus.clone())
        .post_message(
            &fixture.channel.id,
            Sender::Agent(AgentId("agent-1".into())),
            long.clone(),
        )
        .await
        .unwrap();
    wait_for_deliveries(&adapter, 2).await;

    let delivered = adapter.delivered.lock().unwrap().clone();
    assert_eq!(delivered.len(), 2);
    assert_eq!(delivered[0].1.part, ChunkPart { index: 0, total: 2 });
    assert_eq!(delivered[1].1.part, ChunkPart { index: 1, total: 2 });
    assert_eq!(
        format!("{}\n\n{}", delivered[0].1.text, delivered[1].1.text),
        long
    );

    started.stop_all();
}

/// A platform that counts UTF-16 code units, the way Telegram does.
struct UnitCounted(std::sync::Arc<FakeAdapter>);

#[async_trait::async_trait]
impl cowbot_integrations::Integration for UnitCounted {
    fn kind(&self) -> cowbot_core::integration::IntegrationKind {
        cowbot_core::integration::IntegrationKind::Telegram
    }

    fn chunk_limit(&self) -> cowbot_integrations::ChunkLimit {
        cowbot_integrations::ChunkLimit::Units(4_096)
    }

    async fn run(
        &self,
        _ctx: cowbot_integrations::IntegrationContext,
        cancel: cowbot_core::cancel::CancelSignal,
    ) -> cowbot_core::error::CowResult<()> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn deliver(
        &self,
        target: &cowbot_integrations::OutboundTarget,
        content: &cowbot_integrations::OutboundContent,
    ) -> cowbot_core::error::CowResult<cowbot_integrations::DeliveredId> {
        self.0.deliver(target, content).await
    }
}

#[tokio::test]
async fn a_mirrored_answer_is_split_by_the_unit_limit_its_platform_reports() {
    // The wiring that makes the unit-counted split reachable. Without it an
    // emoji-heavy answer is twice as long to Telegram as the mirror thinks.
    let fixture = fixture().await;
    let adapter = std::sync::Arc::new(FakeAdapter::new());
    let sink = std::sync::Arc::new(cowbot_integrations::ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    ));
    let mut registry = Registry::new();
    let counted = std::sync::Arc::new(UnitCounted(adapter.clone()));
    registry.register(
        IntegrationKind::Mattermost,
        Arc::new(move |_config| Ok(counted.clone() as Arc<dyn Integration>)),
    );
    let started = start_workspace(
        &fixture.store,
        &fixture.bus,
        &registry,
        sink,
        &fixture.workspace.id,
    )
    .await
    .unwrap();

    // 3 000 emoji: 6 000 UTF-16 units, well over Telegram's 4 096, and far
    // under 4 000 *characters* — so a character budget would send it whole.
    ChannelService::new(fixture.store.clone(), fixture.bus.clone())
        .post_message(
            &fixture.channel.id,
            Sender::Agent(AgentId("agent-1".into())),
            "🚀".repeat(3_000),
        )
        .await
        .unwrap();
    wait_for_deliveries(&adapter, 2).await;

    let delivered = adapter.delivered.lock().unwrap().clone();
    assert_eq!(delivered.len(), 2, "the answer was not split");
    for (_, content) in &delivered {
        assert!(content.text.encode_utf16().count() <= 4_096);
    }

    started.stop_all();
}

#[tokio::test]
async fn a_replayed_delivery_starts_one_turn_not_two() {
    let fixture = fixture().await;
    let adapter = FakeAdapter::new();
    // The platform sent the same event twice, as Slack does on a timeout.
    {
        let mut queue = adapter.outbound.lock().unwrap();
        queue.push(inbound("evt-1", "Deploy status?"));
        queue.push(inbound("evt-1", "Deploy status?"));
    }
    let sink = Arc::new(ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    ));
    let mut registry = Registry::new();
    let handle = adapter.clone();
    registry.register(
        IntegrationKind::Mattermost,
        Arc::new(move |_config| Ok(Arc::new(handle.clone()) as Arc<dyn Integration>)),
    );

    let started = start_workspace(
        &fixture.store,
        &fixture.bus,
        &registry,
        sink,
        &fixture.workspace.id,
    )
    .await
    .unwrap();
    wait_for_messages(&fixture.store, &fixture.channel.id, 1).await;
    // Give the second delivery a chance to be (wrongly) stored.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(
        fixture
            .store
            .list_messages(&fixture.channel.id, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    started.stop_all();
}

#[tokio::test]
async fn a_sink_starts_a_turn_only_for_a_message_that_addresses_the_bot() {
    let fixture = fixture().await;
    let turns = cowbot_integrations::RecordingTurns::new();
    let sink = ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    )
    .with_turns(Box::new(turns.clone()));

    let outcome = sink
        .accept(inbound("evt-1", "Deploy status?"))
        .await
        .unwrap();
    assert!(outcome.is_posted());
    assert_eq!(turns.started().len(), 1);
    assert_eq!(turns.started()[0].0, fixture.channel.id);

    // The same event again is a duplicate, and starts nothing.
    let again = sink
        .accept(inbound("evt-1", "Deploy status?"))
        .await
        .unwrap();
    assert_eq!(again, Ingested::Skipped(Skipped::Duplicate));
    assert_eq!(turns.started().len(), 1);
}

#[tokio::test]
async fn a_build_without_an_adapter_reports_the_integration_it_cannot_run() {
    // The row can exist before the build that runs it, and the other
    // integrations must still start.
    let fixture = fixture().await;
    let registry = Registry::new();
    let sink = Arc::new(ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    ));
    let started = start_workspace(
        &fixture.store,
        &fixture.bus,
        &registry,
        sink,
        &fixture.workspace.id,
    )
    .await
    .unwrap();
    assert!(started.running.is_empty());
    assert_eq!(started.unsupported, vec![fixture.integration.id.clone()]);
}

#[tokio::test]
async fn a_revoked_integration_is_not_started() {
    let fixture = fixture().await;
    fixture
        .store
        .revoke_integration(&fixture.integration.id)
        .await
        .unwrap();
    let registry = Registry::new();
    let sink = Arc::new(ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    ));
    let started = start_workspace(
        &fixture.store,
        &fixture.bus,
        &registry,
        sink,
        &fixture.workspace.id,
    )
    .await
    .unwrap();
    assert!(started.is_empty());
}

#[tokio::test]
async fn a_stopped_integration_stops_both_of_its_tasks() {
    let fixture = fixture().await;
    let adapter = FakeAdapter::new();
    let sink = Arc::new(ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    ));
    let running = start(
        fixture.store.clone(),
        fixture.bus.clone(),
        fixture.integration.clone(),
        Arc::new(adapter.clone()),
        sink,
    );
    assert!(!running.is_stopped());
    running.stop();
    assert!(running.is_stopped());

    // A message posted after the stop is not mirrored.
    ChannelService::new(fixture.store.clone(), fixture.bus.clone())
        .post_message(
            &fixture.channel.id,
            Sender::Agent(AgentId("agent-1".into())),
            "After the stop.",
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(adapter.delivered.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_message_posted_the_moment_start_returns_is_still_mirrored() {
    // The mirror subscribes synchronously in `start`. Subscribing inside its
    // task would leave a window where a reply posted right after `start`
    // returned is never delivered, because the subscription did not exist yet.
    let fixture = fixture().await;
    let adapter = FakeAdapter::new();
    let sink = Arc::new(ChannelSink::new(
        fixture.store.clone(),
        ChannelService::new(fixture.store.clone(), fixture.bus.clone()),
        fixture.integration.clone(),
    ));
    let running = start(
        fixture.store.clone(),
        fixture.bus.clone(),
        fixture.integration.clone(),
        Arc::new(adapter.clone()),
        sink,
    );

    ChannelService::new(fixture.store.clone(), fixture.bus.clone())
        .post_message(
            &fixture.channel.id,
            Sender::Agent(AgentId("agent-1".into())),
            "Immediately after start.",
        )
        .await
        .unwrap();

    wait_for_deliveries(&adapter, 1).await;
    assert_eq!(
        adapter.delivered.lock().unwrap()[0].1.text,
        "Immediately after start."
    );
    running.stop();
}

// -- helpers ---------------------------------------------------------------

async fn wait_for_messages(store: &Store, channel: &ChannelId, count: usize) {
    for _ in 0..100 {
        if store.list_messages(channel, 20).await.unwrap().len() >= count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("expected {count} message(s) in the channel");
}

async fn wait_for_deliveries(adapter: &FakeAdapter, count: usize) {
    for _ in 0..100 {
        if adapter.delivered.lock().unwrap().len() >= count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!(
        "expected {count} delivery/deliveries, got {}",
        adapter.delivered.lock().unwrap().len()
    );
}
