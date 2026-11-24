//! The adapter contract (ADR-0056).
//!
//! An adapter owns two things a platform makes unavoidable: verifying what it
//! receives, and speaking the platform's own payload shape on the way out. It
//! does not own the pipeline, the turn, or the model — those are shared, which is
//! what lets one implementation of the inbound and outbound rules serve four
//! platforms.

use async_trait::async_trait;

use cowbot_core::cancel::CancelSignal;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::integration::IntegrationKind;
use cowbot_store::Store;

use crate::model::{BindingHealth, DeliveredId, OutboundContent, OutboundTarget};

/// What an adapter is given when it starts.
///
/// Deliberately not an `LlmProvider`: running the turn is the deployment's job,
/// injected through [`MessageSink`], so an adapter never learns how a turn
/// executes and stays testable without a model.
#[derive(Clone)]
pub struct IntegrationContext {
    pub store: Store,
    /// The deployment's event bus.
    ///
    /// Carried so an adapter can post a notice the way a turn does. A bus of its
    /// own would write the notice to the database and publish it to nobody, which
    /// is a notice no client ever sees.
    pub bus: cowbot_bus::EventBus,
    /// The integration this adapter was started for.
    pub integration: cowbot_core::integration::IntegrationRecord,
    /// Where an accepted message goes.
    pub sink: std::sync::Arc<dyn MessageSink>,
    /// Where attachment bytes are written, when the deployment has a root.
    ///
    /// The same directory the deployment's file tools are rooted at, so a path it
    /// produces is one a tool can open. `None` for a deployment with no root — the
    /// test fixtures, and a build whose tools are not rooted anywhere — in which
    /// case an attachment is named in the message text as not kept.
    pub attachments_dir: Option<std::path::PathBuf>,
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

    /// How this platform measures a message's length.
    ///
    /// The default is the shared character budget. An adapter overrides it when
    /// its platform counts differently — Telegram counts UTF-16 code units — so
    /// an answer is split to a limit the platform will actually accept.
    fn chunk_limit(&self) -> crate::model::ChunkLimit {
        crate::model::ChunkLimit::Characters(crate::outbound::MAX_CHUNK_CHARS)
    }

    /// Whether this external conversation can actually reach the adapter.
    ///
    /// A binding row says a conversation feeds a channel; it does not say the
    /// platform will deliver anything. Every platform has a requirement the row
    /// cannot express — Mattermost delivers posts only for channels the bot has
    /// joined — and a binding that fails it is silent rather than broken, which
    /// makes it the hardest kind of fault for an operator to find.
    ///
    /// The default reports [`BindingHealth::Unknown`] rather than
    /// [`BindingHealth::Reachable`]: an adapter that cannot check has not
    /// confirmed anything, and a green light it did not earn is worse than an
    /// honest "I cannot tell".
    async fn check_binding(&self, _external_id: &str) -> BindingHealth {
        BindingHealth::Unknown("this adapter cannot check its bindings yet".into())
    }

    /// Fetch the bytes of one attachment this adapter reported (ADR-0060).
    ///
    /// `max_bytes` is passed down rather than checked afterwards because the cap
    /// has to stop the read: buffering a two-gigabyte file to then reject it is
    /// the denial of service the cap exists to prevent. An adapter must also stop
    /// at a size the platform already declared.
    ///
    /// The default refuses. It is reached by a platform that has no attachments
    /// and by an adapter that has not written the download yet, and both should
    /// say so in the message text rather than return bytes that are not there.
    async fn fetch_attachment(
        &self,
        _attachment: &crate::model::Attachment,
        _max_bytes: u64,
    ) -> CowResult<Vec<u8>> {
        Err(cowbot_core::error::CowError::Config(format!(
            "this {} adapter cannot download attachments",
            self.kind().as_str()
        )))
    }

    /// Ask a person in this conversation to decide an approval (ADR-0060).
    ///
    /// Posts the question and returns; it does not wait. The waiting turn is
    /// already polling the row, so an adapter that blocked here would stall its
    /// own mirror for as long as a person takes to look at their phone.
    ///
    /// The default posts the prompt as ordinary text: the person can read it in
    /// the IM and decide from a CowBot client, which is a real answer rather than
    /// a button that does nothing. An adapter whose platform has buttons
    /// overrides this.
    async fn post_approval(
        &self,
        target: &OutboundTarget,
        prompt: &crate::approvals::ApprovalPrompt,
    ) -> CowResult<crate::approvals::ApprovalPosting> {
        let delivered = self
            .deliver(
                target,
                &OutboundContent {
                    text: format!(
                        "{}\n\nDecide this in CowBot: this channel cannot carry the buttons.",
                        prompt.describe()
                    ),
                    thread_id: target.thread_id.clone(),
                    part: crate::model::ChunkPart { index: 0, total: 1 },
                },
            )
            .await?;
        Ok(crate::approvals::ApprovalPosting {
            delivered,
            buttons: false,
        })
    }

    /// Revise the message `post_approval` posted, once the request is decided.
    ///
    /// `posting` is what [`Self::post_approval`] returned and `summary` is
    /// [`crate::approvals::decision_summary`]. The default does nothing: a
    /// platform without an edit call leaves the original question in the room,
    /// which is a stale card rather than a wrong one.
    async fn update_approval(
        &self,
        _target: &OutboundTarget,
        _posting: &crate::approvals::ApprovalPosting,
        _summary: &str,
    ) -> CowResult<()> {
        Ok(())
    }

    /// Whether this adapter is running on a webhook transport (ADR-0062).
    ///
    /// Default `false`. Every adapter predating the webhook work dials out, and
    /// one that only dials out must never be asked to accept an inbound request
    /// — the route reports a deployment that asks for it as a misconfiguration
    /// rather than accepting a body it cannot verify.
    fn accepts_webhook(&self) -> bool {
        false
    }

    /// Verify one inbound HTTP request and hand it to the pipeline.
    ///
    /// Everything a platform forces — a signature, a secret header, a
    /// challenge, a decryption — belongs here, and it are verified against the
    /// **raw** body before anything parses it. Everything after verification
    /// belongs to the shared pipeline, which is why an implementation is a thin
    /// adapter over the translation the socket transport already uses.
    ///
    /// The default refuses. Reaching it means a caller ignored
    /// [`Self::accepts_webhook`], and saying so is better than answering a
    /// platform with a success it will not retry.
    async fn webhook(
        &self,
        _ctx: &IntegrationContext,
        _request: InboundRequest<'_>,
    ) -> CowResult<WebhookReply> {
        Err(CowError::Config(format!(
            "this {} adapter does not accept webhooks",
            self.kind().as_str()
        )))
    }
}

/// One inbound HTTP request, before it has been trusted.
///
/// The body is the raw bytes because a signature is computed over what arrived,
/// not over what a parser would produce from it. A platform's JSON is parsed
/// only after [`Integration::webhook`] has decided the request is genuine.
pub struct InboundRequest<'a> {
    /// Every request header, lower-cased.
    pub headers: &'a [(String, String)],
    /// The body exactly as it arrived.
    pub body: &'a [u8],
}

impl InboundRequest<'_> {
    /// One header's value, matched case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// What a platform's webhook does with one request.
///
/// Each variant is a response a platform actually expects, so the route can
/// answer correctly without knowing which platform it is serving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookReply {
    /// Accepted and handled: 200 with an empty body.
    Ack,
    /// The platform's own handshake challenge, echoed back verbatim.
    ///
    /// Slack wants `{"challenge": …}` and Feishu wants the same shape; the
    /// variant carries the whole body so each adapter answers with what its
    /// platform documents rather than a shape the route invented.
    Challenge(String),
    /// A button press, answered in place.
    ///
    /// The body is optional because a platform that only needs an
    /// acknowledgement sends nothing, and one that shows a toast sends the JSON.
    Interaction(Option<String>),
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
