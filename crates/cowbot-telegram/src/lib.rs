//! The Telegram adapter: a Bot API connection over long polling (ADR-0058).
//!
//! Telegram is the second adapter and the first *SaaS* one. Mattermost proved the
//! contract against a self-hosted system the deployment already runs; Telegram
//! proves it against a remote platform with retries and rate limits — and costs a
//! fraction of Slack's work, because there is no signature to verify, nothing to
//! decrypt, and no public endpoint to expose.
//!
//! The transport is the cheapest there is: `getUpdates` is an HTTPS request the
//! server makes outward, with the token in the path and no handshake. A bot
//! needs no reverse proxy, no DNS name, and no TLS certificate — which is the
//! same reason a Mattermost bot dials out, arrived at without a socket.
//!
//! Its two surfaces map onto what CowBot already models:
//!
//! * a private chat needs no mention, which is a direct conversation (ADR-0024);
//! * a group delivers only what mentions the bot, which is a mention turn
//!   (ADR-0022), and that is Telegram's default rather than a switch.
//!
//! ```text
//! config.rs   the JSON an operator stores: the bot token, an optional API base
//! update.rs   the update shapes, and the UTF-16 entity offsets mention handling
//!             depends on
//! api.rs      the Bot API calls: getMe, getUpdates, sendMessage, getChat
//! render.rs   an answer as Telegram's HTML, escaped first and tagged after
//! adapter.rs  the Integration implementation, tying those to the pipeline
//! ```

pub mod adapter;
pub mod api;
pub mod config;
pub mod render;
pub mod update;

pub use adapter::TelegramIntegration;
pub use config::TelegramConfig;
