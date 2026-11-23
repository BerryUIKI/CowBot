//! The inbound pipeline: one platform delivery into one CowBot message
//! (ADR-0056).
//!
//! ```text
//! platform event
//!   → verify (the adapter's job, before this runs)
//!   → drop if the author is a bot          ← else two bots answer each other
//!   → resolve the binding                  ← else there is nowhere to put it
//!   → claim the platform event id          ← else a retry runs a second turn
//!   → resolve the author to a CowBot user
//!   → turn "the bot was mentioned" into "@AgentName"
//!   → post the message as Sender::User
//! ```
//!
//! Two orderings in that list are load-bearing. The claim happens *before* the
//! post, so a retry that arrives while the first copy is still posting is
//! dropped rather than duplicated; the cost is that a crash between the claim
//! and the post loses that one delivery, which is the better failure because the
//! platform will not retry a delivery it saw acknowledged. And the binding is
//! resolved before the claim, so a conversation nobody bound costs no dedupe
//! rows — a bot mentioned in a hundred rooms it does not serve must not fill the
//! table.

use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::channel::Sender;
use cowbot_core::error::CowResult;
use cowbot_core::integration::IntegrationRecord;
use cowbot_service::ChannelService;
use cowbot_store::Store;
use tracing::debug;

use crate::model::{InboundMessage, Ingested, Skipped};
use crate::principal::resolve_principal;
use crate::route::resolve_route;

/// Store one platform delivery as a CowBot channel message.
///
/// Returns what happened, including the ordinary reasons nothing was stored. An
/// adapter logs the outcome and acknowledges the platform; it never has to
/// decide what a duplicate means.
pub async fn ingest(
    store: &Store,
    channels: &ChannelService,
    integration: &IntegrationRecord,
    message: &InboundMessage,
    attachments: Option<&crate::attachments::Inbound<'_>>,
) -> CowResult<Ingested> {
    // A bot, including this one, must not be answered. A platform delivers
    // another bot's message happily, and two bots mentioning each other is an
    // unbounded loop that costs real money.
    if message.author.is_bot {
        return Ok(Ingested::Skipped(Skipped::FromBot));
    }

    let Some(route) = resolve_route(store, integration, &message.external_conversation_id).await?
    else {
        return Ok(Ingested::Skipped(Skipped::Unbound));
    };

    // A binding can outlive the channel it points at: a channel is deleted
    // without the integration hearing about it.
    let Some(workspace_id) = store.channel_workspace(&route.channel_id).await? else {
        return Ok(Ingested::Skipped(Skipped::ChannelGone));
    };
    if workspace_id != integration.workspace_id {
        // A channel moved between workspaces (an import re-keys ids, so this is
        // rare) leaves a binding that would write into another team's data.
        return Ok(Ingested::Skipped(Skipped::ChannelGone));
    }

    // Claim before posting: a retry that lands while the first copy is still
    // posting is dropped here instead of appearing twice in the timeline.
    if !store
        .claim_integration_event(&integration.id, &message.event_id)
        .await?
    {
        return Ok(Ingested::Skipped(Skipped::Duplicate));
    }

    // The attachments are fetched here, after the claim, so a platform retry
    // downloads nothing (ADR-0060). A file that cannot be stored is named as not
    // kept rather than dropping the message.
    let files = crate::attachments::store_all(message, attachments).await;

    let principal = resolve_principal(store, integration, &message.author).await?;
    let mut content = attribute_mention(store, route.agent_id.as_ref(), message).await?;
    if let Some(files) = files {
        content.push_str(&files);
    }
    // The check runs on the composed text: a photo sent with no caption reaches
    // the agent as the path it was written to, and dropping it as empty would
    // lose the only thing the sender meant to say.
    if content.trim().is_empty() {
        // Nothing worth storing. The claim is released so the platform's retry
        // is not swallowed: this delivery was declined, not handled.
        store
            .release_integration_event(&integration.id, &message.event_id)
            .await?;
        return Ok(Ingested::Skipped(Skipped::Empty));
    }

    let posted = match channels
        .post_thread_message(
            &route.channel_id,
            Sender::User(principal.user_id.0.clone()),
            content,
            message.external_thread_id.clone(),
        )
        .await
    {
        Ok(posted) => posted,
        Err(error) => {
            // The message was never stored, so a retry can still succeed.
            store
                .release_integration_event(&integration.id, &message.event_id)
                .await?;
            return Err(error);
        }
    };

    // A message starts a turn only when the platform said it addresses the bot.
    // The binding's agent does not widen that: in a busy group room it names
    // *which* agent the bot is, so a mention of the bot can become `@ThatAgent`,
    // not a reason to answer every message in the room.
    let starts_turn = message.addresses_bot;
    debug!(
        integration = %integration.id,
        external = %message.external_conversation_id,
        message = %posted.id,
        starts_turn,
        "Stored an inbound IM message"
    );

    Ok(Ingested::Posted {
        channel_id: route.channel_id,
        message_id: posted.id,
        user_id: principal.user_id.0,
        starts_turn,
    })
}

/// Turn "the bot was mentioned" into a CowBot `@AgentName`.
///
/// `parse_mentions` reads `@` + a name, so a platform mention of the bot has to
/// become the address the binding's agent answers to. Everything else is left
/// exactly as the adapter translated it: only the bot itself is guaranteed to
/// have a CowBot name, because only the bot is bound.
///
/// A message that already names the agent is left alone, so an IM user who typed
/// `@CowBot` does not become `@CowBot @CowBot`.
async fn attribute_mention(
    store: &Store,
    bound_agent: Option<&AgentId>,
    message: &InboundMessage,
) -> CowResult<String> {
    let text = message.text.trim();
    let Some(agent_id) = bound_agent else {
        // Nothing is forced by the binding, so the text is stored as written:
        // this is a direct conversation (which answers everything) or a group
        // channel whose users type CowBot names themselves.
        return Ok(text.to_string());
    };
    if !message.addresses_bot {
        return Ok(text.to_string());
    }

    let Some(agent) = store.get_agent(agent_id).await? else {
        // The binding names an agent that no longer exists. Storing the text
        // unchanged still records what was said, and the turn then produces the
        // same "no agent matched" notice a mistyped mention does.
        return Ok(text.to_string());
    };

    if already_mentions(text, &agent) {
        return Ok(text.to_string());
    }
    Ok(format!("@{} {text}", agent.name))
}

/// Whether `text` already addresses `agent` by name or id.
fn already_mentions(text: &str, agent: &AgentProfile) -> bool {
    cowbot_core::mention::resolve_mentions(text, std::slice::from_ref(agent))
        .agents
        .iter()
        .any(|matched| matched.id == agent.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExternalAuthor;
    use crate::tests::fixture::{in_memory, sample};
    use cowbot_core::channel::ChannelId;
    use cowbot_core::integration::IntegrationBinding;

    #[tokio::test]
    async fn a_first_message_is_stored_as_the_person_who_wrote_it() {
        let fixture = in_memory().await;
        let channels = fixture.channels();

        let outcome = ingest(
            &fixture.store,
            &channels,
            &fixture.integration,
            &sample("evt-1", "Hello there", true),
            None,
        )
        .await
        .unwrap();

        let Ingested::Posted {
            channel_id,
            message_id,
            user_id,
            starts_turn,
        } = outcome
        else {
            panic!("expected a posted message, got {outcome:?}");
        };
        assert!(starts_turn);

        let messages = fixture.store.list_messages(&channel_id, 10).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, message_id);
        assert_eq!(messages[0].sender, Sender::User(user_id));
        assert_eq!(messages[0].content, "Hello there");
    }

    #[tokio::test]
    async fn a_replayed_event_id_stores_nothing_the_second_time() {
        let fixture = in_memory().await;
        let channels = fixture.channels();
        let message = sample("evt-1", "Hello", true);

        assert!(ingest(
            &fixture.store,
            &channels,
            &fixture.integration,
            &message,
            None,
        )
        .await
        .unwrap()
        .is_posted());
        assert_eq!(
            ingest(
                &fixture.store,
                &channels,
                &fixture.integration,
                &message,
                None,
            )
            .await
            .unwrap(),
            Ingested::Skipped(Skipped::Duplicate)
        );

        assert_eq!(
            fixture
                .store
                .list_messages(&ChannelId::new("c1"), 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_second_bots_message_is_dropped_rather_than_answered() {
        let fixture = in_memory().await;
        let channels = fixture.channels();
        let mut message = sample("evt-1", "Deploy finished", true);
        message.author = ExternalAuthor {
            is_bot: true,
            ..message.author
        };

        assert_eq!(
            ingest(
                &fixture.store,
                &channels,
                &fixture.integration,
                &message,
                None,
            )
            .await
            .unwrap(),
            Ingested::Skipped(Skipped::FromBot)
        );
        // Nothing was claimed either, so a human event with that id still works.
        assert!(!fixture
            .store
            .integration_event_seen(&fixture.integration.id, "evt-1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn an_unbound_conversation_is_ignored_and_claims_no_dedupe_row() {
        let fixture = in_memory().await;
        let channels = fixture.channels();
        let mut message = sample("evt-1", "Anyone there?", true);
        message.external_conversation_id = "C-unknown".into();

        assert_eq!(
            ingest(
                &fixture.store,
                &channels,
                &fixture.integration,
                &message,
                None,
            )
            .await
            .unwrap(),
            Ingested::Skipped(Skipped::Unbound)
        );
        assert!(!fixture
            .store
            .integration_event_seen(&fixture.integration.id, "evt-1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn a_binding_whose_channel_was_deleted_is_gone_rather_than_a_failure() {
        // The binding cascades with the channel, so the delivery reads as
        // unbound. The point of the test is that nothing is stored, nothing is
        // claimed, and nothing reports an error an operator has to act on.
        let fixture = in_memory().await;
        let channels = fixture.channels();
        fixture
            .store
            .delete_channel(&ChannelId::new("c1"))
            .await
            .unwrap();

        let outcome = ingest(
            &fixture.store,
            &channels,
            &fixture.integration,
            &sample("evt-1", "Hello", true),
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, Ingested::Skipped(Skipped::Unbound));
        assert!(!fixture
            .store
            .integration_event_seen(&fixture.integration.id, "evt-1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn a_binding_into_another_workspace_is_refused() {
        // A channel that moved workspaces leaves a binding pointing across a
        // tenant boundary. Writing into it would put one team's question in
        // another team's channel.
        let fixture = in_memory().await;
        let channels = fixture.channels();
        let other = cowbot_core::identity::Workspace::new("Other team");
        fixture.store.create_workspace(&other).await.unwrap();
        fixture
            .store
            .move_channel_to_workspace(&ChannelId::new("c1"), &other.id)
            .await
            .unwrap();

        assert_eq!(
            ingest(
                &fixture.store,
                &channels,
                &fixture.integration,
                &sample("evt-1", "Hello", true),
                None,
            )
            .await
            .unwrap(),
            Ingested::Skipped(Skipped::ChannelGone)
        );
    }

    #[tokio::test]
    async fn an_empty_message_releases_its_claim_so_a_retry_can_arrive() {
        let fixture = in_memory().await;
        let channels = fixture.channels();

        assert_eq!(
            ingest(
                &fixture.store,
                &channels,
                &fixture.integration,
                &sample("evt-1", "   ", true),
                None,
            )
            .await
            .unwrap(),
            Ingested::Skipped(Skipped::Empty)
        );
        assert!(!fixture
            .store
            .integration_event_seen(&fixture.integration.id, "evt-1")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn a_forum_topic_is_kept_as_the_thread() {
        let fixture = in_memory().await;
        let channels = fixture.channels();
        let mut message = sample("evt-1", "Hello", true);
        message.external_thread_id = Some("topic-9".into());

        ingest(
            &fixture.store,
            &channels,
            &fixture.integration,
            &message,
            None,
        )
        .await
        .unwrap();

        let messages = fixture
            .store
            .list_messages(&ChannelId::new("c1"), 10)
            .await
            .unwrap();
        assert_eq!(messages[0].thread_id.as_deref(), Some("topic-9"));
    }

    #[tokio::test]
    async fn a_mentioned_bot_is_translated_into_the_agents_cowbot_name() {
        // A group channel needs the mention: `parse_mentions` reads `@Name`, and
        // the IM platform's mention of the bot is not that.
        let fixture = in_memory().await;
        let channels = fixture.channels();
        let agent = fixture.add_agent("CowBot").await;
        fixture
            .store
            .bind_integration_channel(&IntegrationBinding {
                integration_id: fixture.integration.id.clone(),
                external_id: "C024".into(),
                channel_id: fixture.group_channel().await,
                agent_id: Some(agent.clone()),
            })
            .await
            .unwrap();

        ingest(
            &fixture.store,
            &channels,
            &fixture.integration,
            &sample("evt-1", "what is the deploy status?", true),
            None,
        )
        .await
        .unwrap();

        let messages = fixture
            .store
            .list_messages(&ChannelId::new("c-group"), 10)
            .await
            .unwrap();
        assert_eq!(messages[0].content, "@CowBot what is the deploy status?");
        assert_eq!(messages[0].channel_id, ChannelId::new("c-group"));
    }

    #[tokio::test]
    async fn a_message_that_already_names_the_agent_is_not_addressed_twice() {
        let fixture = in_memory().await;
        let channels = fixture.channels();
        let agent = fixture.add_agent("CowBot").await;
        fixture
            .store
            .bind_integration_channel(&IntegrationBinding {
                integration_id: fixture.integration.id.clone(),
                external_id: "C024".into(),
                channel_id: fixture.group_channel().await,
                agent_id: Some(agent),
            })
            .await
            .unwrap();

        ingest(
            &fixture.store,
            &channels,
            &fixture.integration,
            &sample("evt-1", "@cowbot deploy status?", true),
            None,
        )
        .await
        .unwrap();

        let messages = fixture
            .store
            .list_messages(&ChannelId::new("c-group"), 10)
            .await
            .unwrap();
        assert_eq!(messages[0].content, "@cowbot deploy status?");
    }

    #[tokio::test]
    async fn a_group_message_that_does_not_address_the_bot_is_stored_but_starts_no_turn() {
        let fixture = in_memory().await;
        let channels = fixture.channels();
        let agent = fixture.add_agent("CowBot").await;
        fixture
            .store
            .bind_integration_channel(&IntegrationBinding {
                integration_id: fixture.integration.id.clone(),
                external_id: "C024".into(),
                channel_id: fixture.group_channel().await,
                agent_id: Some(agent),
            })
            .await
            .unwrap();

        let outcome = ingest(
            &fixture.store,
            &channels,
            &fixture.integration,
            &sample("evt-1", "lunch at noon?", false),
            None,
        )
        .await
        .unwrap();

        let Ingested::Posted { starts_turn, .. } = outcome else {
            panic!("expected a posted message, got {outcome:?}");
        };
        assert!(!starts_turn);
    }

    #[tokio::test]
    async fn a_direct_conversation_starts_a_turn_without_a_mention() {
        let fixture = in_memory().await;
        let channels = fixture.channels();
        let outcome = ingest(
            &fixture.store,
            &channels,
            &fixture.integration,
            &sample("evt-1", "hi", true),
            None,
        )
        .await
        .unwrap();
        let Ingested::Posted { starts_turn, .. } = outcome else {
            panic!("expected a posted message, got {outcome:?}");
        };
        assert!(starts_turn);
    }
}
