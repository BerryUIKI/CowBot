use cowbot_core::risk::RiskLevel;
use cowbot_core::tool::ToolCall;
use serde::{Deserialize, Serialize};

/// Result of classifying a tool call before execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub rule_id: String,
    pub reason: String,
}

/// Deterministic, static risk classifier for the built-in tools.
#[derive(Debug, Default, Clone)]
pub struct StaticRiskClassifier;

impl StaticRiskClassifier {
    pub fn classify(&self, call: &ToolCall) -> RiskAssessment {
        match call.name.as_str() {
            "read_file" => {
                self.classify_path_tool(call, RiskLevel::ReadOnly, RiskLevel::Medium, "read_file")
            }
            "write_file" => {
                self.classify_path_tool(call, RiskLevel::Medium, RiskLevel::High, "write_file")
            }
            "execute_code" => self.classify_command(call),
            "list_agents" | "list_templates" => RiskAssessment {
                level: RiskLevel::ReadOnly,
                rule_id: "workspace.read_only".to_string(),
                reason: format!("{} reads workspace metadata", call.name),
            },
            "create_channel" => RiskAssessment {
                level: RiskLevel::Medium,
                rule_id: "workspace.create_channel".to_string(),
                reason: "create_channel changes the workspace channel list".to_string(),
            },
            "create_task" => RiskAssessment {
                level: RiskLevel::Medium,
                rule_id: "workspace.create_task".to_string(),
                reason: "create_task queues work for another agent".to_string(),
            },
            "instantiate_team" => RiskAssessment {
                level: RiskLevel::High,
                rule_id: "workspace.instantiate_team".to_string(),
                reason: "instantiate_team adds agents to the workspace".to_string(),
            },
            other => RiskAssessment {
                level: RiskLevel::High,
                rule_id: "unknown_tool".to_string(),
                reason: format!("unknown tool '{other}' cannot be classified"),
            },
        }
    }

    fn classify_path_tool(
        &self,
        call: &ToolCall,
        base: RiskLevel,
        broad_path: RiskLevel,
        tool: &str,
    ) -> RiskAssessment {
        let Some(path) = call.arguments.get("path").and_then(|value| value.as_str()) else {
            return RiskAssessment {
                level: RiskLevel::High,
                rule_id: format!("{tool}.missing_path"),
                reason: "path argument is missing".to_string(),
            };
        };

        if is_broad_path(path) {
            RiskAssessment {
                level: broad_path,
                rule_id: format!("{tool}.outside_workspace"),
                reason: format!("path '{path}' may escape the workspace"),
            }
        } else {
            RiskAssessment {
                level: base,
                rule_id: format!("{tool}.workspace"),
                reason: format!("{tool} inside the workspace"),
            }
        }
    }

    fn classify_command(&self, call: &ToolCall) -> RiskAssessment {
        let Some(command) = call
            .arguments
            .get("command")
            .and_then(|value| value.as_str())
        else {
            return RiskAssessment {
                level: RiskLevel::High,
                rule_id: "execute_code.missing_command".to_string(),
                reason: "command argument is missing".to_string(),
            };
        };
        let normalized = command.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return RiskAssessment {
                level: RiskLevel::High,
                rule_id: "execute_code.empty_command".to_string(),
                reason: "command is empty".to_string(),
            };
        }

        if let Some(pattern) = CRITICAL_PATTERNS
            .iter()
            .find(|pattern| normalized.contains(*pattern))
        {
            return RiskAssessment {
                level: RiskLevel::Critical,
                rule_id: "execute_code.critical_pattern".to_string(),
                reason: format!("command matches critical pattern '{pattern}'"),
            };
        }
        if let Some(pattern) = HIGH_PATTERNS
            .iter()
            .find(|pattern| normalized.contains(*pattern))
        {
            return RiskAssessment {
                level: RiskLevel::High,
                rule_id: "execute_code.high_pattern".to_string(),
                reason: format!("command matches high-risk pattern '{pattern}'"),
            };
        }

        if is_read_only_command(&normalized) {
            let level = if has_shell_composition(&normalized) {
                RiskLevel::Medium
            } else {
                RiskLevel::ReadOnly
            };
            return RiskAssessment {
                level,
                rule_id: "execute_code.read_only".to_string(),
                reason: "read-only command".to_string(),
            };
        }
        if is_low_risk_command(&normalized) {
            return RiskAssessment {
                level: RiskLevel::Low,
                rule_id: "execute_code.low_risk".to_string(),
                reason: "low-risk command".to_string(),
            };
        }

        RiskAssessment {
            level: RiskLevel::Medium,
            rule_id: "execute_code.default".to_string(),
            reason: "command execution defaults to medium risk".to_string(),
        }
    }
}

const CRITICAL_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -fr /",
    "rm -rf --no-preserve-root",
    "mkfs",
    "dd if=",
    "diskpart",
    "bcdedit",
    "reg delete hklm",
    "shutdown",
    "reboot",
    "format c:",
    "cipher /w",
    "powershell -enc",
    "powershell -encodedcommand",
    "curl | sh",
    "curl | bash",
    "wget | sh",
    "wget | bash",
    ":(){",
];

const HIGH_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -fr",
    "del /f",
    "del /s",
    "rmdir /s",
    "rd /s",
    "sudo ",
    "runas ",
    "git push --force",
    "git push -f",
    "git reset --hard",
    "git clean -fd",
    "chmod 777",
    "chmod -r",
    "chown ",
    "curl ",
    "wget ",
    "ssh ",
    "scp ",
    "taskkill",
    "stop-process",
    "kill -9",
    "format ",
];

fn is_broad_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive_absolute = bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic();
    path.contains("..") || path.starts_with('/') || path.starts_with('\\') || drive_absolute
}

fn is_read_only_command(normalized: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "ls",
        "dir",
        "cat",
        "type",
        "head",
        "tail",
        "grep",
        "rg",
        "find",
        "pwd",
        "wc",
        "stat",
        "whoami",
        "git status",
        "git diff",
        "git log",
        "git show",
        "git branch",
    ];
    matches_prefix(normalized, PREFIXES)
}

fn is_low_risk_command(normalized: &str) -> bool {
    const PREFIXES: &[&str] = &["echo", "mkdir", "touch", "cp", "copy"];
    matches_prefix(normalized, PREFIXES)
}

fn matches_prefix(normalized: &str, prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .any(|prefix| normalized == *prefix || normalized.starts_with(&format!("{prefix} ")))
}

fn has_shell_composition(normalized: &str) -> bool {
    normalized.contains('|')
        || normalized.contains('>')
        || normalized.contains('<')
        || normalized.contains(';')
        || normalized.contains("&&")
        || normalized.contains("$(")
        || normalized.contains('`')
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
    fn read_file_is_read_only_inside_workspace() {
        let assessment =
            StaticRiskClassifier.classify(&call("read_file", json!({ "path": "src/main.rs" })));
        assert_eq!(assessment.level, RiskLevel::ReadOnly);
    }

    #[test]
    fn read_file_outside_workspace_is_medium() {
        let assessment =
            StaticRiskClassifier.classify(&call("read_file", json!({ "path": "../secrets.txt" })));
        assert_eq!(assessment.level, RiskLevel::Medium);
    }

    #[test]
    fn write_file_outside_workspace_is_high() {
        let assessment = StaticRiskClassifier.classify(&call(
            "write_file",
            json!({ "path": "/etc/hosts", "content": "x" }),
        ));
        assert_eq!(assessment.level, RiskLevel::High);
    }

    #[test]
    fn read_only_command_is_read_only() {
        let assessment =
            StaticRiskClassifier.classify(&call("execute_code", json!({ "command": "ls -la" })));
        assert_eq!(assessment.level, RiskLevel::ReadOnly);
    }

    #[test]
    fn read_only_command_with_arguments_is_read_only() {
        for command in ["cat file.txt", "head notes.md", "git status --short"] {
            let assessment =
                StaticRiskClassifier.classify(&call("execute_code", json!({ "command": command })));
            assert_eq!(assessment.level, RiskLevel::ReadOnly, "{command}");
        }
    }

    #[test]
    fn composed_read_only_command_is_medium() {
        let assessment = StaticRiskClassifier.classify(&call(
            "execute_code",
            json!({ "command": "cat file | grep secret" }),
        ));
        assert_eq!(assessment.level, RiskLevel::Medium);
    }

    #[test]
    fn destructive_command_is_critical() {
        let assessment =
            StaticRiskClassifier.classify(&call("execute_code", json!({ "command": "rm -rf /" })));
        assert_eq!(assessment.level, RiskLevel::Critical);
    }

    #[test]
    fn force_push_is_high() {
        let assessment = StaticRiskClassifier.classify(&call(
            "execute_code",
            json!({ "command": "git push --force" }),
        ));
        assert_eq!(assessment.level, RiskLevel::High);
    }

    #[test]
    fn workspace_tools_have_static_risk_levels() {
        let classifier = StaticRiskClassifier;
        assert_eq!(
            classifier.classify(&call("list_agents", json!({}))).level,
            RiskLevel::ReadOnly
        );
        assert_eq!(
            classifier
                .classify(&call("list_templates", json!({})))
                .level,
            RiskLevel::ReadOnly
        );
        assert_eq!(
            classifier
                .classify(&call("create_channel", json!({ "name": "ops" })))
                .level,
            RiskLevel::Medium
        );
        assert_eq!(
            classifier
                .classify(&call(
                    "create_task",
                    json!({
                        "description": "x",
                        "expected_output": "y",
                        "assignee": "main"
                    })
                ))
                .level,
            RiskLevel::Medium
        );
        assert_eq!(
            classifier
                .classify(&call(
                    "instantiate_team",
                    json!({ "template": "software-team" })
                ))
                .level,
            RiskLevel::High
        );
    }

    #[test]
    fn unknown_tool_is_high() {
        let assessment =
            StaticRiskClassifier.classify(&call("mystery_tool", json!({ "input": "x" })));
        assert_eq!(assessment.level, RiskLevel::High);
    }
}
