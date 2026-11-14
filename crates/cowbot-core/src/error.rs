use thiserror::Error;

/// Core error types for the CowBot platform.
#[derive(Error, Debug)]
pub enum CowError {
    #[error("Agent error: {0}")]
    Agent(String),

    #[error("Task error: {0}")]
    Task(String),

    #[error("Tool execution error in '{name}': {message}")]
    ToolExecution { name: String, message: String },

    #[error("Tool '{0}' not found")]
    ToolNotFound(String),

    #[error("LLM provider error: {0}")]
    Llm(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Workspace package error: {0}")]
    Package(String),

    /// The requested record does not exist, or is not visible to the caller.
    ///
    /// Callers that expose an API turn this into a 404 rather than an
    /// invalid-request error.
    #[error("{0}")]
    NotFound(String),

    /// Too many attempts in a short window; the caller should retry later.
    #[error("{0}")]
    RateLimited(String),

    /// An email and password pair did not match. Distinct from a missing or
    /// expired session, which the API reports as unauthorized too.
    #[error("{0}")]
    InvalidCredentials(String),

    /// The work was stopped on purpose, for example by a turn cancellation.
    ///
    /// A cancelled turn is not a failure: callers persist what was produced and
    /// report it as stopped rather than reporting an error.
    #[error("Cancelled")]
    Cancelled,
}

pub type CowResult<T> = Result<T, CowError>;
