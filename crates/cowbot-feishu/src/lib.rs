//! The Feishu / Lark adapter (ADR-0061).
//!
//! The plan for this stage said Feishu would be the adapter that forces the
//! crypto work — an Encrypt Key, AES-256-CBC, an HMAC over the raw body, a
//! `url_verification` handshake answered inside one second. That is all true of
//! Feishu's **webhook** transport, and it is why the stage was ordered last.
//!
//! Feishu also has a long-connection transport, and it removes every one of those
//! requirements: CowBot dials out, and the frames it receives after the handshake
//! are plaintext that Feishu has already authenticated. There is no public URL,
//! no signature, and no decryption. So this crate implements the long connection
//! and not the webhook, exactly as the Telegram adapter implements long polling
//! and not `setWebhook` (ADR-0058). An operator who needs a webhook — because the
//! deployment is a cloud function, or the tenant forbids long connections — is
//! served by a later change, not by a half-built one.
//!
//! The protocol is the part that is genuinely hard, and it is not the crypto:
//!
//! * frames are **protobuf**, not JSON — a length-delimited envelope with a
//!   service/method pair and a payload;
//! * a payload over its fragmentation threshold arrives in pieces keyed by
//!   `message_id`, to be reassembled in `seq` order;
//! * a handler has **three seconds** to answer on the same socket, and the answer
//!   is itself a protobuf frame whose payload is JSON wrapped in base64;
//! * a `ping` must go out on an interval, and a pong must come back, or a silently
//!   dead path looks like an idle one.
//!
//! Because the frames are protobuf and this crate has no protobuf dependency, the
//! four fields the protocol needs are decoded by hand. It is a varint and a few
//! length-delimited fields — see [`frame`] for the wire format and why a
//! hand-written decoder is defensible here rather than a `prost` dependency.

pub mod adapter;
pub mod api;
pub mod card;
pub mod config;
pub mod crypto;
pub mod event;
pub mod frame;
pub mod socket;

pub use adapter::FeishuIntegration;
pub use config::FeishuConfig;
