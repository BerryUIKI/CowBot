//! Server configuration.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use cowbot_llm::{
    AnthropicProvider, LlmProvider, MockLlmProvider, OpenAiCompatibleProvider, ResponsesProvider,
};

/// Wire protocol a server-side provider speaks, mirroring the desktop's three
/// (ADR-0016, ADR-0021).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerLlmProtocol {
    /// OpenAI Chat Completions, and every endpoint that speaks it.
    OpenAiChat,
    /// Anthropic Messages.
    Anthropic,
    /// OpenAI Responses (Codex-style endpoints).
    OpenAiResponses,
    /// A provider that echoes the prompt, for a deployment without a key and
    /// for the tests that exercise the executor.
    Mock,
}

impl ServerLlmProtocol {
    /// Parse the value of `COWBOT_SERVER_LLM_PROTOCOL`.
    ///
    /// Accepts the names this crate documents, so an operator who reads
    /// `docs/SERVER.md` and types what it says is not punished for it.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai-chat" | "openai" => Ok(Self::OpenAiChat),
            "anthropic" => Ok(Self::Anthropic),
            "openai-responses" | "responses" => Ok(Self::OpenAiResponses),
            "mock" => Ok(Self::Mock),
            other => Err(format!(
                "COWBOT_SERVER_LLM_PROTOCOL is not a known protocol: '{other}' \
                 (expected openai-chat, anthropic, openai-responses, or mock)"
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai-chat",
            Self::Anthropic => "anthropic",
            Self::OpenAiResponses => "openai-responses",
            Self::Mock => "mock",
        }
    }
}

/// The LLM a server deployment answers channel turns with (ADR-0055).
///
/// Absent means the server posts messages but runs no turn, which is exactly
/// what a server did before this existed: a client sees its message stored and
/// no answer arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerLlmConfig {
    pub protocol: ServerLlmProtocol,
    /// Endpoint base URL. Unused by [`ServerLlmProtocol::Mock`].
    pub base_url: Option<String>,
    /// Stored so the deployment can rotate it in its environment; the server
    /// never writes it anywhere or returns it from an API.
    pub api_key: Option<String>,
    pub model: String,
    /// Context window the endpoint advertises for `model`, if the operator
    /// knows it.
    ///
    /// A conversation's capacity check needs this and the server has no model
    /// catalogue of its own, so it is configuration rather than discovery. Left
    /// unset, the check is skipped and a full branch surfaces as a turn failure
    /// instead of a skip — the same degraded behavior a group has when the
    /// desktop passes no window.
    pub context_window: Option<u32>,
    /// Output limit the endpoint advertises for `model`, subtracted from the
    /// window when computing what a turn may use.
    pub max_output_tokens: Option<u32>,
}

impl ServerLlmConfig {
    /// Build the configured LLM, or `None` when the deployment configures none.
    ///
    /// The four variables are all-or-nothing: setting only some of them is a
    /// configuration error rather than a silent fallback, the same rule the
    /// rest of this struct applies to a mistyped value.
    pub fn from_env() -> Result<Option<Self>, String> {
        let context_window = parse_optional_number(
            "COWBOT_SERVER_LLM_CONTEXT_WINDOW",
            read("COWBOT_SERVER_LLM_CONTEXT_WINDOW"),
        )?;
        let max_output_tokens = parse_optional_number(
            "COWBOT_SERVER_LLM_MAX_OUTPUT_TOKENS",
            read("COWBOT_SERVER_LLM_MAX_OUTPUT_TOKENS"),
        )?;
        Self::from_values(
            read("COWBOT_SERVER_LLM_PROTOCOL"),
            read("COWBOT_SERVER_LLM_BASE_URL"),
            read("COWBOT_SERVER_LLM_API_KEY"),
            read("COWBOT_SERVER_LLM_MODEL"),
            context_window,
            max_output_tokens,
        )
    }

    /// The pure part of [`Self::from_env`], so the rule can be tested without
    /// mutating the process environment.
    pub fn from_values(
        protocol: Option<String>,
        base_url: Option<String>,
        api_key: Option<String>,
        model: Option<String>,
        context_window: Option<u32>,
        max_output_tokens: Option<u32>,
    ) -> Result<Option<Self>, String> {
        if protocol.is_none() && base_url.is_none() && api_key.is_none() && model.is_none() {
            return Ok(None);
        }

        let protocol = protocol.ok_or_else(|| {
            "COWBOT_SERVER_LLM_PROTOCOL is required when any other COWBOT_SERVER_LLM_* \
             variable is set"
                .to_string()
        })?;
        let protocol = ServerLlmProtocol::parse(&protocol)?;

        // A mock needs no endpoint and no model: it answers whatever it is
        // asked, so requiring either would make the one configuration that
        // works without a key the hardest to set up.
        if protocol == ServerLlmProtocol::Mock {
            return Ok(Some(Self {
                protocol,
                base_url,
                api_key,
                model: model.unwrap_or_else(|| "mock".to_string()),
                context_window,
                max_output_tokens,
            }));
        }

        let base_url = base_url.ok_or_else(|| {
            "COWBOT_SERVER_LLM_BASE_URL is required for a remote provider".to_string()
        })?;
        let model = model.ok_or_else(|| {
            "COWBOT_SERVER_LLM_MODEL is required for a remote provider".to_string()
        })?;
        Ok(Some(Self {
            protocol,
            base_url: Some(base_url),
            api_key,
            model,
            context_window,
            max_output_tokens,
        }))
    }

    /// Build the provider this configuration describes.
    ///
    /// The four clients are the same ones the desktop builds for its own saved
    /// providers (ADR-0021), so a deployment and a desktop talk to an endpoint
    /// identically.
    pub fn provider(&self) -> Arc<dyn LlmProvider> {
        match self.protocol {
            ServerLlmProtocol::OpenAiChat => Arc::new(OpenAiCompatibleProvider::new(
                self.base_url.clone().unwrap_or_default(),
                self.api_key.clone(),
            )),
            ServerLlmProtocol::Anthropic => Arc::new(AnthropicProvider::new(
                self.base_url.clone().unwrap_or_default(),
                self.api_key.clone(),
            )),
            ServerLlmProtocol::OpenAiResponses => Arc::new(ResponsesProvider::new(
                self.base_url.clone().unwrap_or_default(),
                self.api_key.clone(),
            )),
            ServerLlmProtocol::Mock => Arc::new(MockLlmProvider::echo()),
        }
    }
}

fn read(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Parse an optional numeric variable, naming it in the error.
///
/// A value that does not parse stops the server for the same reason every other
/// setting does: a mistyped window would silently disable the capacity check,
/// and the operator would have no way to notice.
fn parse_optional_number(name: &str, value: Option<String>) -> Result<Option<u32>, String> {
    match value {
        None => Ok(None),
        Some(raw) => raw
            .parse()
            .map(Some)
            .map_err(|_| format!("{name} is not a number: '{raw}'")),
    }
}

/// Everything the server needs to start.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// The SQLite database the server owns. The server is the only writer.
    pub database: PathBuf,
    /// The address to listen on.
    pub address: SocketAddr,
    /// Name given to the owner account created on an empty database.
    pub owner_name: String,
    /// Email the owner signs in with.
    pub owner_email: String,
    /// Workspace created on an empty database.
    pub workspace_name: String,
    /// How long a session stays valid before the user must sign in again.
    pub session_hours: i64,
    /// Failed sign-ins allowed per window from one source before the server
    /// starts refusing them.
    pub login_attempts: u32,
    /// Length of the rate-limiting window, in seconds.
    pub login_window_secs: i64,
    /// Directory the server's file tools are rooted at, and where a sandbox
    /// program runs. Created on start when the LLM is configured.
    ///
    /// A server has no "user project folder", so a turn that writes a file must
    /// be confined to a directory the server owns (ADR-0055).
    pub workspace_dir: PathBuf,
    /// The LLM that answers channel turns; `None` runs none.
    pub llm: Option<ServerLlmConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            database: PathBuf::from("cowbot-server.db"),
            address: "127.0.0.1:8787"
                .parse()
                .expect("the default listen address parses"),
            owner_name: "Owner".to_string(),
            owner_email: "owner@cowbot.local".to_string(),
            workspace_name: "Team Workspace".to_string(),
            session_hours: 24 * 14,
            login_attempts: 10,
            login_window_secs: 15 * 60,
            workspace_dir: PathBuf::from("cowbot-workspace"),
            llm: None,
        }
    }
}

impl ServerConfig {
    /// Read the environment variables a container deployment sets.
    ///
    /// Every setting has a default that keeps a single-machine trial working,
    /// and a value that does not parse is an error rather than a silent
    /// fallback — a mistyped listen address should stop the server, not move it
    /// somewhere unexpected.
    ///
    /// | Variable | Default |
    /// | --- | --- |
    /// | `COWBOT_SERVER_DATABASE` | `cowbot-server.db` |
    /// | `COWBOT_SERVER_ADDRESS` | `127.0.0.1:8787` |
    /// | `COWBOT_SERVER_OWNER_NAME` | `Owner` |
    /// | `COWBOT_SERVER_OWNER_EMAIL` | `owner@cowbot.local` |
    /// | `COWBOT_SERVER_WORKSPACE` | `Team Workspace` |
    /// | `COWBOT_SERVER_SESSION_HOURS` | `336` |
    /// | `COWBOT_SERVER_LOGIN_ATTEMPTS` | `10` |
    /// | `COWBOT_SERVER_LOGIN_WINDOW_SECS` | `900` |
    /// | `COWBOT_SERVER_WORKSPACE_DIR` | `cowbot-workspace` |
    ///
    /// The four `COWBOT_SERVER_LLM_*` variables are read by
    /// [`ServerLlmConfig::from_env`] and are all-or-nothing: none of them set
    /// means the server runs no turn (ADR-0055).
    pub fn from_env() -> Result<Self, String> {
        let mut config = Self::default();
        if let Ok(value) = std::env::var("COWBOT_SERVER_DATABASE") {
            config.database = PathBuf::from(value);
        }
        if let Ok(value) = std::env::var("COWBOT_SERVER_ADDRESS") {
            config.address = value
                .parse()
                .map_err(|_| format!("COWBOT_SERVER_ADDRESS is not a socket address: '{value}'"))?;
        }
        if let Ok(value) = std::env::var("COWBOT_SERVER_OWNER_NAME") {
            config.owner_name = value;
        }
        if let Ok(value) = std::env::var("COWBOT_SERVER_OWNER_EMAIL") {
            config.owner_email = value;
        }
        if let Ok(value) = std::env::var("COWBOT_SERVER_WORKSPACE") {
            config.workspace_name = value;
        }
        if let Ok(value) = std::env::var("COWBOT_SERVER_SESSION_HOURS") {
            config.session_hours = value
                .parse()
                .map_err(|_| format!("COWBOT_SERVER_SESSION_HOURS is not a number: '{value}'"))?;
        }
        if let Ok(value) = std::env::var("COWBOT_SERVER_LOGIN_ATTEMPTS") {
            config.login_attempts = value
                .parse()
                .map_err(|_| format!("COWBOT_SERVER_LOGIN_ATTEMPTS is not a number: '{value}'"))?;
        }
        if let Ok(value) = std::env::var("COWBOT_SERVER_LOGIN_WINDOW_SECS") {
            config.login_window_secs = value.parse().map_err(|_| {
                format!("COWBOT_SERVER_LOGIN_WINDOW_SECS is not a number: '{value}'")
            })?;
        }
        if let Ok(value) = std::env::var("COWBOT_SERVER_WORKSPACE_DIR") {
            config.workspace_dir = PathBuf::from(value);
        }
        config.llm = ServerLlmConfig::from_env()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_keep_a_single_machine_trial_working() {
        let config = ServerConfig::default();
        assert_eq!(config.database, PathBuf::from("cowbot-server.db"));
        assert_eq!(config.address.port(), 8787);
        assert_eq!(config.session_hours, 24 * 14);
        // The default binds to loopback, so a fresh deployment is not exposed.
        assert!(config.address.ip().is_loopback());
    }

    #[test]
    fn a_mistyped_address_is_an_error_rather_than_a_silent_default() {
        // The parse path itself; the environment read is exercised by the
        // server's start-up test.
        let parsed = "not-an-address".parse::<SocketAddr>();
        assert!(parsed.is_err());
    }

    #[test]
    fn no_llm_variables_means_no_turn_execution() {
        // The regression that matters most: a deployment that sets none of the
        // four variables behaves exactly as it did before the executor existed.
        let config = ServerLlmConfig::from_values(None, None, None, None, None, None).unwrap();
        assert!(config.is_none());
    }

    #[test]
    fn a_partial_llm_configuration_is_an_error() {
        // A remote provider with no endpoint or no model would otherwise run
        // turns that fail on every request; refuse to start instead.
        let missing_endpoint = ServerLlmConfig::from_values(
            Some("openai-chat".into()),
            None,
            Some("key".into()),
            Some("gpt-4o".into()),
            None,
            None,
        );
        assert!(missing_endpoint.unwrap_err().contains("BASE_URL"));

        let missing_model = ServerLlmConfig::from_values(
            Some("openai-chat".into()),
            Some("http://x".into()),
            None,
            None,
            None,
            None,
        );
        assert!(missing_model.unwrap_err().contains("MODEL"));

        let missing_protocol = ServerLlmConfig::from_values(
            None,
            Some("http://x".into()),
            None,
            Some("m".into()),
            None,
            None,
        );
        assert!(missing_protocol.unwrap_err().contains("PROTOCOL"));
    }

    #[test]
    fn a_mock_needs_neither_endpoint_nor_model() {
        // The only configuration that runs without a key, and therefore the one
        // every executor test uses.
        let config =
            ServerLlmConfig::from_values(Some("mock".into()), None, None, None, None, None)
                .unwrap()
                .expect("a mock is a configured LLM");
        assert_eq!(config.protocol, ServerLlmProtocol::Mock);
        assert_eq!(config.model, "mock");
        assert!(config.base_url.is_none());
    }

    #[test]
    fn a_declared_window_reaches_the_capacity_check() {
        // Without this the server cannot enforce a budget at all: the service
        // skips the check when the caller passes no window, so an operator who
        // sets these two variables is the only way a server turn is bounded.
        let config = ServerLlmConfig::from_values(
            Some("openai-chat".into()),
            Some("https://api.openai.com/v1".into()),
            Some("key".into()),
            Some("gpt-4o".into()),
            Some(128_000),
            Some(4_096),
        )
        .unwrap()
        .expect("a configured LLM");
        assert_eq!(config.context_window, Some(128_000));
        assert_eq!(config.max_output_tokens, Some(4_096));
    }

    #[test]
    fn a_mistyped_window_is_an_error_rather_than_a_silent_skip() {
        // A window that quietly fails to parse would disable the capacity
        // check without the operator noticing.
        let error = parse_optional_number("COWBOT_SERVER_LLM_CONTEXT_WINDOW", Some("128k".into()))
            .unwrap_err();
        assert!(error.contains("128k"));
        assert!(error.contains("COWBOT_SERVER_LLM_CONTEXT_WINDOW"));
        assert_eq!(parse_optional_number("X", None).unwrap(), None);
    }

    #[test]
    fn a_remote_provider_keeps_the_endpoint_and_model_it_was_given() {
        let config = ServerLlmConfig::from_values(
            Some("Anthropic".into()),
            Some("https://api.anthropic.com/v1".into()),
            Some("secret".into()),
            Some("claude-sonnet-5-5".into()),
            None,
            None,
        )
        .unwrap()
        .expect("a remote provider is a configured LLM");
        assert_eq!(config.protocol, ServerLlmProtocol::Anthropic);
        assert_eq!(config.model, "claude-sonnet-5-5");
        assert_eq!(
            config.base_url.as_deref(),
            Some("https://api.anthropic.com/v1")
        );
        // The key stays available to the builder but is never serialised; this
        // assertion pins that it survives the parse rather than the default.
        assert_eq!(config.api_key.as_deref(), Some("secret"));
    }

    #[test]
    fn an_unknown_protocol_names_what_is_accepted() {
        let error =
            ServerLlmConfig::from_values(Some("gemini".into()), None, None, None, None, None)
                .unwrap_err();
        assert!(error.contains("gemini"));
        assert!(error.contains("openai-chat"));
        assert!(error.contains("mock"));
    }

    #[test]
    fn protocol_names_round_trip() {
        for protocol in [
            ServerLlmProtocol::OpenAiChat,
            ServerLlmProtocol::Anthropic,
            ServerLlmProtocol::OpenAiResponses,
            ServerLlmProtocol::Mock,
        ] {
            assert_eq!(
                ServerLlmProtocol::parse(protocol.as_str()).unwrap(),
                protocol
            );
        }
        // A short alias an operator is likely to type.
        assert_eq!(
            ServerLlmProtocol::parse("responses").unwrap(),
            ServerLlmProtocol::OpenAiResponses
        );
        assert_eq!(
            ServerLlmProtocol::parse("openai").unwrap(),
            ServerLlmProtocol::OpenAiChat
        );
    }
}
