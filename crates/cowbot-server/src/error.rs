//! The API error model.
//!
//! Every handler error becomes one JSON body with a stable `code`, so a remote
//! client branches on the code rather than parsing a message. The `message` is
//! safe to show; a storage failure is logged and replaced with a generic one,
//! because a database error can name a table or a row.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cowbot_core::error::CowError;
use serde::{Deserialize, Serialize};

/// One API failure.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

/// The wire shape of an error.
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(ErrorBody {
            code: self.code.to_string(),
            message: self.message,
        });
        (self.status, body).into_response()
    }
}

impl From<CowError> for ApiError {
    fn from(error: CowError) -> Self {
        match error {
            CowError::Config(message) => ApiError::bad_request(message),
            CowError::Cancelled => ApiError::new(
                StatusCode::CONFLICT,
                "cancelled",
                "the request was cancelled",
            ),
            // A storage failure can leak table and column names, so it is
            // logged for the operator and reported generically.
            CowError::Storage(message) => {
                tracing::error!(error = %message, "Storage failure while serving a request");
                ApiError::internal("the server could not complete the request")
            }
            other => {
                tracing::warn!(error = %other, "Request failed");
                ApiError::internal(other.to_string())
            }
        }
    }
}
