use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::agent::AgentProfile;
use crate::error::{CowError, CowResult};

/// Current schema version for template documents.
pub const TEMPLATE_SCHEMA_VERSION: u32 = 1;

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// Kind of reusable template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateKind {
    Agent,
    Team,
}

/// A stored agent or team template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub kind: TemplateKind,
    pub name: String,
    pub description: String,
    pub version: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

impl Template {
    pub fn new(kind: TemplateKind, name: impl Into<String>, payload: Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            kind,
            name: name.into(),
            description: String::new(),
            version: "1.0.0".to_string(),
            payload,
            created_at: Utc::now(),
        }
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// A portable template document (`.cowtemplate` JSON payload).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateDocument {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub kind: TemplateKind,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub agents: Vec<AgentTemplate>,
    #[serde(default)]
    pub tasks: Vec<TaskTemplate>,
}

impl TemplateDocument {
    /// Validate schema version, required fields, references, and acyclicity.
    pub fn validate(&self) -> CowResult<()> {
        if self.schema_version != TEMPLATE_SCHEMA_VERSION {
            return Err(CowError::Config(format!(
                "template '{}' uses unsupported schema_version {} (expected {})",
                self.id, self.schema_version, TEMPLATE_SCHEMA_VERSION
            )));
        }
        require_non_empty("template id", &self.id)?;
        require_non_empty("template name", &self.name)?;
        require_non_empty("template version", &self.version)?;

        match self.kind {
            TemplateKind::Agent => {
                if self.agents.len() != 1 {
                    return Err(CowError::Config(format!(
                        "agent template '{}' must contain exactly one agent",
                        self.id
                    )));
                }
                if !self.tasks.is_empty() {
                    return Err(CowError::Config(format!(
                        "agent template '{}' cannot define tasks",
                        self.id
                    )));
                }
            }
            TemplateKind::Team => {
                if self.agents.is_empty() {
                    return Err(CowError::Config(format!(
                        "team template '{}' must contain at least one agent",
                        self.id
                    )));
                }
            }
        }

        let mut agent_names = HashSet::new();
        for agent in &self.agents {
            require_non_empty("agent name", &agent.name)?;
            require_non_empty("agent role", &agent.role)?;
            require_non_empty("agent goal", &agent.goal)?;
            if agent.max_iterations == 0 {
                return Err(CowError::Config(format!(
                    "agent '{}' must have max_iterations greater than zero",
                    agent.name
                )));
            }
            if !agent_names.insert(agent.name.clone()) {
                return Err(CowError::Config(format!(
                    "template '{}' contains duplicate agent name '{}'",
                    self.id, agent.name
                )));
            }
            for tool in &agent.tools {
                require_non_empty("tool name", tool)?;
            }
        }

        let mut task_ids = HashSet::new();
        for task in &self.tasks {
            require_non_empty("task id", &task.id)?;
            require_non_empty("task description", &task.description)?;
            require_non_empty("task expected_output", &task.expected_output)?;
            if !agent_names.contains(&task.agent) {
                return Err(CowError::Config(format!(
                    "task '{}' references unknown agent '{}'",
                    task.id, task.agent
                )));
            }
            if task.depends_on.iter().any(|dep| dep == &task.id) {
                return Err(CowError::Config(format!(
                    "task '{}' cannot depend on itself",
                    task.id
                )));
            }
            if !task_ids.insert(task.id.clone()) {
                return Err(CowError::Config(format!(
                    "template '{}' contains duplicate task id '{}'",
                    self.id, task.id
                )));
            }
        }

        for task in &self.tasks {
            for dependency in &task.depends_on {
                if !task_ids.contains(dependency) {
                    return Err(CowError::Config(format!(
                        "task '{}' depends on unknown task '{}'",
                        task.id, dependency
                    )));
                }
            }
        }

        validate_acyclic(&self.tasks)
    }

    pub fn to_stored(&self) -> CowResult<Template> {
        Ok(Template {
            id: self.id.clone(),
            kind: self.kind,
            name: self.name.clone(),
            description: self.description.clone(),
            version: self.version.clone(),
            payload: serde_json::to_value(self)?,
            created_at: Utc::now(),
        })
    }

    pub fn from_stored(template: &Template) -> CowResult<Self> {
        let document: TemplateDocument =
            serde_json::from_value(template.payload.clone()).map_err(|err| {
                CowError::Config(format!(
                    "invalid template payload for '{}': {err}",
                    template.id
                ))
            })?;
        if document.id != template.id {
            return Err(CowError::Config(format!(
                "template payload id '{}' does not match stored id '{}'",
                document.id, template.id
            )));
        }
        document.validate()?;
        Ok(document)
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// A single agent inside a template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplate {
    pub name: String,
    pub role: String,
    pub goal: String,
    #[serde(default)]
    pub backstory: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub allow_delegation: bool,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

impl AgentTemplate {
    pub fn to_profile(&self) -> AgentProfile {
        let mut profile = AgentProfile::new(
            self.name.clone(),
            self.role.clone(),
            self.goal.clone(),
            self.backstory.clone(),
        );
        profile.model = self.model.clone();
        profile.tools = self.tools.clone();
        profile.allow_delegation = self.allow_delegation;
        profile.max_iterations = self.max_iterations;
        profile
    }
}

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// A task definition inside a team template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTemplate {
    pub id: String,
    pub description: String,
    pub expected_output: String,
    pub agent: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

fn default_schema_version() -> u32 {
    TEMPLATE_SCHEMA_VERSION
}

fn default_version() -> String {
    "1.0.0".to_string()
}

fn default_max_iterations() -> usize {
    15
}

fn require_non_empty(field: &str, value: &str) -> CowResult<()> {
    if value.trim().is_empty() {
        Err(CowError::Config(format!("{field} cannot be empty")))
    } else {
        Ok(())
    }
}

fn validate_acyclic(tasks: &[TaskTemplate]) -> CowResult<()> {
    let graph: HashMap<String, Vec<String>> = tasks
        .iter()
        .map(|task| (task.id.clone(), task.depends_on.clone()))
        .collect();
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();

    for task_id in graph.keys() {
        if has_cycle(task_id, &graph, &mut visiting, &mut visited) {
            return Err(CowError::Config(format!(
                "template contains a task dependency cycle involving '{task_id}'"
            )));
        }
    }
    Ok(())
}

fn has_cycle(
    node: &str,
    graph: &HashMap<String, Vec<String>>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
) -> bool {
    if visited.contains(node) {
        return false;
    }
    if !visiting.insert(node.to_string()) {
        return true;
    }
    if let Some(dependencies) = graph.get(node) {
        for dependency in dependencies {
            if has_cycle(dependency, graph, visiting, visited) {
                return true;
            }
        }
    }
    visiting.remove(node);
    visited.insert(node.to_string());
    false
}
