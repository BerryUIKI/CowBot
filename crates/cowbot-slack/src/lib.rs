//! The Slack adapter (ADR-0059).
//!
//! The fourth platform, and the second one reached over a WebSocket. Three
//! things are Slack's own and shape this crate:
//!
//! * **Socket Mode** turns the event stream into an outbound WebSocket, so the
//!   deployment needs no public URL — the same property Mattermost's socket and
//!   Telegram's long poll give it.
//! * **Every envelope is acknowledged individually**, within three seconds, by
//!   echoing its `envelope_id`. A message that is not acknowledged is redelivered,
//!   so the ack is what replaces a signature: there is no inbound request to
//!   verify.
//! * **Text is mrkdwn**, in which a mention is `<@U123>` and the characters `&`,
//!   `<`, `>` arrive escaped. Both have to be undone before the message is
//!   stored, or the timeline shows opaque ids and `&amp;`.

pub mod adapter;
pub mod api;
pub mod blocks;
pub mod config;
pub mod socket;

pub use adapter::SlackIntegration;
pub use config::SlackConfig;
