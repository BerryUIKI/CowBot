//! Process-wide per-branch compaction locks.
//!
//! A conversation can be compacted from the desktop, by the pre-send capacity
//! check, or by a group turn's automatic path. Those paths can run on
//! different `ConversationService` instances, so the registry is shared by
//! `Arc` rather than owned by one service.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use cowbot_core::agent::AgentId;
use cowbot_core::channel::ChannelId;

/// Branches with an in-flight compaction.
#[derive(Default)]
pub struct CompactionLocks {
    held: Mutex<HashSet<(ChannelId, AgentId)>>,
}

impl CompactionLocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Claim `key`, or return `None` when another compaction already holds it.
    ///
    /// The guard is released on drop, including on a model error, an early
    /// return, or a panic. No await happens while the container lock is held.
    #[must_use = "the guard releases the branch when dropped"]
    pub fn try_acquire(self: &Arc<Self>, key: (ChannelId, AgentId)) -> Option<CompactionGuard> {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !held.insert(key.clone()) {
            return None;
        }
        Some(CompactionGuard {
            locks: Arc::clone(self),
            key,
        })
    }
}

/// Releases its branch when dropped.
pub struct CompactionGuard {
    locks: Arc<CompactionLocks>,
    key: (ChannelId, AgentId),
}

impl Drop for CompactionGuard {
    fn drop(&mut self) {
        let mut held = self
            .locks
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        held.remove(&self.key);
    }
}
