//! Telling an operator that a binding cannot receive (ADR-0056).
//!
//! Every platform has a requirement a binding row cannot express — Mattermost
//! delivers posts only for channels the bot joined, Telegram only for chats it
//! was added to — and failing it is the worst kind of fault: nothing is broken,
//! nothing is logged, and the channel is simply silent.
//!
//! The answer is the same on every platform, so it lives here and not in an
//! adapter: a notice in the CowBot channel the binding points at, where the
//! person who would go and fix the IM is already looking.

use cowbot_core::channel::Sender;
use cowbot_service::ChannelService;
use tracing::warn;

use crate::adapter::{Integration, IntegrationContext};
use crate::model::BindingHealth;

/// How every binding notice opens.
///
/// Written once so an operator sees one sentence on every platform, and so a
/// test can assert the notice is a notice rather than matching a whole
/// platform-specific sentence.
pub const NOTICE_PREFIX: &str = "This channel's IM binding cannot receive messages";

/// Check every binding of `integration` and report the ones that cannot reach.
///
/// A notice rather than a startup error: the other bindings on this integration
/// work, and one conversation the bot was never added to is a fix an operator
/// makes in the IM, not a reason to stop serving the rest.
pub async fn report_binding_health(ctx: &IntegrationContext, adapter: &dyn Integration) {
    let bindings = match ctx
        .store
        .list_integration_bindings(&ctx.integration.id)
        .await
    {
        Ok(bindings) => bindings,
        Err(error) => {
            // The store is the caller's; a failure here is reported and the
            // adapter still runs, because receiving messages does not depend on
            // having read the binding list.
            warn!(
                integration = %ctx.integration.id,
                kind = ctx.integration.kind.as_str(),
                %error,
                "could not read the bindings to check them"
            );
            return;
        }
    };

    for binding in bindings {
        let health = adapter.check_binding(&binding.external_id).await;
        if let BindingHealth::Reachable = health {
            continue;
        }
        let reason = health.problem().unwrap_or_default();
        warn!(
            integration = %ctx.integration.id,
            kind = ctx.integration.kind.as_str(),
            external = %binding.external_id,
            channel = %binding.channel_id.0,
            %reason,
            "a bound IM conversation cannot reach the bot"
        );
        let notice = format!("{NOTICE_PREFIX}: {reason}.");
        if let Err(error) = ChannelService::new(ctx.store.clone(), ctx.bus.clone())
            .post_message(&binding.channel_id, Sender::System, notice)
            .await
        {
            // A notice about a broken binding must not be the thing that breaks
            // the adapter.
            warn!(
                channel = %binding.channel_id.0,
                %error,
                "could not post a binding notice"
            );
        }
    }
}
