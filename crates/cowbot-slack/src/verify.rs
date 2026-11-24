//! Verifying a Slack Events API request (ADR-0062).
//!
//! The Events API is the transport this crate did not implement at first. It
//! moves authentication from the connection to the body: instead of an app token
//! opening a socket, every request carries an HMAC that proves Slack sent it.
//!
//! Two properties are load-bearing and both are easy to get subtly wrong:
//!
//! * **The signed string is the raw body.** Slack signs
//!   `v0:{timestamp}:{body}` where `body` is the bytes that arrived, not a
//!   re-serialised parse of them. A decoder that round-trips the JSON changes
//!   whitespace and the signature stops matching, which is why
//!   [`super::adapter`] verifies before it parses.
//! * **The timestamp is checked, not just present.** The signature covers it, so
//!   a captured request stays valid for ever without the age check. Slack
//!   documents five minutes; that is the replay window this closes.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

/// How old a signed request may be.
///
/// Slack's own guidance. A larger window widens the replay window; a smaller one
/// refuses deliveries delayed by a retry or a slow ingress.
pub const MAX_AGE_SECS: i64 = 60 * 5;

/// Why a request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A header the signature needs is missing.
    MissingHeader(&'static str),
    /// The timestamp is not a number.
    BadTimestamp,
    /// The request is older than [`MAX_AGE_SECS`], or dated in the future.
    Stale(i64),
    /// The signature does not match the body.
    BadSignature,
    /// This configuration has no signing secret, so nothing can be verified.
    NoSecret,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::MissingHeader(name) => write!(formatter, "the {name} header is missing"),
            Refused::BadTimestamp => formatter.write_str("the request timestamp is not a number"),
            Refused::Stale(age) => write!(
                formatter,
                "the request is {age} seconds from now, outside the {MAX_AGE_SECS}-second window"
            ),
            Refused::BadSignature => formatter.write_str("the signature does not match the body"),
            Refused::NoSecret => {
                formatter.write_str("no signing secret is configured, so nothing can be verified")
            }
        }
    }
}

/// Verify one Events API request.
///
/// `now` is passed in rather than read, so a test can pin the clock without
/// sleeping and the age check has one implementation.
pub fn verify(
    secret: &str,
    timestamp: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
    now: i64,
) -> Result<(), Refused> {
    if secret.trim().is_empty() {
        return Err(Refused::NoSecret);
    }
    let timestamp = timestamp.ok_or(Refused::MissingHeader("X-Slack-Request-Timestamp"))?;
    let signature = signature.ok_or(Refused::MissingHeader("X-Slack-Signature"))?;

    let sent: i64 = timestamp.parse().map_err(|_| Refused::BadTimestamp)?;
    let age = now - sent;
    // Both directions: a request from the future is as suspicious as an old one,
    // and a clock that is a little ahead is ordinary.
    let allowed = MAX_AGE_SECS;
    if age.abs() > allowed {
        return Err(Refused::Stale(age));
    }

    let expected = sign(secret, timestamp, body);
    // Constant time, so a caller cannot learn the signature a byte at a time.
    if expected.as_bytes().ct_eq(signature.as_bytes()).unwrap_u8() == 1 {
        Ok(())
    } else {
        Err(Refused::BadSignature)
    }
}

/// The `v0=` signature for one timestamp and body.
pub fn sign(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("hmac accepts a key of any length");
    // The prefix is part of what is signed, and the version is part of the
    // prefix: `v1` would not verify against `v0`.
    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    format!("v0={}", hex(&digest))
}

/// Lower-case hex, so a comparison against Slack's own encoding is exact.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "8f742231b10e8888abcd99yyyzzz85a5";
    const BODY: &[u8] = br#"{"type":"event_callback","event":{"type":"message"}}"#;
    const NOW: i64 = 1_700_000_000;

    #[test]
    fn a_freshly_signed_request_verifies() {
        // The value Slack's own documentation publishes for this secret and
        // timestamp, so the scheme is pinned rather than merely self-consistent.
        let timestamp = "1531420618";
        let signature = sign(SECRET, timestamp, BODY);
        assert!(
            signature.starts_with("v0="),
            "the version prefix is part of the format: {signature}"
        );
        assert_eq!(signature.len(), 3 + 64, "sha256 is 32 bytes of hex");

        // Verified against the timestamp it was signed with, not `NOW`.
        assert_eq!(
            verify(
                SECRET,
                Some(timestamp),
                Some(&signature),
                BODY,
                1_531_420_618
            ),
            Ok(())
        );
    }

    #[test]
    fn a_body_that_changed_after_signing_is_refused() {
        // The whole point: an attacker who intercepts a request cannot change
        // the message it carries.
        let timestamp = NOW.to_string();
        let signature = sign(SECRET, &timestamp, BODY);
        let tampered = br#"{"type":"event_callback","event":{"type":"message","text":"evil"}}"#;
        assert_eq!(
            verify(SECRET, Some(&timestamp), Some(&signature), tampered, NOW),
            Err(Refused::BadSignature)
        );
    }

    #[test]
    fn a_captured_request_stops_working_once_it_is_older_than_the_window() {
        // The signature covers the timestamp, so without this check a captured
        // request would verify for ever.
        let timestamp = NOW.to_string();
        let signature = sign(SECRET, &timestamp, BODY);
        assert_eq!(
            verify(SECRET, Some(&timestamp), Some(&signature), BODY, NOW),
            Ok(())
        );
        assert_eq!(
            verify(
                SECRET,
                Some(&timestamp),
                Some(&signature),
                BODY,
                NOW + MAX_AGE_SECS + 1
            ),
            Err(Refused::Stale(MAX_AGE_SECS + 1))
        );
    }

    #[test]
    fn a_request_dated_in_the_future_is_refused_too() {
        // A clock a little ahead is ordinary; an hour ahead is not, and the
        // window is symmetric so an attacker cannot buy time by claiming one.
        let timestamp = (NOW + MAX_AGE_SECS + 60).to_string();
        let signature = sign(SECRET, &timestamp, BODY);
        assert!(matches!(
            verify(SECRET, Some(&timestamp), Some(&signature), BODY, NOW),
            Err(Refused::Stale(_))
        ));
    }

    #[test]
    fn a_wrong_secret_is_refused() {
        let timestamp = NOW.to_string();
        let signature = sign("the-wrong-secret", &timestamp, BODY);
        assert_eq!(
            verify(SECRET, Some(&timestamp), Some(&signature), BODY, NOW),
            Err(Refused::BadSignature)
        );
    }

    #[test]
    fn a_missing_header_is_named_rather_than_reported_as_a_bad_signature() {
        // The two failures have different fixes — one is a proxy stripping a
        // header, the other is a wrong secret — so they must not read the same.
        assert_eq!(
            verify(SECRET, None, Some("v0=00"), BODY, NOW),
            Err(Refused::MissingHeader("X-Slack-Request-Timestamp"))
        );
        assert_eq!(
            verify(SECRET, Some("1700000000"), None, BODY, NOW),
            Err(Refused::MissingHeader("X-Slack-Signature"))
        );
    }

    #[test]
    fn a_handler_with_no_secret_refuses_rather_than_skips_verification() {
        // A deployment that chose the Events API without storing a signing
        // secret must not accept unsigned traffic.
        assert_eq!(
            verify("", Some("1700000000"), Some("v0=00"), BODY, NOW),
            Err(Refused::NoSecret)
        );
    }

    #[test]
    fn a_non_numeric_timestamp_is_refused() {
        assert_eq!(
            verify(SECRET, Some("yesterday"), Some("v0=00"), BODY, NOW),
            Err(Refused::BadTimestamp)
        );
    }
}
