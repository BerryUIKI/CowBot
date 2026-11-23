//! The configuration an operator stores for a Feishu / Lark app (ADR-0061).
//!
//! Three values: the app's id and secret, and which of the two platform domains
//! it belongs to. The domain is an enum rather than a URL because the two hosts
//! the adapter needs — the OpenAPI origin and the accounts origin the OAuth token
//! endpoint lives on — are both derived from it, and a free-form base URL would
//! be a third thing to get wrong (the same reasoning that makes
//! `MattermostConfig` refuse a `wss://` origin).
//!
//! The encrypt key and verification token a webhook transport would need are not
//! configuration here. Long-connection mode carries neither, and storing a secret
//! nothing reads is worse than not accepting it: it looks like it is in force.

use cowbot_core::error::{CowError, CowResult};
use serde::Deserialize;

/// Which Feishu platform an app belongs to.
///
/// `Feishu` is the mainland deployment and `Lark` the international one. They are
/// separate services with separate credentials, so an app created on one cannot
/// be used against the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Domain {
    #[default]
    Feishu,
    Lark,
}

impl Domain {
    /// The OpenAPI origin: every business call lives below this.
    pub fn open_api(self) -> &'static str {
        match self {
            Domain::Feishu => "https://open.feishu.cn",
            Domain::Lark => "https://open.larksuite.com",
        }
    }

    /// The accounts origin, which is where the OAuth token endpoint lives.
    ///
    /// A different host from [`Self::open_api`]: the token endpoint was moved
    /// there, and a request to the wrong host answers a redirect rather than a
    /// token.
    pub fn accounts(self) -> &'static str {
        match self {
            Domain::Feishu => "https://accounts.feishu.cn",
            Domain::Lark => "https://accounts.larksuite.com",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Domain::Feishu => "feishu",
            Domain::Lark => "lark",
        }
    }
}

/// A Feishu / Lark app's connection settings.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct FeishuConfig {
    /// The app id, `cli_…`.
    pub app_id: String,
    /// The app secret, used for both the tenant token and the socket handshake.
    pub app_secret: String,
    #[serde(default)]
    pub domain: Domain,
    #[serde(default)]
    pub transport: Transport,
    /// The Verification Token from the app's console.
    ///
    /// Required for [`Transport::Webhook`], where it is what a card callback is
    /// signed with. Unread by the long connection, which carries no token at all.
    #[serde(default)]
    pub verification_token: Option<String>,
    /// The Encrypt Key, when the app has one configured.
    ///
    /// A webhook delivery is encrypted with it and refused unsigned otherwise.
    /// Optional even in webhook mode: an app that left the Encrypt Key blank
    /// receives plaintext, and refusing to start would break a working setup.
    #[serde(default)]
    pub encrypt_key: Option<String>,
}

/// How Feishu delivers events (ADR-0062).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// An outbound WebSocket, the default: no public URL, no crypto.
    #[default]
    LongConnection,
    /// An inbound HTTPS endpoint, which needs decryption and a signature.
    Webhook,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::LongConnection => "long_connection",
            Transport::Webhook => "webhook",
        }
    }
}

impl FeishuConfig {
    /// Read the configuration an operator stored.
    pub fn from_json(config: &str) -> CowResult<Self> {
        let mut parsed: FeishuConfig = serde_json::from_str(config)
            .map_err(|error| CowError::Config(format!("invalid Feishu config: {error}")))?;
        parsed.app_id = parsed.app_id.trim().to_string();
        parsed.app_secret = parsed.app_secret.trim().to_string();

        if !parsed.app_id.starts_with("cli_") {
            return Err(CowError::Config(
                "app_id must be the app's id from the developer console, which starts with \
                 `cli_`"
                    .into(),
            ));
        }
        parsed.verification_token = parsed
            .verification_token
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        parsed.encrypt_key = parsed
            .encrypt_key
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());

        // The verification token is what a card callback is signed with. Without
        // it that delivery kind cannot be authenticated at all, so a webhook
        // configuration that omitted it is refused here rather than dropping
        // every button press at runtime.
        if parsed.transport == Transport::Webhook && parsed.verification_token.is_none() {
            return Err(CowError::Config(
                "verification_token is required when transport is 'webhook': it is what signs \
                 a card callback. Use transport 'long_connection', or copy the token from the \
                 app's Event Subscriptions page"
                    .into(),
            ));
        }
        if parsed.app_secret.is_empty() {
            return Err(CowError::Config("app_secret cannot be empty".into()));
        }
        Ok(parsed)
    }

    /// An absolute OpenAPI URL for one path.
    pub fn open_api(&self, path: &str) -> String {
        format!("{}{}", self.domain.open_api(), path)
    }
}

impl std::fmt::Debug for FeishuConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The secret is a full credential for the app, so it is never printed.
        formatter
            .debug_struct("FeishuConfig")
            .field("app_id", &self.app_id)
            .field("app_secret", &"[sealed]")
            .field("domain", &self.domain)
            .field("transport", &self.transport)
            .field(
                "verification_token",
                &self.verification_token.as_ref().map(|_| "[sealed]"),
            )
            .field(
                "encrypt_key",
                &self.encrypt_key.as_ref().map(|_| "[sealed]"),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: &str) -> CowResult<FeishuConfig> {
        FeishuConfig::from_json(json)
    }

    #[test]
    fn the_domain_defaults_to_feishu_and_can_select_lark() {
        let feishu = config(r#"{"app_id":"cli_1","app_secret":"s"}"#).unwrap();
        assert_eq!(feishu.domain, Domain::Feishu);
        assert_eq!(
            feishu.open_api("/open-apis/im/v1/messages"),
            "https://open.feishu.cn/open-apis/im/v1/messages"
        );

        let lark = config(r#"{"app_id":"cli_1","app_secret":"s","domain":"lark"}"#).unwrap();
        assert_eq!(lark.domain, Domain::Lark);
        assert!(lark
            .open_api("/x")
            .starts_with("https://open.larksuite.com"));
    }

    #[test]
    fn the_token_host_is_not_the_openapi_host() {
        // The move matters: a token request to the OpenAPI origin answers a
        // redirect rather than a token.
        assert_ne!(Domain::Feishu.open_api(), Domain::Feishu.accounts());
        assert_eq!(Domain::Feishu.accounts(), "https://accounts.feishu.cn");
    }

    #[test]
    fn an_app_id_that_is_not_one_is_refused_with_the_reason() {
        let error = config(r#"{"app_id":"ou_123","app_secret":"s"}"#).unwrap_err();
        assert!(error.to_string().contains("cli_"), "{error}");
    }

    #[test]
    fn an_empty_secret_is_refused() {
        let error = config(r#"{"app_id":"cli_1","app_secret":"  "}"#).unwrap_err();
        assert!(error.to_string().contains("app_secret"), "{error}");
    }

    #[test]
    fn a_debug_print_never_carries_the_secret() {
        let printed = format!(
            "{:?}",
            config(r#"{"app_id":"cli_1","app_secret":"super-secret"}"#).unwrap()
        );
        assert!(!printed.contains("super-secret"), "{printed}");
        assert!(printed.contains("[sealed]"));
    }
}
