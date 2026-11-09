use std::collections::BTreeMap;
use std::io::Write;

use cowbot_core::identity::{Workspace, WorkspaceId};
use cowbot_core::template::TemplateDocument;
use cowbot_store::Store;
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

use crate::manifest::{
    ExportedBy, Manifest, PackageError, PackageKind, WorkspaceHeader, AGENTS_PATH, AUDIT_PATH,
    CHANNELS_PATH, CONTEXT_ENTRIES_PATH, CONTEXT_EVENTS_PATH, CONVERSATION_SETTINGS_PATH,
    MANIFEST_PATH, MESSAGES_PATH, PARTICIPANTS_PATH, RUNS_PATH, TASKS_PATH, TEMPLATES_PATH,
    WORKSPACE_PATH,
};

/// Upper bound for a package produced or read in one go.
pub const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

/// How many newest messages to pull per channel.
const MESSAGES_PER_CHANNEL: u32 = 10_000;
/// How many newest audit events to include.
const AUDIT_EVENT_LIMIT: u32 = 10_000;

/// Build the bytes of a workspace package.
pub async fn export_workspace(
    store: &Store,
    workspace_id: &WorkspaceId,
) -> Result<Vec<u8>, PackageError> {
    let workspace = store.get_workspace(workspace_id).await?.ok_or_else(|| {
        PackageError::Invalid(format!("workspace '{workspace_id:?}' was not found"))
    })?;
    let owner_id = store
        .list_members(workspace_id)
        .await?
        .into_iter()
        .next()
        .map(|membership| membership.user_id.0)
        .unwrap_or_default();

    let agents = store.list_agents(workspace_id).await?;
    let templates: Vec<TemplateDocument> = store
        .list_templates(None)
        .await?
        .into_iter()
        .filter_map(|stored| TemplateDocument::from_stored(&stored).ok())
        .collect();
    let channels = store.list_channels(workspace_id).await?;

    let mut messages = Vec::new();
    for channel in &channels {
        messages.extend(
            store
                .list_messages(&channel.id, MESSAGES_PER_CHANNEL)
                .await?,
        );
    }

    let tasks = store.list_tasks(workspace_id).await?;
    let mut runs = Vec::new();
    for task in &tasks {
        runs.extend(store.list_runs(&task.id).await?);
    }

    let participants = store.list_all_channel_participants(workspace_id).await?;
    let conversation_settings = store
        .list_conversation_settings_for_workspace(workspace_id)
        .await?;
    let context_entries = store.list_all_context_entries(workspace_id).await?;
    let context_events = store.list_all_context_events(workspace_id).await?;

    let audit = store
        .list_audit_events(workspace_id, AUDIT_EVENT_LIMIT)
        .await?;

    let mut entries: BTreeMap<&str, Vec<u8>> = BTreeMap::new();
    entries.insert(
        WORKSPACE_PATH,
        to_bytes(&WorkspaceHeader::from(&workspace))?,
    );
    entries.insert(AGENTS_PATH, to_bytes(&agents)?);
    entries.insert(TEMPLATES_PATH, to_bytes(&templates)?);
    entries.insert(CHANNELS_PATH, to_jsonl(&channels)?);
    entries.insert(MESSAGES_PATH, to_jsonl(&messages)?);
    entries.insert(PARTICIPANTS_PATH, to_jsonl(&participants)?);
    entries.insert(CONVERSATION_SETTINGS_PATH, to_jsonl(&conversation_settings)?);
    entries.insert(CONTEXT_ENTRIES_PATH, to_jsonl(&context_entries)?);
    entries.insert(CONTEXT_EVENTS_PATH, to_jsonl(&context_events)?);
    entries.insert(TASKS_PATH, to_bytes(&tasks)?);
    entries.insert(RUNS_PATH, to_jsonl(&runs)?);
    entries.insert(AUDIT_PATH, to_jsonl(&audit)?);

    let mut manifest = Manifest::new(
        PackageKind::Workspace,
        env!("CARGO_PKG_VERSION"),
        ExportedBy {
            user_id: owner_id,
            user_name: String::new(),
            workspace_id: workspace.id.0.clone(),
            workspace_name: workspace.name.clone(),
        },
    );
    for (path, bytes) in &entries {
        manifest
            .checksums
            .insert((*path).to_string(), sha256_hex(bytes));
    }
    let manifest_bytes = to_bytes(&manifest)?;

    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    archive.start_file(MANIFEST_PATH, options)?;
    archive.write_all(&manifest_bytes)?;
    for (path, bytes) in &entries {
        archive.start_file(*path, options)?;
        archive.write_all(bytes)?;
    }
    let cursor = archive.finish()?;
    let bytes = cursor.into_inner();
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err(PackageError::TooLarge {
            limit: MAX_PACKAGE_BYTES,
        });
    }
    Ok(bytes)
}

impl From<&Workspace> for WorkspaceHeader {
    fn from(workspace: &Workspace) -> Self {
        Self {
            id: workspace.id.0.clone(),
            name: workspace.name.clone(),
            created_at: workspace.created_at,
        }
    }
}

fn to_bytes<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, PackageError> {
    Ok(serde_json::to_vec_pretty(value)?)
}

fn to_jsonl<T: serde::Serialize>(values: &[T]) -> Result<Vec<u8>, PackageError> {
    let mut out = Vec::new();
    for value in values {
        serde_json::to_writer(&mut out, value)?;
        out.push(b'\n');
    }
    Ok(out)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
