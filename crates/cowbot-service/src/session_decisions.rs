use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cowbot_core::approval::{ApprovalId, ApprovalStatus};
use cowbot_core::tool::ToolCall;
use serde::Serialize;

/// A remembered approval decision shown in Settings.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SessionDecisionEntry {
    pub key: String,
    pub decision: String,
}

/// In-memory approvals remembered for the lifetime of the local process.
///
/// Non-shell tools are remembered per tool; `execute_code` is remembered per
/// normalized exact command. Decisions are never persisted and clear on
/// restart.
#[derive(Debug, Default)]
pub struct SessionDecisions {
    decisions: Mutex<HashMap<String, ApprovalStatus>>,
    pending: Mutex<HashMap<ApprovalId, ToolCall>>,
}

impl SessionDecisions {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Associate a pending approval with the call it authorizes.
    pub fn record_pending(&self, approval_id: &ApprovalId, call: &ToolCall) {
        self.pending
            .lock()
            .unwrap()
            .insert(approval_id.clone(), call.clone());
    }

    /// Take the call behind a pending approval, if this process created it.
    pub fn take_pending(&self, approval_id: &ApprovalId) -> Option<ToolCall> {
        self.pending.lock().unwrap().remove(approval_id)
    }

    /// Remember a human decision for the call's session key.
    pub fn remember(&self, call: &ToolCall, status: ApprovalStatus) {
        if status != ApprovalStatus::Approved && status != ApprovalStatus::Rejected {
            return;
        }
        self.decisions
            .lock()
            .unwrap()
            .insert(session_key(call), status);
    }

    pub fn lookup(&self, call: &ToolCall) -> Option<ApprovalStatus> {
        self.decisions
            .lock()
            .unwrap()
            .get(&session_key(call))
            .copied()
    }

    pub fn entries(&self) -> Vec<SessionDecisionEntry> {
        let mut entries = self
            .decisions
            .lock()
            .unwrap()
            .iter()
            .map(|(key, status)| SessionDecisionEntry {
                key: key.clone(),
                decision: match status {
                    ApprovalStatus::Approved => "approved".to_string(),
                    ApprovalStatus::Rejected => "rejected".to_string(),
                    _ => "unknown".to_string(),
                },
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        entries
    }

    pub fn clear(&self) {
        self.decisions.lock().unwrap().clear();
    }
}

/// The key used to remember one session decision.
pub fn session_key(call: &ToolCall) -> String {
    if call.name == "execute_code" {
        if let Some(command) = call
            .arguments
            .get("command")
            .and_then(|value| value.as_str())
        {
            let normalized = command
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            return format!("execute_code:{normalized}");
        }
    }
    format!("tool:{}", call.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            name: name.to_string(),
            arguments,
        }
    }

    #[test]
    fn non_shell_tools_are_remembered_by_name() {
        let decisions = SessionDecisions::new();
        let write = call("write_file", json!({"path": "a.txt"}));
        decisions.remember(&write, ApprovalStatus::Approved);

        assert_eq!(
            decisions.lookup(&call("write_file", json!({"path": "b.txt"}))),
            Some(ApprovalStatus::Approved)
        );
        assert_eq!(decisions.lookup(&call("read_file", json!({}))), None);
        assert_eq!(decisions.entries()[0].key, "tool:write_file");
    }

    #[test]
    fn shell_commands_are_remembered_by_normalized_command() {
        let decisions = SessionDecisions::new();
        let command = call("execute_code", json!({"command": "  Git   STATUS  "}));
        decisions.remember(&command, ApprovalStatus::Rejected);

        assert_eq!(
            decisions.lookup(&call("execute_code", json!({"command": "git status"}))),
            Some(ApprovalStatus::Rejected)
        );
        assert_eq!(
            decisions.lookup(&call("execute_code", json!({"command": "git diff"}))),
            None
        );
    }

    #[test]
    fn pending_calls_round_trip_by_approval_id() {
        let decisions = SessionDecisions::new();
        let approval_id = ApprovalId::new("approval-1");
        let write = call("write_file", json!({"path": "a.txt"}));
        decisions.record_pending(&approval_id, &write);

        let taken = decisions.take_pending(&approval_id).unwrap();
        assert_eq!(taken.name, "write_file");
        assert!(decisions.take_pending(&approval_id).is_none());
    }
}
