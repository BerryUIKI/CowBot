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
