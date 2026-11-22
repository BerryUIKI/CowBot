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

    // A container sets the environment; a local run falls back to the defaults.
    let config = match ServerConfig::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("configuration error: {message}");
            std::process::exit(2);
        }
    };
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

    let state = AppState::new(store.clone(), auth, cowbot_bus::EventBus::default_bus());
    // The running integrations outlive this scope so a later stop control could
    // reach them; nothing holds a lock while the server serves requests.
    let integrations = tokio::sync::Mutex::new(None);
    let state = match &config.llm {
        Some(llm) => {
            let executor = cowbot_server::TurnExecutor::new(
                store.clone(),
                state.bus.clone(),
                llm,
                &config.workspace_dir,
            )
            .await?;
            tracing::info!(
                protocol = llm.protocol.as_str(),
                model = %llm.model,
                workspace_dir = %config.workspace_dir.display(),
                "Server will answer channel turns"
            );
            // A bot has to be running before anyone messages it, so the
            // integrations start here rather than from a route. They are only
            // started when a provider exists: a bot that stores a question and
            // never answers is worse than one that is not running, because the
            // IM user sees their message accepted and waits (ADR-0057).
            //
            // The handles are kept for the life of the process. Dropping one does
            // not stop the tasks — the cancel signal is owned, not a guard — but
            // a handle that is reachable is one an operator-facing stop control
            // could later act on.
            match cowbot_server::integration_startup::start_all(
                &store,
                &state.bus,
                executor.clone(),
            )
            .await
            {
                Ok(started) => {
                    *integrations.lock().await = Some(started);
                }
                Err(error) => {
                    // A database read that fails here must not stop the HTTP
                    // surface: a server that cannot start its bots still serves
                    // the workspace.
                    tracing::warn!(%error, "IM integrations did not start");
                }
            }
            state.with_turns(executor)
        }
        None => {
            tracing::info!(
                "No COWBOT_SERVER_LLM_* variables set: channel turns are stored but not answered, \
                 so no IM integration is started either"
            );
            state
        }
    };
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
