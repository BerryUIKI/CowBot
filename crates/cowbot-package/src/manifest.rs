use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Package format identifier written into every manifest.
pub const PACKAGE_FORMAT: &str = "cowbot-package";

/// Manifest schema version. Increases monotonically; the importer supports
/// exactly the version it writes.
pub const PACKAGE_SCHEMA_VERSION: u32 = 1;

/// Archive paths used by the v1 package layout.
pub const MANIFEST_PATH: &str = "manifest.json";
pub const WORKSPACE_PATH: &str = "workspace.json";
pub const AGENTS_PATH: &str = "agents.json";
pub const TEMPLATES_PATH: &str = "templates.json";
pub const CHANNELS_PATH: &str = "channels.jsonl";
pub const MESSAGES_PATH: &str = "messages.jsonl";
pub const PARTICIPANTS_PATH: &str = "channel_participants.jsonl";
pub const CONVERSATION_SETTINGS_PATH: &str = "conversation_settings.jsonl";
pub const CONTEXT_ENTRIES_PATH: &str = "context_entries.jsonl";
pub const CONTEXT_EVENTS_PATH: &str = "context_events.jsonl";
pub const TASKS_PATH: &str = "tasks.json";
pub const RUNS_PATH: &str = "runs.jsonl";
pub const AUDIT_PATH: &str = "audit.jsonl";

/// What a package contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    Workspace,
    Template,
}

/// Root document of a `.cowpkg` archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub schema_version: u32,
    pub kind: PackageKind,
    pub app_version: String,
    pub created_at: DateTime<Utc>,
    pub exported_by: ExportedBy,
    /// Archive path to lowercase hex SHA-256 of the file contents.
    pub checksums: std::collections::BTreeMap<String, String>,
}

impl Manifest {
    pub fn new(kind: PackageKind, app_version: impl Into<String>, exported_by: ExportedBy) -> Self {
        Self {
            format: PACKAGE_FORMAT.to_string(),
            schema_version: PACKAGE_SCHEMA_VERSION,
            kind,
            app_version: app_version.into(),
            created_at: Utc::now(),
            exported_by,
            checksums: Default::default(),
        }
    }

    pub fn validate(&self) -> Result<(), PackageError> {
        if self.format != PACKAGE_FORMAT {
            return Err(PackageError::Invalid(format!(
                "unexpected package format '{}' (expected '{}')",
                self.format, PACKAGE_FORMAT
            )));
        }
        if self.schema_version != PACKAGE_SCHEMA_VERSION {
            return Err(PackageError::UnsupportedVersion {
                found: self.schema_version,
                expected: PACKAGE_SCHEMA_VERSION,
            });
        }
        Ok(())
    }
}

/// Who produced the package. Never carries credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedBy {
    pub user_id: String,
    pub user_name: String,
    pub workspace_id: String,
    pub workspace_name: String,
}

/// Workspace header stored in `workspace.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceHeader {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// Errors raised while building or reading a package.
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("invalid package: {0}")]
    Invalid(String),
    #[error("unsupported package schema version {found} (this build supports {expected})")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("package file '{path}' is missing")]
    MissingFile { path: String },
    #[error("checksum mismatch for '{path}'")]
    ChecksumMismatch { path: String },
    #[error("package contains an unexpected file '{path}'")]
    UnexpectedFile { path: String },
    #[error("package exceeds the {limit} byte limit")]
    TooLarge { limit: u64 },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("store error: {0}")]
    Store(#[from] cowbot_core::error::CowError),
}
