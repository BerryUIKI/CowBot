use std::io::Read;

use chrono::Utc;
use cowbot_core::agent::AgentProfile;
use cowbot_core::audit::AuditEvent;
use cowbot_core::channel::{Channel, ChannelMessage};
use cowbot_core::conversation::{
    ChannelParticipant, ContextEntry, ContextEvent, ConversationSettings,
};
use cowbot_core::identity::{Role, User, UserId, Workspace, WorkspaceId};
use cowbot_core::permission::{PermissionRule, RuleSource};
use cowbot_core::run::Run;
use cowbot_core::task::Task;
use cowbot_core::template::TemplateDocument;
use cowbot_store::Store;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::export::MAX_PACKAGE_BYTES;
use crate::manifest::{
    Manifest, PackageError, PackageKind, WorkspaceHeader, AGENTS_PATH, AUDIT_PATH, CHANNELS_PATH,
    CONTEXT_ENTRIES_PATH, CONTEXT_EVENTS_PATH, CONVERSATION_SETTINGS_PATH, MANIFEST_PATH,
    MESSAGES_PATH, PARTICIPANTS_PATH, PERMISSION_RULES_PATH, RUNS_PATH, TASKS_PATH, TEMPLATES_PATH,
    WORKSPACE_PATH,
};

/// Result of importing a package into a fresh local workspace.
#[derive(Debug, Clone)]
pub struct ImportSummary {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub channels: usize,
    pub messages: usize,
    pub agents: usize,
    pub tasks: usize,
    pub runs: usize,
    pub audit_events: usize,
    pub conversations: usize,
    pub context_entries: usize,
    pub permission_rules: usize,
}

/// Read and verify a package without importing it.
pub fn inspect(bytes: &[u8]) -> Result<Manifest, PackageError> {
    read_verified_entries(bytes).map(|(manifest, _)| manifest)
}

/// Import a workspace package. The archive is fully verified before any
/// write happens, so a bad package leaves no partial state.
pub async fn import_workspace(store: &Store, bytes: &[u8]) -> Result<ImportSummary, PackageError> {
    let (manifest, entries) = read_verified_entries(bytes)?;
    if manifest.kind != PackageKind::Workspace {
        return Err(PackageError::Invalid(
            "expected a workspace package".to_string(),
        ));
    }

    let header: WorkspaceHeader = parse(required(&entries, WORKSPACE_PATH)?)?;
    let agents: Vec<AgentProfile> = parse(required(&entries, AGENTS_PATH)?)?;
    let templates: Vec<TemplateDocument> = parse(required(&entries, TEMPLATES_PATH)?)?;
    let channels: Vec<Channel> = parse_jsonl(required(&entries, CHANNELS_PATH)?)?;
    let mut messages: Vec<ChannelMessage> = parse_jsonl(required(&entries, MESSAGES_PATH)?)?;
    let participants: Vec<ChannelParticipant> =
        optional_jsonl(&entries, PARTICIPANTS_PATH)?.unwrap_or_default();
    let conversation_settings: Vec<ConversationSettings> =
        optional_jsonl(&entries, CONVERSATION_SETTINGS_PATH)?.unwrap_or_default();
    let context_entries: Vec<ContextEntry> =
        optional_jsonl(&entries, CONTEXT_ENTRIES_PATH)?.unwrap_or_default();
    let context_events: Vec<ContextEvent> =
        optional_jsonl(&entries, CONTEXT_EVENTS_PATH)?.unwrap_or_default();
    let tasks: Vec<Task> = parse(required(&entries, TASKS_PATH)?)?;
    let runs: Vec<Run> = parse_jsonl(required(&entries, RUNS_PATH)?)?;
    let audit: Vec<AuditEvent> = parse_jsonl(required(&entries, AUDIT_PATH)?)?;
    let permission_rules: Vec<PermissionRule> =
        optional_jsonl(&entries, PERMISSION_RULES_PATH)?.unwrap_or_default();

    // Give the import a fresh identity so it never collides with local rows.
    let workspace_id = WorkspaceId::random();
    let user_id = UserId::random();
    let owner = User::new(
        if manifest.exported_by.user_name.is_empty() {
            "Imported owner"
        } else {
            &manifest.exported_by.user_name
        },
        format!("{}@imported.local", user_id.0),
        "",
    );

    store
        .create_user(&User {
            id: user_id.clone(),
            ..owner
        })
        .await?;
    store
        .create_workspace(&Workspace {
            id: workspace_id.clone(),
            name: format!("{} (imported)", header.name),
            created_at: header.created_at,
        })
        .await?;
    store
        .add_member(&cowbot_core::identity::Membership::new(
            user_id,
            workspace_id.clone(),
            Role::Owner,
        ))
        .await?;

    for agent in &agents {
        store.upsert_agent(&workspace_id, agent).await?;
    }
    for document in &templates {
        store
            .upsert_template(&cowbot_core::template::Template::new(
                document.kind,
                document.name.clone(),
                serde_json::to_value(document)?,
            ))
            .await?;
    }

    let channel_ids: std::collections::HashSet<_> =
        channels.iter().map(|channel| channel.id.clone()).collect();
    for channel in &channels {
        store.create_channel(&workspace_id, channel).await?;
    }
    for participant in participants
        .iter()
        .filter(|participant| channel_ids.contains(&participant.channel_id))
    {
        store.insert_channel_participant(participant).await?;
    }
    for settings in conversation_settings
        .iter()
        .filter(|settings| channel_ids.contains(&settings.channel_id))
    {
        store.insert_conversation_settings(settings).await?;
    }
    messages.retain(|message| channel_ids.contains(&message.channel_id));
    messages.sort_by_key(|message| message.created_at);
    for message in &messages {
        store.append_message(message).await?;
    }
    for entry in context_entries
        .iter()
        .filter(|entry| channel_ids.contains(&entry.channel_id))
    {
        store.insert_context_entry(entry).await?;
    }
    for event in context_events
        .iter()
        .filter(|event| channel_ids.contains(&event.channel_id))
    {
        store.insert_context_event(event).await?;
    }

    for task in &tasks {
        store
            .create_task(&workspace_id, task.channel_id.as_ref(), task)
            .await?;
    }
    for run in &runs {
        store.create_run(run).await?;
    }
    for event in &audit {
        // Audit rows carry the exporting workspace id and a globally unique
        // event id; re-key both for the imported workspace.
        let imported = AuditEvent {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.clone(),
            ..event.clone()
        };
        store.append_audit_event(&imported).await?;
    }
    // Imported rules start disabled: the user reviews each one before it can
    // affect a decision.
    for rule in &permission_rules {
        let imported = PermissionRule {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.clone(),
            tool: rule.tool.clone(),
            command: rule.command.clone(),
            decision: rule.decision,
            enabled: false,
            source: RuleSource::Imported,
            created_at: rule.created_at,
            updated_at: Utc::now(),
        };
        store.upsert_permission_rule(&imported).await?;
    }

    Ok(ImportSummary {
        workspace_id,
        workspace_name: format!("{} (imported)", header.name),
        channels: channels.len(),
        messages: messages.len(),
        agents: agents.len(),
        tasks: tasks.len(),
        runs: runs.len(),
        audit_events: audit.len(),
        conversations: conversation_settings.len(),
        context_entries: context_entries.len(),
        permission_rules: permission_rules.len(),
    })
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, PackageError> {
    Ok(serde_json::from_slice(bytes)?)
}

fn parse_jsonl<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<Vec<T>, PackageError> {
    let mut values = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        values.push(serde_json::from_slice(line)?);
    }
    Ok(values)
}

/// Parse a file introduced after the first package format version. Packages
/// written before conversation sessions existed simply omit it.
fn optional_jsonl<T: serde::de::DeserializeOwned>(
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
    path: &str,
) -> Result<Option<Vec<T>>, PackageError> {
    entries
        .get(path)
        .map(|bytes| parse_jsonl(bytes))
        .transpose()
}

fn required<'a>(
    entries: &'a std::collections::BTreeMap<String, Vec<u8>>,
    path: &str,
) -> Result<&'a [u8], PackageError> {
    entries
        .get(path)
        .map(Vec::as_slice)
        .ok_or_else(|| PackageError::MissingFile {
            path: path.to_string(),
        })
}

/// Known archive paths for the v1 layout.
fn known_paths() -> &'static [&'static str] {
    &[
        MANIFEST_PATH,
        WORKSPACE_PATH,
        AGENTS_PATH,
        TEMPLATES_PATH,
        CHANNELS_PATH,
        MESSAGES_PATH,
        PARTICIPANTS_PATH,
        CONVERSATION_SETTINGS_PATH,
        CONTEXT_ENTRIES_PATH,
        CONTEXT_EVENTS_PATH,
        TASKS_PATH,
        RUNS_PATH,
        AUDIT_PATH,
        PERMISSION_RULES_PATH,
    ]
}

/// Read every entry, reject unknown files, and check all checksums.
fn read_verified_entries(
    bytes: &[u8],
) -> Result<(Manifest, std::collections::BTreeMap<String, Vec<u8>>), PackageError> {
    if bytes.len() as u64 > MAX_PACKAGE_BYTES {
        return Err(PackageError::TooLarge {
            limit: MAX_PACKAGE_BYTES,
        });
    }

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut entries = std::collections::BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name()?.to_string();
        if !known_paths().contains(&name.as_str()) {
            return Err(PackageError::UnexpectedFile { path: name });
        }
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        entries.insert(name, contents);
    }

    let manifest_bytes = required(&entries, MANIFEST_PATH)?;
    let manifest: Manifest = serde_json::from_slice(manifest_bytes)?;
    manifest.validate()?;

    for (path, expected) in &manifest.checksums {
        let actual = entries
            .get(path)
            .ok_or_else(|| PackageError::MissingFile { path: path.clone() })?;
        let digest = Sha256::digest(actual);
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        if &hex != expected {
            return Err(PackageError::ChecksumMismatch { path: path.clone() });
        }
    }
    for path in entries.keys() {
        if path != MANIFEST_PATH && !manifest.checksums.contains_key(path) {
            return Err(PackageError::ChecksumMismatch { path: path.clone() });
        }
    }

    Ok((manifest, entries))
}
