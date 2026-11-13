//! Self-hosted server mode.
//!
//! Phase 2 milestone 2.1 starts here: an HTTP surface over the same
//! `cowbot-store` database the desktop uses, with password accounts, opaque
//! bearer sessions, and a workspace-bootstrapped owner (ADR-0043).
//!
//! The server deliberately reuses the local schema and repositories. A team
//! deployment is the same data model reachable over the network, not a second
//! one; `cowbot-client`'s remote mode is the later milestone that speaks this
//! API.

pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod router;
pub mod workspaces;

pub use auth::{AuthService, AuthenticatedUser, IssuedSession};
pub use config::ServerConfig;
pub use error::ApiError;
pub use router::{app, AppState};
