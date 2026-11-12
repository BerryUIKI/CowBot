//! Persisted, redacted prompt snapshots for LLM call audit (ADR-0037).
//!
//! A snapshot records what a call actually sent, at the moment it was sent.
//! Unlike [`crate::prompt::PromptPlan`], which rebuilds the *next* request for
//! the context inspector, a snapshot is an immutable audit record: settings,
//! memory, compaction, tool lists, and the model may all change between the
//! call and the review, so a rebuilt prompt cannot stand in for the original.
//!
//! Snapshots are sealed before storage. Sealing redacts credentials with the
//! same [`redact_secrets`] path memory uses, bounds the payload, counts tokens,
//! and hashes exactly the content that was stored. Every step is deterministic,
//! so the same call seals to the same hash.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent::AgentId;
use crate::channel::ChannelId;
use crate::conversation::ContextProjection;
use crate::identity::WorkspaceId;
use crate::memory::{
    bearer_token_span, is_secret_token, is_sensitive_key, trim_quotes, word_spans,
    TRUNCATION_MARKER,
};
use crate::prompt::{PromptMemory, PromptRole, PromptSlot};
use crate::run::RunId;
use crate::task::TaskId;
use crate::tool::ToolDefinition;
use crate::usage::{LlmCallKind, TokenCounter};

/// Version of the snapshot document a row carries.
///
/// Readers must refuse a version they do not know rather than guess at field
/// meanings; the desktop renders only the versions it ships with.
pub const PROMPT_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Largest stored payload, in bytes.
///
/// A snapshot is audit evidence, not a transcript archive. Sealing keeps the
/// document under this bound and records that it did so.
pub const PROMPT_SNAPSHOT_MAX_BYTES: usize = 256 * 1024;

/// Largest single message body kept before the payload bound drops messages.
pub const PROMPT_SNAPSHOT_MESSAGE_MAX_CHARS: usize = 8 * 1024;

/// Default retention for stored snapshots.
pub const DEFAULT_PROMPT_SNAPSHOT_RETENTION_DAYS: u32 = 30;
/// Shortest selectable retention.
pub const MIN_PROMPT_SNAPSHOT_RETENTION_DAYS: u32 = 1;
/// Longest selectable retention.
pub const MAX_PROMPT_SNAPSHOT_RETENTION_DAYS: u32 = 3_650;

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// One tool call carried by an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// One message exactly as the provider received it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotMessage {
    pub role: PromptRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<SnapshotToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl SnapshotMessage {
    pub fn new(role: PromptRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            name: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_tool_calls(mut self, tool_calls: Vec<SnapshotToolCall>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    pub fn with_tool_call_id(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = Some(tool_call_id.into());
        self
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// A sealed, redacted record of one model request.
///
/// The row that stores this is keyed by the LLM call it belongs to; the call
/// row carries the run, task, channel, and agent. The model and call kind are
/// repeated here so the document stands on its own after a package export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmPromptSnapshot {
    pub schema_version: u32,
    pub call_kind: LlmCallKind,
    pub model: String,
    /// Messages in send order.
    pub messages: Vec<SnapshotMessage>,
    /// Tool schemas sent with the request.
    pub tools: Vec<ToolDefinition>,
    /// Named system-prompt slots, when the caller assembles the prompt that
    /// way. Router, summary, compaction, and extraction calls usually have
    /// none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<PromptSlot>,
    /// The injected memory block, when one was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<PromptMemory>,
    /// Memory entry ids the recall selected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_memory_ids: Vec<String>,
    /// The context breakdown behind the request, when the caller has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<ContextProjection>,
    /// Tokens the stored request occupies, counted when the snapshot sealed.
    pub token_count: u32,
    /// SHA-256 of the stored document with this field cleared, lowercase hex.
    pub content_hash: String,
    /// True when sealing removed at least one credential-shaped value.
    pub redacted: bool,
    /// True when sealing dropped or shortened content to fit the bound.
    pub truncated: bool,
    /// Messages dropped by the payload bound.
    #[serde(default)]
    pub omitted_messages: u32,
    pub created_at: DateTime<Utc>,
}

impl LlmPromptSnapshot {
    /// Start an empty snapshot for one call.
    ///
    /// `created_at` defaults to now. A caller that records the time the call
    /// happened — not the time the snapshot was sealed — sets it before
    /// [`Self::seal`], because the hash covers the stored document.
    pub fn new(call_kind: LlmCallKind, model: impl Into<String>) -> Self {
        Self {
            schema_version: PROMPT_SNAPSHOT_SCHEMA_VERSION,
            call_kind,
            model: model.into(),
            messages: Vec::new(),
            tools: Vec::new(),
            slots: Vec::new(),
            memory: None,
            selected_memory_ids: Vec::new(),
            projection: None,
            token_count: 0,
            content_hash: String::new(),
            redacted: false,
            truncated: false,
            omitted_messages: 0,
            created_at: Utc::now(),
        }
    }

    /// Record the request messages in send order.
    pub fn with_messages(mut self, messages: Vec<SnapshotMessage>) -> Self {
        self.messages = messages;
        self
    }

    /// Record the tool schemas the request carried.
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }

    /// Record the named system-prompt slots of an assembled prompt.
    pub fn with_slots(mut self, slots: Vec<PromptSlot>) -> Self {
        self.slots = slots;
        self
    }

    /// Record the memory block and the entry ids the recall selected.
    pub fn with_memory(mut self, memory: PromptMemory, selected: Vec<String>) -> Self {
        self.memory = Some(memory);
        self.selected_memory_ids = selected;
        self
    }

    /// Record the context breakdown behind the request.
    pub fn with_projection(mut self, projection: ContextProjection) -> Self {
        self.projection = Some(projection);
        self
    }

    /// Redact, bound, count, and hash the snapshot.
    ///
    /// Sealing is idempotent on already-sealed content: redaction of clean text
    /// is a no-op, an in-bound document is unchanged, and the hash covers the
    /// stored bytes, so sealing twice yields the same hash.
    pub fn seal(mut self, counter: &dyn TokenCounter) -> Self {
        self.redact();
        self.bound();
        self.token_count = self.count_tokens(counter);
        self.content_hash = self.compute_hash();
        self
    }

    /// True when `content_hash` matches the document as stored.
    ///
    /// Stored rows are compared with this on read so a truncated or edited
    /// payload is reported rather than silently trusted.
    pub fn hash_matches(&self) -> bool {
        self.content_hash == self.compute_hash()
    }

    /// The JSON document to store.
    ///
    /// A snapshot that cannot serialize is a programming error, not a value a
    /// caller may silently store empty.
    pub fn payload(&self) -> String {
        serde_json::to_string(self).expect("a prompt snapshot always serializes")
    }

    /// Read a stored payload back.
    ///
    /// A document from a newer writer is refused because its field meanings
    /// cannot be guessed at; every version up to the one this build knows is
    /// accepted, so an upgrade never hides pre-upgrade evidence.
    pub fn from_payload(payload: &str) -> Result<Self, String> {
        let snapshot: Self = serde_json::from_str(payload).map_err(|error| error.to_string())?;
        if snapshot.schema_version == 0 || snapshot.schema_version > PROMPT_SNAPSHOT_SCHEMA_VERSION
        {
            return Err(format!(
                "unsupported prompt snapshot schema version {}",
                snapshot.schema_version
            ));
        }
        Ok(snapshot)
    }

    fn redact(&mut self) {
        let mut redacted = false;

        for message in &mut self.messages {
            redacted |= redact_snapshot_text(&mut message.content);
            for call in &mut message.tool_calls {
                redacted |= redact_json(&mut call.arguments);
            }
        }
        for slot in &mut self.slots {
            redacted |= redact_snapshot_text(&mut slot.content);
        }
        if let Some(memory) = &mut self.memory {
            redacted |= redact_snapshot_text(&mut memory.block);
        }
        for tool in &mut self.tools {
            redacted |= redact_snapshot_text(&mut tool.description);
            redacted |= redact_json(&mut tool.parameters);
        }

        // Sealing twice must not clear the flag: the hash covers it, and the
        // first pass already removed the credentials.
        self.redacted |= redacted;
    }

    fn bound(&mut self) {
        if self.payload().len() <= PROMPT_SNAPSHOT_MAX_BYTES {
            return;
        }
        self.truncated = true;

        for message in &mut self.messages {
            truncate_chars(&mut message.content, PROMPT_SNAPSHOT_MESSAGE_MAX_CHARS);
        }
        // Drop the oldest history first; the system prompt and the most recent
        // messages are what an audit reads.
        while self.payload().len() > PROMPT_SNAPSHOT_MAX_BYTES {
            let Some(index) = self
                .messages
                .iter()
                .position(|message| message.role != PromptRole::System)
            else {
                break;
            };
            let non_system = self
                .messages
                .iter()
                .filter(|message| message.role != PromptRole::System)
                .count();
            if non_system <= 1 {
                break;
            }
            self.messages.remove(index);
            self.omitted_messages = self.omitted_messages.saturating_add(1);
        }
        // Last resort: schemas and slot bodies cannot be dropped without
        // losing the little that still explains the request, so shorten them.
        if self.payload().len() > PROMPT_SNAPSHOT_MAX_BYTES {
            self.tools.clear();
            for slot in &mut self.slots {
                truncate_chars(&mut slot.content, PROMPT_SNAPSHOT_MESSAGE_MAX_CHARS);
            }
            if let Some(memory) = &mut self.memory {
                truncate_chars(&mut memory.block, PROMPT_SNAPSHOT_MESSAGE_MAX_CHARS);
            }
        }
        if self.payload().len() > PROMPT_SNAPSHOT_MAX_BYTES {
            for message in &mut self.messages {
                truncate_chars(&mut message.content, 1024);
            }
        }
    }

    fn count_tokens(&self, counter: &dyn TokenCounter) -> u32 {
        let messages = self.messages.iter().fold(0_u32, |total, message| {
            total.saturating_add(counter.count_message(&message.content))
        });
        let tools = serde_json::to_string(&self.tools)
            .map(|json| counter.count(&json))
            .unwrap_or(0);
        messages.saturating_add(tools)
    }

    fn compute_hash(&self) -> String {
        let mut value = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.remove("content_hash");
        }
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        hex(&Sha256::digest(&bytes))
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// Whether this workspace stores prompt snapshots, and for how long.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSnapshotSettings {
    pub workspace_id: WorkspaceId,
    /// Store a snapshot with every LLM call. Turning this off stops new
    /// snapshots; stored ones still expire on the retention clock.
    pub enabled: bool,
    /// Days a stored snapshot is kept before the retention sweep removes it.
    pub retention_days: u32,
    pub updated_at: DateTime<Utc>,
}

impl PromptSnapshotSettings {
    pub fn new(workspace_id: WorkspaceId) -> Self {
        Self {
            workspace_id,
            enabled: true,
            retention_days: DEFAULT_PROMPT_SNAPSHOT_RETENTION_DAYS,
            updated_at: Utc::now(),
        }
    }

    /// Set the retention window, clamped to the supported range.
    pub fn set_retention_days(&mut self, days: u32) {
        self.retention_days = clamp_retention_days(days);
        self.updated_at = Utc::now();
    }
}

/// Clamp a configured retention window into the supported range.
pub fn clamp_retention_days(value: u32) -> u32 {
    value.clamp(
        MIN_PROMPT_SNAPSHOT_RETENTION_DAYS,
        MAX_PROMPT_SNAPSHOT_RETENTION_DAYS,
    )
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// What a stored snapshot is, without its payload.
///
/// The row that stores a snapshot also stores the call it belongs to, so a
/// list of summaries answers "which calls have evidence, and what was sent"
/// without parsing every document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSnapshotSummary {
    pub llm_call_id: String,
    pub run_id: Option<RunId>,
    pub task_id: Option<TaskId>,
    pub channel_id: Option<ChannelId>,
    pub agent_id: Option<AgentId>,
    pub call_kind: LlmCallKind,
    pub model: String,
    /// Tokens the stored request occupies, counted when the snapshot sealed.
    pub token_count: u32,
    /// Size of the stored payload in bytes.
    pub byte_size: u32,
    pub truncated: bool,
    pub redacted: bool,
    pub created_at: DateTime<Utc>,
}

impl PromptSnapshotSummary {
    /// Describe a snapshot that was just read, without re-parsing the payload.
    pub fn from_snapshot(llm_call_id: impl Into<String>, snapshot: &LlmPromptSnapshot) -> Self {
        Self {
            llm_call_id: llm_call_id.into(),
            run_id: None,
            task_id: None,
            channel_id: None,
            agent_id: None,
            call_kind: snapshot.call_kind,
            model: snapshot.model.clone(),
            token_count: snapshot.token_count,
            byte_size: snapshot.payload().len() as u32,
            truncated: snapshot.truncated,
            redacted: snapshot.redacted,
            created_at: snapshot.created_at,
        }
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// A stored snapshot together with the call it belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSnapshotDetail {
    pub summary: PromptSnapshotSummary,
    pub snapshot: LlmPromptSnapshot,
    /// True when the stored payload no longer matches its content hash.
    ///
    /// A caller may still render it, but must label it as altered.
    #[serde(default)]
    pub hash_mismatch: bool,
}

/// Redact one prompt field.
///
/// Snapshots are the audit record of a real request, so redaction is stricter
/// than the memory path: every `key=value` / `key: value` pair whose key looks
/// sensitive is cleared — not only the first separator on a line, and only the
/// value rather than the rest of the line — bearer tokens are replaced, bare
/// tokens with a known credential prefix are replaced, and PEM blocks are
/// replaced whole. The result is still heuristic; retention is the second line
/// of defence.
fn redact_snapshot_text(text: &mut String) -> bool {
    let (stripped, pem) = redact_pem_blocks(text);
    *text = stripped;
    let mut changed = pem;
    changed |= map_lines(text, redact_sensitive_pairs);
    changed |= map_lines(text, redact_bearer_tokens);
    changed |= map_lines(text, redact_secret_tokens);
    changed
}

/// Apply one line-scoped redactor, preserving the original line endings.
fn map_lines(text: &mut String, redact: fn(&str) -> (String, bool)) -> bool {
    let mut changed = false;
    let mut rebuilt = String::with_capacity(text.len());
    for piece in text.split_inclusive('\n') {
        let (body, ending) = match piece.strip_suffix("\r\n") {
            Some(body) => (body, "\r\n"),
            None => match piece.strip_suffix('\n') {
                Some(body) => (body, "\n"),
                None => (piece, ""),
            },
        };
        let (line, line_changed) = redact(body);
        changed |= line_changed;
        rebuilt.push_str(&line);
        rebuilt.push_str(ending);
    }
    *text = rebuilt;
    changed
}

/// Replace the value of a `Bearer` scheme on a line.
fn redact_bearer_tokens(line: &str) -> (String, bool) {
    let Some((start, end)) = bearer_token_span(line) else {
        return (line.to_string(), false);
    };
    let mut value = line.to_string();
    value.replace_range(start..end, "[redacted]");
    (value, true)
}

/// Replace words that carry a known credential prefix or shape.
fn redact_secret_tokens(line: &str) -> (String, bool) {
    let mut rebuilt = String::with_capacity(line.len());
    let mut cursor = 0;
    let mut changed = false;
    for (start, end) in word_spans(line) {
        let word = &line[start..end];
        let inner = trim_quotes(word);
        if inner.is_empty() || inner == "[redacted]" || !is_secret_token(inner) {
            continue;
        }
        let inner_start = start + (inner.as_ptr() as usize - word.as_ptr() as usize);
        rebuilt.push_str(&line[cursor..inner_start]);
        rebuilt.push_str("[redacted]");
        cursor = inner_start + inner.len();
        changed = true;
    }
    if !changed {
        return (line.to_string(), false);
    }
    rebuilt.push_str(&line[cursor..]);
    (rebuilt, true)
}

/// Replace every `key=value` or `key: value` whose key looks like a credential.
///
/// [`redact_secrets`] only inspects the first separator on a line, which misses
/// `curl -H "X-Api-Key: sk-..." https://host/?token=...`. This pass walks every
/// separator and stops the value at whitespace and at URL, JSON, and shell
/// delimiters.
fn redact_sensitive_pairs(line: &str) -> (String, bool) {
    const KEY_DELIMITERS: &[char] = &[
        '"', '\'', '`', '(', '[', '{', ',', ';', '&', '?', '<', '|', '=',
    ];
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    let mut changed = false;
    let mut index = 0;

    while index < line.len() {
        let character = line[index..].chars().next().unwrap_or(' ');
        if character != '=' && character != ':' {
            index += character.len_utf8();
            continue;
        }

        // The key is the token immediately before the separator; trailing
        // whitespace (`password = ...`) must not empty it.
        let prefix = line[..index].trim_end();
        let key_start = prefix
            .rfind(|candidate: char| {
                candidate.is_whitespace() || KEY_DELIMITERS.contains(&candidate)
            })
            .map(|position| position + prefix[position..].chars().next().unwrap_or(' ').len_utf8())
            .unwrap_or(0);
        let key = &prefix[key_start..];
        if !is_sensitive_key(key) {
            index += character.len_utf8();
            continue;
        }

        let mut value_start = index + character.len_utf8();
        while value_start < line.len()
            && line[value_start..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            value_start += line[value_start..].chars().next().unwrap_or(' ').len_utf8();
        }
        // `Authorization: Bearer <token>` and `token: Basic <blob>` put the
        // scheme before the credential; redact the credential, not the scheme.
        let scheme = line[value_start..]
            .split(|candidate: char| candidate.is_whitespace())
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if matches!(scheme.as_str(), "bearer" | "basic" | "token" | "digest") {
            let mut next = value_start + scheme.len();
            while next < line.len() && line[next..].chars().next().is_some_and(char::is_whitespace)
            {
                next += line[next..].chars().next().unwrap_or(' ').len_utf8();
            }
            value_start = next;
        }
        // Already-redacted text must be left byte-identical: sealing twice has
        // to produce the same document and therefore the same hash.
        if line[value_start..].starts_with("[redacted]") {
            index = value_start + "[redacted]".len();
            continue;
        }
        let value_end = value_start
            + line[value_start..]
                .find(|candidate: char| {
                    candidate.is_whitespace()
                        || matches!(
                            candidate,
                            '&' | '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}' | '<'
                        )
                })
                .unwrap_or(line.len() - value_start);
        if value_end <= value_start {
            index += character.len_utf8();
            continue;
        }

        if value_start >= cursor {
            output.push_str(&line[cursor..value_start]);
            output.push_str("[redacted]");
            cursor = value_end;
            changed = true;
        }
        index = value_end;
    }

    output.push_str(&line[cursor..]);
    (output, changed)
}

/// Replace PEM blocks — private keys, certificates, and the like — as a whole.
fn redact_pem_blocks(text: &str) -> (String, bool) {
    const BEGIN: &str = "-----BEGIN ";
    const END: &str = "-----END ";
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;

    while let Some(begin) = rest.find(BEGIN) {
        let Some(end_offset) = rest[begin..].find(END) else {
            break;
        };
        let end_start = begin + end_offset;
        let line_end = rest[end_start..]
            .find('\n')
            .map(|offset| end_start + offset)
            .unwrap_or(rest.len());
        output.push_str(&rest[..begin]);
        output.push_str("[redacted PEM block]");
        if line_end < rest.len() {
            output.push('\n');
            rest = &rest[line_end + 1..];
        } else {
            rest = "";
        }
        changed = true;
    }

    output.push_str(rest);
    (output, changed)
}

/// Redact every string inside a JSON value.
///
/// Redaction happens per string rather than on serialized JSON so the document
/// stays valid: replacing a quoted value inside the text would break the
/// syntax. An object value under a sensitive key is cleared outright, because
/// the key and its value are separate JSON fields.
fn redact_json(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => redact_snapshot_text(text),
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= redact_json(item);
            }
            changed
        }
        serde_json::Value::Object(map) => {
            let mut changed = false;
            for (key, item) in map.iter_mut() {
                if is_sensitive_key(key) {
                    if let serde_json::Value::String(text) = item {
                        if !text.is_empty() && text != "[redacted]" {
                            text.clear();
                            text.push_str("[redacted]");
                            changed = true;
                        }
                    }
                }
                changed |= redact_json(item);
            }
            changed
        }
        _ => false,
    }
}

/// Keep at most `max_chars` characters, appending the truncation marker.
fn truncate_chars(text: &mut String, max_chars: usize) {
    if text.chars().count() <= max_chars {
        return;
    }
    let kept = text
        .chars()
        .take(max_chars)
        .collect::<String>()
        .trim_end()
        .to_string();
    *text = kept;
    text.push_str(TRUNCATION_MARKER);
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::ContextProjection;
    use crate::usage::{TokenSource, ESTIMATE_COUNTER};

    fn message(role: PromptRole, content: &str) -> SnapshotMessage {
        SnapshotMessage::new(role, content)
    }

    #[test]
    fn a_sealed_snapshot_hashes_the_document_it_stores() {
        let snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "gpt-4o")
            .with_messages(vec![
                message(PromptRole::System, "You are CowBot."),
                message(PromptRole::User, "Summarise the task."),
            ])
            .seal(&ESTIMATE_COUNTER);

        assert_eq!(snapshot.schema_version, PROMPT_SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.content_hash.len(), 64);
        assert!(snapshot.hash_matches());
        assert!(snapshot.token_count > 0);
        assert!(!snapshot.truncated);
        assert!(!snapshot.redacted);
    }

    #[test]
    fn sealing_twice_produces_the_same_hash() {
        let snapshot = LlmPromptSnapshot::new(LlmCallKind::Compact, "claude-sonnet")
            .with_messages(vec![message(PromptRole::User, "hello")])
            .seal(&ESTIMATE_COUNTER);
        let again = snapshot.clone().seal(&ESTIMATE_COUNTER);
        assert_eq!(snapshot.content_hash, again.content_hash);
        assert_eq!(snapshot.token_count, again.token_count);
    }

    #[test]
    fn redaction_covers_messages_slots_memory_and_tool_arguments() {
        let snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "gpt-4o")
            .with_messages(vec![
                message(
                    PromptRole::System,
                    "Use this key: OPENAI_API_KEY=sk-live-secret-value",
                ),
                SnapshotMessage::new(PromptRole::Assistant, "").with_tool_calls(vec![
                    SnapshotToolCall {
                        id: "call-1".to_string(),
                        name: "run_shell".to_string(),
                        arguments: serde_json::json!({
                            "command": "curl -H 'Authorization: Bearer abc123def456ghi789jkl'"
                        }),
                    },
                ]),
            ])
            .with_slots(vec![PromptSlot {
                name: crate::prompt::PromptSlotName::Platform,
                content: "token=super-secret-token-value".to_string(),
                tokens: 3,
            }])
            .with_memory(
                PromptMemory {
                    block: "password = hunter2-hunter2".to_string(),
                    tokens: 4,
                },
                vec!["entry-1".to_string()],
            )
            .with_tools(vec![ToolDefinition {
                name: "run_shell".to_string(),
                description: "Run a command with API_KEY=sk-another-secret".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            }])
            .seal(&ESTIMATE_COUNTER);

        assert!(snapshot.redacted);
        let payload = snapshot.payload();
        for secret in [
            "sk-live-secret-value",
            "abc123def456ghi789jkl",
            "super-secret-token-value",
            "hunter2-hunter2",
            "sk-another-secret",
        ] {
            assert!(
                !payload.contains(secret),
                "the sealed payload still contains {secret}: {payload}"
            );
        }
        assert!(snapshot.hash_matches());
        // The document stays readable JSON after redaction.
        let restored = LlmPromptSnapshot::from_payload(&payload).expect("sealed payload parses");
        assert_eq!(restored, snapshot);
        assert_eq!(restored.selected_memory_ids, vec!["entry-1".to_string()]);
    }

    #[test]
    fn clean_content_is_not_marked_redacted() {
        let snapshot = LlmPromptSnapshot::new(LlmCallKind::Router, "mock-model")
            .with_messages(vec![message(PromptRole::User, "Who should answer?")])
            .seal(&ESTIMATE_COUNTER);
        assert!(!snapshot.redacted);
        assert!(snapshot.payload().contains("Who should answer?"));
    }

    #[test]
    fn an_oversized_snapshot_drops_the_oldest_history_and_marks_it() {
        let long = "x".repeat(PROMPT_SNAPSHOT_MESSAGE_MAX_CHARS);
        let mut messages = vec![message(PromptRole::System, "system prompt")];
        for index in 0..200 {
            messages.push(message(PromptRole::User, &format!("{index}-{long}")));
        }
        let snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "gpt-4o")
            .with_messages(messages)
            .seal(&ESTIMATE_COUNTER);

        assert!(snapshot.truncated);
        assert!(snapshot.omitted_messages > 0);
        assert!(snapshot.payload().len() <= PROMPT_SNAPSHOT_MAX_BYTES);
        // The system prompt and the newest message survive.
        assert_eq!(snapshot.messages[0].role, PromptRole::System);
        assert!(snapshot
            .messages
            .last()
            .unwrap()
            .content
            .starts_with("199-"));
        assert!(snapshot.hash_matches());
    }

    #[test]
    fn message_truncation_keeps_char_boundaries() {
        let content = "你".repeat(PROMPT_SNAPSHOT_MESSAGE_MAX_CHARS + 10);
        let snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "gpt-4o")
            .with_messages(vec![message(PromptRole::User, &content)])
            .seal(&ESTIMATE_COUNTER);
        // A single message under the payload bound keeps its body: 8 KiB of
        // multi-byte characters is still far below 256 KiB.
        assert!(!snapshot.truncated);
        assert_eq!(
            snapshot.messages[0].content.chars().count(),
            content.chars().count()
        );

        let mut text = "你".repeat(10);
        truncate_chars(&mut text, 4);
        assert_eq!(text.chars().take(4).collect::<String>(), "你你你你");
        assert!(text.ends_with(TRUNCATION_MARKER));
    }

    #[test]
    fn tokens_count_messages_and_tool_schemas() {
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "gpt-4o")
            .with_messages(vec![message(PromptRole::User, "hello world")])
            .with_tools(tools.clone())
            .seal(&ESTIMATE_COUNTER);

        let expected = ESTIMATE_COUNTER.count_message("hello world")
            + ESTIMATE_COUNTER.count(&serde_json::to_string(&tools).unwrap());
        assert_eq!(snapshot.token_count, expected);
    }

    #[test]
    fn a_foreign_schema_version_is_refused() {
        let mut snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "gpt-4o");
        snapshot.schema_version = PROMPT_SNAPSHOT_SCHEMA_VERSION + 1;
        let error = LlmPromptSnapshot::from_payload(&snapshot.payload()).unwrap_err();
        assert!(error.contains("unsupported prompt snapshot schema version"));
    }

    #[test]
    fn a_tampered_payload_fails_its_hash() {
        let snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "gpt-4o")
            .with_messages(vec![message(PromptRole::User, "original")])
            .seal(&ESTIMATE_COUNTER);
        let mut tampered = LlmPromptSnapshot::from_payload(&snapshot.payload()).unwrap();
        tampered.messages[0].content = "edited".to_string();
        assert!(!tampered.hash_matches());
    }

    #[test]
    fn the_projection_and_slots_travel_with_the_snapshot() {
        let projection = ContextProjection {
            system_tokens: 10,
            tool_tokens: 5,
            history_tokens: 20,
            latest_tokens: 4,
            active_entries: 2,
            excluded_entries: 0,
            token_source: TokenSource::Tiktoken,
            token_counter: Some("o200k_base".to_string()),
        };
        let snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "gpt-4o")
            .with_slots(vec![PromptSlot {
                name: crate::prompt::PromptSlotName::Agent,
                content: "Planner".to_string(),
                tokens: 2,
            }])
            .with_projection(projection.clone())
            .seal(&ESTIMATE_COUNTER);

        let restored = LlmPromptSnapshot::from_payload(&snapshot.payload()).unwrap();
        assert_eq!(restored.projection, Some(projection));
        assert_eq!(restored.slots.len(), 1);
        assert_eq!(restored.call_kind, LlmCallKind::Agent);
        assert_eq!(restored.model, "gpt-4o");
    }

    #[test]
    fn retention_days_are_clamped() {
        let mut settings = PromptSnapshotSettings::new(WorkspaceId::new("workspace-1"));
        assert!(settings.enabled);
        assert_eq!(
            settings.retention_days,
            DEFAULT_PROMPT_SNAPSHOT_RETENTION_DAYS
        );
        settings.set_retention_days(0);
        assert_eq!(settings.retention_days, MIN_PROMPT_SNAPSHOT_RETENTION_DAYS);
        settings.set_retention_days(u32::MAX);
        assert_eq!(settings.retention_days, MAX_PROMPT_SNAPSHOT_RETENTION_DAYS);
        settings.set_retention_days(90);
        assert_eq!(settings.retention_days, 90);
    }

    #[test]
    fn redaction_clears_every_sensitive_pair_on_a_line() {
        let mut text =
            "curl -H 'X-Api-Key: sk-header-value' 'https://api.test/v1?token=super-secret-value&limit=5'"
                .to_string();
        assert!(redact_snapshot_text(&mut text));
        assert!(!text.contains("sk-header-value"), "{text}");
        assert!(!text.contains("super-secret-value"), "{text}");
        // Values that are not credential-shaped survive.
        assert!(text.contains("limit=5"), "{text}");
    }

    #[test]
    fn redaction_replaces_pem_blocks_whole() {
        let mut text = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\nafter"
            .to_string();
        assert!(redact_snapshot_text(&mut text));
        assert!(text.contains("[redacted PEM block]"), "{text}");
        assert!(!text.contains("MIIEowIBAAKCAQEA"), "{text}");
        assert!(text.contains("before"), "{text}");
        assert!(text.contains("after"), "{text}");
    }

    #[test]
    fn redaction_clears_the_credential_after_an_auth_scheme() {
        let mut text = "Authorization: Bearer abc123def456ghi789jkl".to_string();
        assert!(redact_snapshot_text(&mut text));
        assert!(!text.contains("abc123def456ghi789jkl"), "{text}");
        assert!(!text.contains("Bearer abc123"), "{text}");
    }

    #[test]
    fn json_values_under_sensitive_keys_are_cleared() {
        let mut value = serde_json::json!({
            "headers": {"api_key": "plain-value", "accept": "application/json"},
            "count": 1,
        });
        assert!(redact_json(&mut value));
        assert_eq!(value["headers"]["api_key"], "[redacted]");
        assert_eq!(value["headers"]["accept"], "application/json");
        assert_eq!(value["count"], 1);
    }

    #[test]
    fn sealing_a_dirty_snapshot_twice_keeps_the_redacted_flag() {
        let snapshot = LlmPromptSnapshot::new(LlmCallKind::Agent, "gpt-4o")
            .with_messages(vec![message(
                PromptRole::User,
                "OPENAI_API_KEY=sk-live-secret-value",
            )])
            .seal(&ESTIMATE_COUNTER);
        assert!(snapshot.redacted);

        let again = snapshot.clone().seal(&ESTIMATE_COUNTER);
        assert!(again.redacted);
        assert_eq!(
            snapshot.content_hash,
            again.content_hash,
            "first: {}\nsecond: {}",
            snapshot.payload(),
            again.payload()
        );
        assert_eq!(snapshot.payload(), again.payload());
    }
}
