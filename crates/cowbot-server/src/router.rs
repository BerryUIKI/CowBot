//! The HTTP router and its shared state.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use cowbot_store::Store;

use crate::api;
use crate::auth::{AuthService, AuthenticatedUser};
use crate::error::ApiError;

/// Shared state every handler sees.
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub auth: Arc<AuthService>,
}

impl AppState {
    pub fn new(store: Store, auth: Arc<AuthService>) -> Self {
        Self { store, auth }
    }
}

/// Build the API router.
///
/// The authenticated routes take their identity from [`AuthenticatedUser`],
/// which extracts and verifies the bearer token itself, so a handler cannot
/// accidentally run without proving who is asking.
pub fn app(state: AppState) -> Router {
    let public = Router::new()
        .route("/health", get(api::health))
        .route("/api/v1/auth/sign-in", post(api::sign_in));

    let authenticated = Router::new()
        .route("/api/v1/me", get(api::me))
        .route("/api/v1/auth/sign-out", post(api::sign_out))
        .route("/api/v1/auth/password", post(api::change_password));

    public
        .merge(authenticated)
        .layer(middleware::from_fn(log_request))
        .with_state(state)
}

/// Log each request's outcome at debug level, so a deployment has a trail
/// without a verbose default.
async fn log_request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    if response.status().is_server_error() {
        tracing::warn!(%method, %path, status = %response.status(), "Request failed");
    } else {
        tracing::debug!(%method, %path, status = %response.status(), "Request served");
    }
    response
}

/// Resolve the bearer token to the signed-in user.
///
/// Implemented as an extractor rather than a middleware so a handler's
/// signature says whether it needs an identity.
impl FromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(token) = api::bearer_token(&parts.headers) else {
            return Err(ApiError::unauthorized(
                "a bearer token is required; sign in first",
            ));
        };
        match state.auth.authenticate(token).await {
            Ok(Some(user)) => Ok(user),
            Ok(None) => Err(ApiError::unauthorized(
                "the session is expired or unknown; sign in again",
            )),
            Err(error) => Err(error.into()),
        }
    }
}

/// Serve the router on an existing listener until the future is dropped.
pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    let router = app(state);
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
}

/// A status assistant so a caller can tell "not found" from "unauthorized"
/// without parsing.
pub fn status_code(error: &ApiError) -> StatusCode {
    error.status
}
