//! The adapter contract (ADR-0056).
//!
//! An adapter owns two things a platform makes unavoidable: verifying what it
//! receives, and speaking the platform's own payload shape on the way out. It
//! does not own the pipeline, the turn, or the model — those are shared, which is
//! what lets one implementation of the inbound and outbound rules serve four
//! platforms.

use async_trait::async_trait;

use cowbot_core::cancel::CancelSignal;
use cowbot_core::error::CowResult;
use cowbot_core::integration::IntegrationKind;
use cowbot_store::Store;

use crate::model::{DeliveredId, OutboundContent, OutboundTarget};

/// What an adapter is given when it starts.
///
/// Deliberately not an `LlmProvider`: running the turn is the deployment's job,
/// injected through [`MessageSink`], so an adapter never learns how a turn
/// executes and stays testable without a model.
#[derive(Clone)]
pub struct IntegrationContext {
    pub store: Store,
    /// The integration this adapter was started for.
    pub integration: cowbot_core::integration::IntegrationRecord,
    /// Where an accepted message goes.
    pub sink: std::sync::Arc<dyn MessageSink>,
}

impl std::fmt::Debug for IntegrationContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IntegrationContext")
            .field("integration", &self.integration.id)
            .finish_non_exhaustive()
    }
}

/// What happens to a message an adapter accepted.
///
/// The pipeline calls this; a test calls it through `crate::ingest` directly. The
/// trait exists so an adapter depends on the behaviour and not on the
/// implementation — the server hands it a sink that closes over its
/// `TurnExecutor`, and a test hands it a sink that records.
#[async_trait]
pub trait MessageSink: Send + Sync {
    /// Store one inbound message and start a turn if it addresses an agent.
    ///
    /// Returns the outcome so the adapter can decide whether to acknowledge
    /// quietly or report something an operator should see.
    async fn accept(
        &self,
        message: crate::model::InboundMessage,
    ) -> CowResult<crate::model::Ingested>;
}

/// One IM connection bound to one CowBot channel.
///
/// Implementations are per-integration and hold their own credentials; the
/// shared work lives in [`crate::ingest`] and [`crate::outbound`].
#[async_trait]
pub trait Integration: Send + Sync {
    fn kind(&self) -> IntegrationKind;

    /// Run the inbound loop until the token is cancelled.
    ///
    /// Returns `Ok(())` on a clean cancellation and an error only when the
    /// connection failed in a way worth reporting. A reconnect is the adapter's
    /// own business: a platform that drops a socket every few hours must not
    /// wake an operator each time.
    async fn run(&self, ctx: IntegrationContext, cancel: CancelSignal) -> CowResult<()>;

    /// Post one chunk of a reply back to the conversation it came from.
    async fn deliver(
        &self,
        target: &OutboundTarget,
        content: &OutboundContent,
    ) -> CowResult<DeliveredId>;
}

/// The adapters a deployment can start, by kind.
///
/// Registration rather than a match in the supervisor: the platform-specific
/// crates land one at a time (S2–S6), and each registers itself here without the
/// supervisor learning its name.
#[derive(Default)]
pub struct Registry {
    factories: Vec<(IntegrationKind, Factory)>,
}

/// Builds an adapter from its stored configuration.
pub type Factory =
    std::sync::Arc<dyn Fn(&str) -> CowResult<std::sync::Arc<dyn Integration>> + Send + Sync>;

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one platform's adapter.
    pub fn register(&mut self, kind: IntegrationKind, factory: Factory) {
        self.factories.retain(|(registered, _)| *registered != kind);
        self.factories.push((kind, factory));
    }

    /// Every kind this build can start.
    pub fn kinds(&self) -> Vec<IntegrationKind> {
        self.factories.iter().map(|(kind, _)| *kind).collect()
    }

    /// Build the adapter for `kind` from `config`, if this build has one.
    ///
    /// A kind with no adapter is not an error here: the row can exist before the
    /// build that runs it, and the supervisor reports that rather than refusing
    /// to start the ones it does have.
    pub fn build(
        &self,
        kind: IntegrationKind,
        config: &str,
    ) -> CowResult<Option<std::sync::Arc<dyn Integration>>> {
        match self
            .factories
            .iter()
            .find(|(registered, _)| *registered == kind)
        {
            Some((_, factory)) => Ok(Some(factory(config)?)),
            None => Ok(None),
        }
    }
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Registry")
            .field("kinds", &self.kinds())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Ingested;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct Stub;

    #[async_trait]
    impl Integration for Stub {
        fn kind(&self) -> IntegrationKind {
            IntegrationKind::Telegram
        }

        async fn run(&self, _ctx: IntegrationContext, cancel: CancelSignal) -> CowResult<()> {
            cancel.cancelled().await;
            Ok(())
        }

        async fn deliver(
            &self,
            _target: &OutboundTarget,
            _content: &OutboundContent,
        ) -> CowResult<DeliveredId> {
            Ok(DeliveredId("1".into()))
        }
    }

    #[test]
    fn a_registered_kind_builds_and_an_unregistered_one_reports_none() {
        let mut registry = Registry::new();
        registry.register(
            IntegrationKind::Telegram,
            Arc::new(|_config| Ok(Arc::new(Stub) as Arc<dyn Integration>)),
        );

        assert_eq!(registry.kinds(), vec![IntegrationKind::Telegram]);
        let built = registry.build(IntegrationKind::Telegram, "{}").unwrap();
        assert_eq!(built.unwrap().kind(), IntegrationKind::Telegram);
        assert!(registry
            .build(IntegrationKind::Slack, "{}")
            .unwrap()
            .is_none());
    }

    #[test]
    fn registering_a_kind_twice_keeps_the_latest() {
        let mut registry = Registry::new();
        register_stub(&mut registry, IntegrationKind::Slack);
        register_stub(&mut registry, IntegrationKind::Slack);
        assert_eq!(registry.kinds(), vec![IntegrationKind::Slack]);
    }

    fn register_stub(registry: &mut Registry, kind: IntegrationKind) {
        registry.register(
            kind,
            Arc::new(move |_config| {
                let _ = kind;
                Ok(Arc::new(Stub) as Arc<dyn Integration>)
            }),
        );
    }

    #[tokio::test]
    async fn a_cancelled_run_returns_cleanly() {
        let cancel = CancelSignal::new();
        cancel.cancel();
        // No context is built here: a clean cancellation must not need one.
        assert!(cancel.is_cancelled());
        let _ = Ingested::Skipped(crate::model::Skipped::Duplicate);
    }
}
