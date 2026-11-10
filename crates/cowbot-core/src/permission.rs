use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::identity::WorkspaceId;
use crate::tool::ToolCall;

/// What a workspace permission rule decides for a matching tool call.
///
/// Priority is `Deny > Ask > Allow`; see [`matching_rule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleDecision {
    Allow,
    Ask,
    Deny,
}

impl RuleDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleDecision::Allow => "allow",
            RuleDecision::Ask => "ask",
            RuleDecision::Deny => "deny",
        }
    }
}

impl std::fmt::Display for RuleDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a permission rule came from.
///
/// Rules that arrive with an imported workspace package start disabled and
/// carry [`RuleSource::Imported`] until the user reviews them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    #[default]
    Local,
    Imported,
}

impl RuleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleSource::Local => "local",
            RuleSource::Imported => "imported",
        }
    }
}

/// A workspace-level allow / ask / deny rule for tool calls.
///
/// `tool` is an exact tool name or `"*"` for every tool. `command` is an
/// optional case-insensitive glob (`*` matches any run of characters) applied
/// to `execute_code` commands; rules on other tools do not carry one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub decision: RuleDecision,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub source: RuleSource,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_enabled() -> bool {
    true
}

impl PermissionRule {
    pub fn new(
        workspace_id: WorkspaceId,
        tool: impl Into<String>,
        command: Option<String>,
        decision: RuleDecision,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id,
            tool: tool.into().trim().to_string(),
            command: command
                .map(|command| command.trim().to_string())
                .filter(|command| !command.is_empty()),
            decision,
            enabled: true,
            source: RuleSource::Local,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_source(mut self, source: RuleSource) -> Self {
        self.source = source;
        self
    }

    /// Whether this rule applies to `call`, ignoring its enabled flag.
    pub fn matches(&self, call: &ToolCall) -> bool {
        if self.tool != "*" && self.tool != call.name {
            return false;
        }
        let Some(pattern) = self.command.as_deref() else {
            return true;
        };
        if call.name != "execute_code" {
            return false;
        }
        let Some(command) = call
            .arguments
            .get("command")
            .and_then(|value| value.as_str())
        else {
            return false;
        };
        glob_matches(pattern, command)
    }

    /// Higher values are more specific and win ties inside one decision.
    pub fn specificity(&self) -> u8 {
        let tool = u8::from(self.tool != "*") * 2;
        let command = match self.command.as_deref() {
            Some(pattern) if pattern.contains('*') => 1,
            Some(_) => 2,
            None => 0,
        };
        tool + command
    }

    /// Human-readable summary shown in audit entries and approval cards.
    pub fn describe(&self) -> String {
        match (&self.tool[..], self.command.as_deref()) {
            ("*", _) => "all tools".to_string(),
            (_, Some(command)) => format!("{}: {command}", self.tool),
            (tool, None) => tool.to_string(),
        }
    }
}

/// The rule that wins for a call: highest decision priority, then the most
/// specific pattern, then the oldest rule.
pub fn matching_rule<'a>(
    rules: &'a [PermissionRule],
    call: &ToolCall,
) -> Option<&'a PermissionRule> {
    rules
        .iter()
        .filter(|rule| rule.enabled && rule.matches(call))
        .max_by(|left, right| {
            left.decision
                .cmp(&right.decision)
                .then(left.specificity().cmp(&right.specificity()))
                .then(right.created_at.cmp(&left.created_at))
                .then(right.id.cmp(&left.id))
        })
}

/// The rule behind a decision, copied into approval events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRuleMatch {
    pub rule_id: String,
    pub description: String,
    pub decision: RuleDecision,
}

impl From<&PermissionRule> for PermissionRuleMatch {
    fn from(rule: &PermissionRule) -> Self {
        Self {
            rule_id: rule.id.clone(),
            description: rule.describe(),
            decision: rule.decision,
        }
    }
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// Match a case-insensitive command pattern where `*` matches any run of
/// characters. Whitespace is normalized on both sides.
pub fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = normalize(pattern);
    let value = normalize(value);
    if !pattern.contains('*') {
        return pattern == value;
    }

    let pattern = pattern.chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let (mut p, mut v) = (0, 0);
    let mut star: Option<usize> = None;
    let mut star_value = 0;

    while v < value.len() {
        if p < pattern.len() && pattern[p] == value[v] {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            star_value = v;
            p += 1;
        } else if let Some(star_index) = star {
            p = star_index + 1;
            star_value += 1;
            v = star_value;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
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

    fn rule(tool: &str, command: Option<&str>, decision: RuleDecision) -> PermissionRule {
        PermissionRule::new(
            WorkspaceId("workspace-1".to_string()),
            tool,
            command.map(str::to_string),
            decision,
        )
    }

    #[test]
    fn tool_rules_match_exact_names_and_wildcards() {
        let write = call("write_file", json!({"path": "a.txt"}));
        assert!(rule("write_file", None, RuleDecision::Ask).matches(&write));
        assert!(rule("*", None, RuleDecision::Ask).matches(&write));
        assert!(!rule("read_file", None, RuleDecision::Ask).matches(&write));
    }

    #[test]
    fn command_rules_match_execute_code_with_globs() {
        let status = call(
            "execute_code",
            json!({"command": "  Git   STATUS --short "}),
        );
        assert!(rule("execute_code", Some("git status*"), RuleDecision::Allow).matches(&status));
        assert!(rule("execute_code", Some("*status*"), RuleDecision::Allow).matches(&status));
        assert!(!rule("execute_code", Some("git diff*"), RuleDecision::Allow).matches(&status));
        assert!(!rule("write_file", Some("*"), RuleDecision::Allow).matches(&status));
    }

    #[test]
    fn precedence_is_deny_then_ask_then_allow() {
        let status = call("execute_code", json!({"command": "git status"}));
        let rules = vec![
            rule("execute_code", Some("git *"), RuleDecision::Allow),
            rule("execute_code", Some("git status"), RuleDecision::Ask),
            rule("*", None, RuleDecision::Deny),
        ];
        assert_eq!(
            matching_rule(&rules, &status).map(|rule| rule.decision),
            Some(RuleDecision::Deny)
        );

        let rules = vec![
            rule("execute_code", Some("git *"), RuleDecision::Allow),
            rule("execute_code", Some("git status"), RuleDecision::Ask),
        ];
        let matched = matching_rule(&rules, &status).unwrap();
        assert_eq!(matched.decision, RuleDecision::Ask);
        assert_eq!(matched.command.as_deref(), Some("git status"));
    }

    #[test]
    fn disabled_rules_are_ignored() {
        let status = call("execute_code", json!({"command": "git status"}));
        let rules = vec![rule("execute_code", None, RuleDecision::Deny).with_enabled(false)];
        assert!(matching_rule(&rules, &status).is_none());
    }

    #[test]
    fn descriptions_name_the_pattern() {
        assert_eq!(rule("*", None, RuleDecision::Allow).describe(), "all tools");
        assert_eq!(
            rule("execute_code", Some("git *"), RuleDecision::Ask).describe(),
            "execute_code: git *"
        );
        assert_eq!(
            rule("write_file", None, RuleDecision::Deny).describe(),
            "write_file"
        );
    }
}
