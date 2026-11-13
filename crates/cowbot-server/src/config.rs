//! Server configuration.

use std::net::SocketAddr;
use std::path::PathBuf;

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
}
