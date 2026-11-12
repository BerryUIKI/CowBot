//! Prompt snapshot recording, retention, and reads (ADR-0037).
//!
//! Recording is best-effort by contract, like usage recording: a snapshot
//! failure must never fail an agent turn. Retention, by contrast, is an
//! explicit operation: the caller asks for it and sees how many rows went.

use chrono::{Duration, Utc};
use cowbot_core::channel::ChannelId;
use cowbot_core::error::CowResult;
use cowbot_core::identity::WorkspaceId;
use cowbot_core::prompt::{PromptMemory, PromptSlot};
use cowbot_core::prompt_snapshot::{
    LlmPromptSnapshot, PromptSnapshotDetail, PromptSnapshotSettings, PromptSnapshotSummary,
};
use cowbot_core::run::RunId;
use cowbot_store::Store;

/// What prompt assembly knew beside the raw request.
///
/// The agent runtime sees only the finished messages, so the conversation
/// service passes the named slots and the memory selection it assembled the
/// prompt from. Applying a context never overwrites what the snapshot already
/// carries.
#[derive(Debug, Clone, Default)]
pub struct SnapshotContext {
    pub slots: Vec<PromptSlot>,
    pub memory: Option<PromptMemory>,
    pub selected_memory_ids: Vec<String>,
}

impl SnapshotContext {
    pub fn with_slots(mut self, slots: Vec<PromptSlot>) -> Self {
        self.slots = slots;
        self
    }

    pub fn with_memory(mut self, memory: Option<PromptMemory>, selected: Vec<String>) -> Self {
        self.memory = memory;
        self.selected_memory_ids = selected;
        self
    }

    fn apply(&self, mut snapshot: LlmPromptSnapshot) -> LlmPromptSnapshot {
        if snapshot.slots.is_empty() {
            snapshot.slots = self.slots.clone();
        }
        if snapshot.memory.is_none() {
            if let Some(memory) = &self.memory {
                snapshot.memory = Some(memory.clone());
                snapshot.selected_memory_ids = self.selected_memory_ids.clone();
            }
        }
        snapshot
    }
}

/// Attach the assembly context to a captured request and seal it.
pub fn seal_with_context(
    snapshot: LlmPromptSnapshot,
    context: &SnapshotContext,
) -> LlmPromptSnapshot {
    cowbot_llm::seal_snapshot(context.apply(snapshot))
}

/// Stores and sweeps the redacted prompt snapshots of one workspace.
#[derive(Clone)]
pub struct PromptSnapshotService {
    store: Store,
    workspace_id: WorkspaceId,
}

impl PromptSnapshotService {
    pub fn new(store: Store, workspace_id: WorkspaceId) -> Self {
        Self {
            store,
            workspace_id,
        }
    }

    /// The workspace snapshot settings, creating defaults on first use.
    pub async fn settings(&self) -> CowResult<PromptSnapshotSettings> {
        self.store
            .get_prompt_snapshot_settings(&self.workspace_id)
            .await
    }

    /// Change the settings and apply the new retention window at once.
    ///
    /// Lowering the window deletes the snapshots that just fell outside it,
    /// so the setting means what it says without waiting for a restart.
    pub async fn update_settings(
        &self,
        enabled: Option<bool>,
        retention_days: Option<u32>,
    ) -> CowResult<PromptSnapshotSettings> {
        let mut settings = self.settings().await?;
        if let Some(enabled) = enabled {
            settings.enabled = enabled;
        }
        if let Some(days) = retention_days {
            settings.set_retention_days(days);
        }
        settings.updated_at = Utc::now();
        self.store.save_prompt_snapshot_settings(&settings).await?;
        self.sweep().await?;
        Ok(settings)
    }

    /// Store one sealed snapshot when the workspace records them.
    ///
    /// Returns whether a row was written. Callers log the error and continue:
    /// evidence is valuable, but it is not worth failing a turn over.
    pub async fn record(&self, llm_call_id: &str, snapshot: &LlmPromptSnapshot) -> CowResult<bool> {
        if !self.settings().await?.enabled {
            return Ok(false);
        }
        self.store
            .record_prompt_snapshot(&self.workspace_id, llm_call_id, snapshot)
            .await?;
        Ok(true)
    }

    /// Read one stored snapshot, verified against its content hash.
    pub async fn get(&self, llm_call_id: &str) -> CowResult<Option<LlmPromptSnapshot>> {
        self.store.get_prompt_snapshot(llm_call_id).await
    }

    /// Read one stored snapshot with the call links and a hash-mismatch flag.
    pub async fn detail(&self, llm_call_id: &str) -> CowResult<Option<PromptSnapshotDetail>> {
        self.store.get_prompt_snapshot_detail(llm_call_id).await
    }

    /// Snapshots recorded for a run, oldest first.
    pub async fn list_for_run(&self, run_id: &RunId) -> CowResult<Vec<PromptSnapshotSummary>> {
        self.store.list_prompt_snapshots_for_run(run_id).await
    }

    /// The newest snapshots in the workspace, for the audit view.
    pub async fn list_recent(&self, limit: u32) -> CowResult<Vec<PromptSnapshotSummary>> {
        self.store
            .list_recent_prompt_snapshots(&self.workspace_id, limit)
            .await
    }

    /// Remove every snapshot recorded for a channel that is being deleted.
    pub async fn delete_for_channel(&self, channel_id: &ChannelId) -> CowResult<u64> {
        self.store
            .delete_prompt_snapshots_for_channel(channel_id)
            .await
    }

    /// Delete snapshots older than the workspace retention window.
    ///
    /// The sweep runs whether or not recording is enabled: turning recording
    /// off stops new evidence, it does not keep old evidence forever.
    pub async fn sweep(&self) -> CowResult<u64> {
        let settings = self.settings().await?;
        let cutoff = Utc::now() - Duration::days(i64::from(settings.retention_days));
        self.store
            .delete_prompt_snapshots_before(&self.workspace_id, cutoff)
            .await
    }
}
