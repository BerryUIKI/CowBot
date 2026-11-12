//! Persistence for sealed prompt snapshots and their workspace settings
//! (ADR-0037).
//!
//! A snapshot is keyed by the LLM call it belongs to. The call row already
//! carries the run, task, channel, and agent, so the summary join is the only
//! place those links are needed.

use chrono::Utc;
use cowbot_core::agent::AgentId;
use cowbot_core::channel::ChannelId;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::prompt_snapshot::{
    clamp_retention_days, LlmPromptSnapshot, PromptSnapshotSettings, PromptSnapshotSummary,
};
use cowbot_core::run::RunId;
use cowbot_core::task::TaskId;
use cowbot_core::usage::LlmCallKind;

use crate::error::{db, parse_datetime};
use crate::store::Store;

/// Metadata columns shared by every summary query, with the snapshot table
/// aliased `s` and the call table aliased `c`.
const SUMMARY_COLUMNS: &str = "s.llm_call_id, c.run_id, c.task_id, c.channel_id, c.agent_id,
     c.call_kind, c.model, s.token_count, s.byte_size, s.truncated, s.redacted, s.created_at";

#[derive(sqlx::FromRow)]
struct SnapshotSummaryRow {
    llm_call_id: String,
    run_id: Option<String>,
    task_id: Option<String>,
    channel_id: Option<String>,
    agent_id: Option<String>,
    call_kind: String,
    model: String,
    token_count: i64,
    byte_size: i64,
    truncated: i64,
    redacted: i64,
    created_at: String,
}

impl SnapshotSummaryRow {
    fn into_domain(self) -> CowResult<PromptSnapshotSummary> {
        let call_kind = self.call_kind.parse::<LlmCallKind>().map_err(|_| {
            CowError::Storage(format!("unknown LLM call kind '{}'", self.call_kind))
        })?;
        Ok(PromptSnapshotSummary {
            llm_call_id: self.llm_call_id,
            run_id: self.run_id.map(RunId),
            task_id: self.task_id.map(TaskId),
            channel_id: self.channel_id.map(ChannelId::new),
            agent_id: self.agent_id.map(AgentId),
            call_kind,
            model: self.model,
            token_count: clamp_count(self.token_count),
            byte_size: clamp_count(self.byte_size),
            truncated: self.truncated != 0,
            redacted: self.redacted != 0,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

#[derive(sqlx::FromRow)]
struct PromptSnapshotRow {
    payload: String,
}

#[derive(sqlx::FromRow)]
struct PromptSnapshotSettingsRow {
    enabled: i64,
    retention_days: i64,
    updated_at: String,
}

impl Store {
    /// Store one sealed snapshot for a call.
    ///
    /// Recording replaces an earlier row for the same call: a retried call
    /// reuses its call id and must not accumulate snapshots.
    pub async fn record_prompt_snapshot(
        &self,
        llm_call_id: &str,
        snapshot: &LlmPromptSnapshot,
    ) -> CowResult<()> {
        let payload = snapshot.payload();
        sqlx::query(
            "INSERT INTO llm_prompt_snapshots
                (llm_call_id, schema_version, content_hash, token_count, byte_size,
                 truncated, redacted, created_at, payload)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(llm_call_id) DO UPDATE SET
                schema_version = excluded.schema_version,
                content_hash = excluded.content_hash,
                token_count = excluded.token_count,
                byte_size = excluded.byte_size,
                truncated = excluded.truncated,
                redacted = excluded.redacted,
                created_at = excluded.created_at,
                payload = excluded.payload",
        )
        .bind(llm_call_id)
        .bind(i64::from(snapshot.schema_version))
        .bind(&snapshot.content_hash)
        .bind(i64::from(snapshot.token_count))
        .bind(payload.len() as i64)
        .bind(bool_int(snapshot.truncated))
        .bind(bool_int(snapshot.redacted))
        .bind(snapshot.created_at.to_rfc3339())
        .bind(payload)
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Read one stored snapshot, verifying it against its recorded hash.
    ///
    /// A payload that no longer matches its hash is reported as a storage
    /// error rather than returned as evidence.
    pub async fn get_prompt_snapshot(
        &self,
        llm_call_id: &str,
    ) -> CowResult<Option<LlmPromptSnapshot>> {
        let row = sqlx::query_as::<_, PromptSnapshotRow>(
            "SELECT payload FROM llm_prompt_snapshots WHERE llm_call_id = ?",
        )
        .bind(llm_call_id)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let snapshot = LlmPromptSnapshot::from_payload(&row.payload)
            .map_err(|error| CowError::Storage(format!("invalid prompt snapshot: {error}")))?;
        if !snapshot.hash_matches() {
            return Err(CowError::Storage(format!(
                "prompt snapshot '{llm_call_id}' does not match its content hash"
            )));
        }
        Ok(Some(snapshot))
    }

    /// Snapshots recorded for a run, oldest first.
    pub async fn list_prompt_snapshots_for_run(
        &self,
        run_id: &RunId,
    ) -> CowResult<Vec<PromptSnapshotSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS}
             FROM llm_prompt_snapshots s
             JOIN llm_calls c ON c.id = s.llm_call_id
             WHERE c.run_id = ?
             ORDER BY s.created_at, s.llm_call_id"
        );
        self.list_snapshot_summaries(&sql, &run_id.0).await
    }

    /// Snapshots recorded for a task, oldest first.
    pub async fn list_prompt_snapshots_for_task(
        &self,
        task_id: &TaskId,
    ) -> CowResult<Vec<PromptSnapshotSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS}
             FROM llm_prompt_snapshots s
             JOIN llm_calls c ON c.id = s.llm_call_id
             WHERE c.task_id = ?
             ORDER BY s.created_at, s.llm_call_id"
        );
        self.list_snapshot_summaries(&sql, &task_id.0).await
    }

    /// Snapshots recorded for a channel or conversation, oldest first.
    pub async fn list_prompt_snapshots_for_channel(
        &self,
        channel_id: &ChannelId,
        limit: u32,
    ) -> CowResult<Vec<PromptSnapshotSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS}
             FROM llm_prompt_snapshots s
             JOIN llm_calls c ON c.id = s.llm_call_id
             WHERE c.channel_id = ?
             ORDER BY s.created_at DESC, s.llm_call_id DESC
             LIMIT ?"
        );
        let rows = sqlx::query_as::<_, SnapshotSummaryRow>(&sql)
            .bind(&channel_id.0)
            .bind(i64::from(limit))
            .fetch_all(self.pool())
            .await
            .map_err(db)?;
        rows.into_iter()
            .map(SnapshotSummaryRow::into_domain)
            .collect()
    }

    /// The newest snapshots across the workspace, for the audit view.
    pub async fn list_recent_prompt_snapshots(
        &self,
        limit: u32,
    ) -> CowResult<Vec<PromptSnapshotSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS}
             FROM llm_prompt_snapshots s
             JOIN llm_calls c ON c.id = s.llm_call_id
             ORDER BY s.created_at DESC, s.llm_call_id DESC
             LIMIT ?"
        );
        let rows = sqlx::query_as::<_, SnapshotSummaryRow>(&sql)
            .bind(i64::from(limit))
            .fetch_all(self.pool())
            .await
            .map_err(db)?;
        rows.into_iter()
            .map(SnapshotSummaryRow::into_domain)
            .collect()
    }

    /// Delete snapshots older than the cutoff, returning how many went.
    ///
    /// Only `llm_prompt_snapshots` rows are touched; the usage rows they
    /// belong to keep their tokens and call metadata.
    pub async fn delete_prompt_snapshots_before(
        &self,
        cutoff: chrono::DateTime<Utc>,
    ) -> CowResult<u64> {
        let result = sqlx::query("DELETE FROM llm_prompt_snapshots WHERE created_at < ?")
            .bind(cutoff.to_rfc3339())
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    /// Read the workspace snapshot settings, creating defaults on first use.
    pub async fn get_prompt_snapshot_settings(
        &self,
        workspace_id: &WorkspaceId,
    ) -> CowResult<PromptSnapshotSettings> {
        let defaults = PromptSnapshotSettings::new(workspace_id.clone());
        sqlx::query(
            "INSERT OR IGNORE INTO workspace_prompt_snapshot_settings
                (workspace_id, enabled, retention_days, updated_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&workspace_id.0)
        .bind(bool_int(defaults.enabled))
        .bind(i64::from(defaults.retention_days))
        .bind(defaults.updated_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;

        let row = sqlx::query_as::<_, PromptSnapshotSettingsRow>(
            "SELECT enabled, retention_days, updated_at
             FROM workspace_prompt_snapshot_settings WHERE workspace_id = ?",
        )
        .bind(&workspace_id.0)
        .fetch_one(self.pool())
        .await
        .map_err(db)?;
        Ok(PromptSnapshotSettings {
            workspace_id: workspace_id.clone(),
            enabled: row.enabled != 0,
            retention_days: clamp_retention_days(clamp_count(row.retention_days)),
            updated_at: parse_datetime(&row.updated_at)?,
        })
    }

    pub async fn save_prompt_snapshot_settings(
        &self,
        settings: &PromptSnapshotSettings,
    ) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO workspace_prompt_snapshot_settings
                (workspace_id, enabled, retention_days, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(workspace_id) DO UPDATE SET
                enabled = excluded.enabled,
                retention_days = excluded.retention_days,
                updated_at = excluded.updated_at",
        )
        .bind(&settings.workspace_id.0)
        .bind(bool_int(settings.enabled))
        .bind(i64::from(clamp_retention_days(settings.retention_days)))
        .bind(settings.updated_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn list_snapshot_summaries(
        &self,
        sql: &str,
        scope: &str,
    ) -> CowResult<Vec<PromptSnapshotSummary>> {
        let rows = sqlx::query_as::<_, SnapshotSummaryRow>(sql)
            .bind(scope)
            .fetch_all(self.pool())
            .await
            .map_err(db)?;
        rows.into_iter()
            .map(SnapshotSummaryRow::into_domain)
            .collect()
    }
}

fn bool_int(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn clamp_count(value: i64) -> u32 {
    value.clamp(0, i64::from(u32::MAX)) as u32
}
