//! `cowbot-server`: the self-hosted team-mode server.
//!
//! Starts the HTTP API over a SQLite database. On an empty database it creates
//! the owner account and prints its password once; the operator changes it
//! from the first session. On an existing database it starts unchanged.

use std::sync::Arc;

use cowbot_server::auth::AuthService;
use cowbot_server::config::ServerConfig;
use cowbot_server::{app, AppState};
use cowbot_store::Store;

/// A generated owner password, long enough for the server's own minimum.
fn generated_password() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut bytes = [0u8; 18];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cowbot_server=info,tower_http=info".into()),
        )
        .init();

    let config = ServerConfig::default();
    let store = Store::connect(&config.database).await?;
    store.migrate().await?;

    let auth = Arc::new(AuthService::new(store.clone(), config.clone()));
    let password = generated_password();
    if let Some(owner) = auth.bootstrap(&password).await? {
        // Printed once: the server has no mail delivery, and the operator needs
        // a way in on a fresh deployment.
        println!("Created owner account {}", owner.email);
        println!("  password: {password}");
        println!("  Change it from POST /api/v1/auth/password after signing in.");
    }

    let state = AppState::new(store, auth, cowbot_bus::EventBus::default_bus());
    let listener = tokio::net::TcpListener::bind(config.address).await?;
    tracing::info!(
        address = %config.address,
        database = %config.database.display(),
        "CowBot server listening"
    );
    axum::serve(
        listener,
        app(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
