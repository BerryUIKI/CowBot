//! The Mattermost adapter: a bot account over the REST API and the WebSocket
//! event stream (ADR-0057).
//!
//! Mattermost is the reference adapter for three reasons. It is self-hosted like
//! `cowbot-server`, so the deployment is already the kind of operator CowBot
//! targets. Its concepts map almost one-to-one — channels, threads, posts — so
//! proving the adapter contract against it costs little translation. And a bot
//! connects *outward*, so the deployment needs no inbound surface: no reverse
//! proxy, no public DNS, no TLS termination, no signature verification. The
//! blast radius of a bug is the bot's own session token.
//!
//! ```text
//! config.rs   the JSON an operator stores: server URL, bot token
//! rest.rs     the HTTP calls: who am I, post, read a channel
//! socket.rs   the event stream and the reconnect loop
//! adapter.rs  the Integration implementation, tying those to the pipeline
//! ```

pub mod adapter;
pub mod config;
pub mod rest;
pub mod socket;

pub use adapter::MattermostIntegration;
pub use config::MattermostConfig;
