use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use cowbot_client::{Client, NewTask};
use cowbot_core::approval::ApprovalLevel;
use cowbot_core::error::CowError;
use cowbot_core::tool::{Tool, ToolCall, ToolResult};
use serde_json::{json, Value};

use crate::config;

fn required(call: &ToolCall, key: &str) -> Result<String, ToolResult> {
    call.arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ToolResult::failure(&call.id, format!("'{key}' is required")))
}

fn optional(call: &ToolCall, key: &str) -> Option<String> {
    call.arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn agent_names(agents: &[cowbot_core::agent::AgentProfile]) -> String {
    agents
        .iter()
        .map(|agent| format!("{} ({})", agent.name, agent.role))
        .collect::<Vec<_>>()
        .join(", ")
}

/// List the agents in the workspace.
#[derive(Clone)]
pub struct ListAgentsTool {
    client: Arc<dyn Client>,
}
impl ListAgentsTool {
    pub fn new(client: Arc<dyn Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for ListAgentsTool {
    fn name(&self) -> &str {
        "list_agents"
    }

    fn description(&self) -> &str {
        "List the agents in this CowBot workspace with their ids, names, roles, and enabled tools."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, CowError> {
        let agents = self.client.list_agents().await?;
        let value = agents
            .iter()
            .map(|agent| {
                json!({
                    "id": agent.id.0,
                    "name": agent.name,
                    "role": agent.role,
                    "tools": agent.tools,
                })
            })
            .collect::<Vec<_>>();
        Ok(ToolResult::success(
            &call.id,
            serde_json::to_string(&value)?,
        ))
    }
}

/// List the channels in the workspace.
#[derive(Clone)]
pub struct ListChannelsTool {
    client: Arc<dyn Client>,
}

impl ListChannelsTool {
    pub fn new(client: Arc<dyn Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for ListChannelsTool {
    fn name(&self) -> &str {
        "list_channels"
    }

    fn description(&self) -> &str {
        "List the channels in this CowBot workspace with their ids, names, topics, and whether each is an agent direct conversation."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, CowError> {
        let channels = self.client.list_channels().await?;
        let value = channels
            .iter()
            .map(|channel| {
                json!({
                    "id": channel.id.0,
                    "name": channel.name,
                    "topic": channel.topic,
                    "direct_conversation": channel.agent_id.is_some(),
                })
            })
            .collect::<Vec<_>>();
        Ok(ToolResult::success(
            &call.id,
            serde_json::to_string(&value)?,
        ))
    }
}

/// Delete a group channel. Direct conversations cannot be deleted.
#[derive(Clone)]
pub struct DeleteChannelTool {
    client: Arc<dyn Client>,
}

impl DeleteChannelTool {
    pub fn new(client: Arc<dyn Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for DeleteChannelTool {
    fn name(&self) -> &str {
        "delete_channel"
    }

    fn description(&self) -> &str {
        "Delete a group channel by name or id. Agent direct conversations cannot be deleted; clear their history instead. Every call requires human approval."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "channel": {"type": "string", "description": "Channel name or id"}
            },
            "required": ["channel"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, CowError> {
        let requested = match required(call, "channel") {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };

        let channels = self.client.list_channels().await?;
        let Some(channel) = channels.iter().find(|channel| {
            channel.name.eq_ignore_ascii_case(&requested)
                || channel.id.0.eq_ignore_ascii_case(&requested)
        }) else {
            return Ok(ToolResult::failure(
                &call.id,
                format!(
                    "No channel matches '{requested}'. Available channels: {}",
                    channels
                        .iter()
                        .map(|channel| channel.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        };

        if channel.agent_id.is_some() {
            return Ok(ToolResult::failure(
                &call.id,
                format!(
                    "'{}' is an agent direct conversation and cannot be deleted; clear its history instead.",
                    channel.name
                ),
            ));
        }

        match self.client.delete_channel(&channel.id).await {
            Ok(deleted) => Ok(ToolResult::success(
                &call.id,
                serde_json::to_string(&json!({
                    "id": deleted.id.0,
                    "name": deleted.name,
                    "topic": deleted.topic,
                    "deleted": true,
                }))?,
            )),
            Err(error) => Ok(ToolResult::failure(&call.id, error.to_string())),
        }
    }
}

/// List the built-in and stored templates available for starting a team.
#[derive(Clone)]
pub struct ListTemplatesTool {
    client: Arc<dyn Client>,
}

impl ListTemplatesTool {
    pub fn new(client: Arc<dyn Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for ListTemplatesTool {
    fn name(&self) -> &str {
        "list_templates"
    }

    fn description(&self) -> &str {
        "List the agent and team templates available in this workspace."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, CowError> {
        let templates = self.client.list_templates().await?;
        let value = templates
            .iter()
            .map(|template| {
                let agents = template
                    .agents
                    .iter()
                    .map(|agent| agent.name.as_str())
                    .collect::<Vec<_>>();
                json!({
                    "id": template.id,
                    "name": template.name,
                    "kind": template.kind,
                    "description": template.description,
                    "agents": agents,
                })
            })
            .collect::<Vec<_>>();
        Ok(ToolResult::success(
            &call.id,
            serde_json::to_string(&value)?,
        ))
    }
}

/// Create a group channel.
#[derive(Clone)]
pub struct CreateChannelTool {
    client: Arc<dyn Client>,
}

impl CreateChannelTool {
    pub fn new(client: Arc<dyn Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for CreateChannelTool {
    fn name(&self) -> &str {
        "create_channel"
    }

    fn description(&self) -> &str {
        "Create a group channel in the workspace. Names must be unique."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Channel name"},
                "topic": {"type": "string", "description": "Optional channel topic"}
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, CowError> {
        let name = match required(call, "name") {
            Ok(name) => name,
            Err(result) => return Ok(result),
        };
        let topic = optional(call, "topic").unwrap_or_default();
        match self.client.create_channel(&name, &topic).await {
            Ok(channel) => Ok(ToolResult::success(
                &call.id,
                serde_json::to_string(&json!({
                    "id": channel.id.0,
                    "name": channel.name,
                    "topic": channel.topic,
                }))?,
            )),
            Err(error) => Ok(ToolResult::failure(&call.id, error.to_string())),
        }
    }
}

/// Create a pending task for a sub-agent, optionally in a git worktree.
#[derive(Clone)]
pub struct CreateTaskTool {
    client: Arc<dyn Client>,
    config_path: PathBuf,
}

impl CreateTaskTool {
    pub fn new(client: Arc<dyn Client>, config_path: PathBuf) -> Self {
        Self {
            client,
            config_path,
        }
    }
}

#[async_trait]
impl Tool for CreateTaskTool {
    fn name(&self) -> &str {
        "create_task"
    }

    fn description(&self) -> &str {
        "Create a task for one of the workspace agents. The task is queued as pending; it is not run automatically. Set project to run it later in that project's git worktree."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "description": {"type": "string", "description": "What must be done"},
                "expected_output": {"type": "string", "description": "The deliverable"},
                "assignee": {"type": "string", "description": "Agent name or id"},
                "channel": {"type": "string", "description": "Optional channel name or id"},
                "project": {"type": "string", "description": "Optional registered project name or id; the task runs in a git worktree of that project"},
                "base_branch": {"type": "string", "description": "Base branch for the worktree; defaults to the project's current branch"}
            },
            "required": ["description", "expected_output", "assignee"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, CowError> {
        let description = match required(call, "description") {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };
        let expected = match required(call, "expected_output") {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };
        let assignee = match required(call, "assignee") {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };

        let agents = self.client.list_agents().await?;
        let Some(agent) = agents.iter().find(|agent| {
            agent.name.eq_ignore_ascii_case(&assignee) || agent.id.0.eq_ignore_ascii_case(&assignee)
        }) else {
            return Ok(ToolResult::failure(
                &call.id,
                format!(
                    "No agent matches '{assignee}'. Available agents: {}",
                    agent_names(&agents)
                ),
            ));
        };

        let requested_base = optional(call, "base_branch");
        let worktree = match optional(call, "project") {
            Some(requested) => {
                let config = config::load(&self.config_path).map_err(|error| {
                    CowError::Config(format!(
                        "failed to read {}: {error}",
                        self.config_path.display()
                    ))
                })?;
                let Some(project) = config.project(&requested) else {
                    let names = config
                        .projects
                        .iter()
                        .map(|project| project.name.as_str())
                        .collect::<Vec<_>>();
                    return Ok(ToolResult::failure(
                        &call.id,
                        format!(
                            "No project matches '{requested}'. Registered projects: {}",
                            if names.is_empty() {
                                "(none)".to_string()
                            } else {
                                names.join(", ")
                            }
                        ),
                    ));
                };
                let branches = config::project_branches(&project.path).map_err(|error| {
                    CowError::Config(format!(
                        "project '{}' is not available: {error}",
                        project.name
                    ))
                })?;
                let base_branch = match requested_base {
                    Some(requested) => {
                        if !branches.branches.iter().any(|branch| branch == &requested) {
                            return Ok(ToolResult::failure(
                                &call.id,
                                format!(
                                    "Base branch '{requested}' does not exist in '{}'. Available branches: {}",
                                    project.name,
                                    branches.branches.join(", ")
                                ),
                            ));
                        }
                        requested
                    }
                    None => branches
                        .current
                        .clone()
                        .or_else(|| branches.branches.first().cloned())
                        .ok_or_else(|| {
                            CowError::Config(format!(
                                "project '{}' has no local branches to base a worktree on",
                                project.name
                            ))
                        })?,
                };
                Some((project.clone(), base_branch))
            }
            None => {
                if requested_base.is_some() {
                    return Ok(ToolResult::failure(
                        &call.id,
                        "'base_branch' requires 'project'",
                    ));
                }
                None
            }
        };

        let mut task = NewTask::new(description, expected, agent.id.clone())
            .with_approval_level(ApprovalLevel::Ask);
        if let Some(channel_name) = optional(call, "channel") {
            let channels = self.client.list_channels().await?;
            let Some(channel) = channels.iter().find(|channel| {
                channel.name.eq_ignore_ascii_case(&channel_name)
                    || channel.id.0.eq_ignore_ascii_case(&channel_name)
            }) else {
                return Ok(ToolResult::failure(
                    &call.id,
                    format!(
                        "No channel matches '{channel_name}'. Available channels: {}",
                        channels
                            .iter()
                            .map(|channel| channel.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            };
            task = task.in_channel(channel.id.clone());
        }

        match self.client.create_task(&task).await {
            Ok(created) => {
                if let Some((project, base_branch)) = &worktree {
                    if let Err(error) = self
                        .client
                        .request_task_worktree(&created.id, &project.id, &project.path, base_branch)
                        .await
                    {
                        return Ok(ToolResult::failure(
                            &call.id,
                            format!(
                                "Task '{}' was created, but its worktree could not be requested: {error}",
                                created.id
                            ),
                        ));
                    }
                }
                Ok(ToolResult::success(
                    &call.id,
                    serde_json::to_string(&json!({
                        "id": created.id.0,
                        "status": created.status,
                        "assignee": agent.name,
                        "queued": true,
                        "worktree": worktree.as_ref().map(|(project, base_branch)| json!({
                            "project": project.name,
                            "project_id": project.id,
                            "base_branch": base_branch,
                            "requested": true,
                        })),
                    }))?,
                ))
            }
            Err(error) => Ok(ToolResult::failure(&call.id, error.to_string())),
        }
    }
}

/// Add the agents from a template to the workspace.
#[derive(Clone)]
pub struct InstantiateTeamTool {
    client: Arc<dyn Client>,
}

impl InstantiateTeamTool {
    pub fn new(client: Arc<dyn Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for InstantiateTeamTool {
    fn name(&self) -> &str {
        "instantiate_team"
    }

    fn description(&self) -> &str {
        "Add the agents from a template to the workspace and provision a direct conversation for each."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "template": {"type": "string", "description": "Template id or name"}
            },
            "required": ["template"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, CowError> {
        let requested = match required(call, "template") {
            Ok(value) => value,
            Err(result) => return Ok(result),
        };

        let templates = self.client.list_templates().await?;
        let Some(template) = templates.iter().find(|template| {
            template.id.eq_ignore_ascii_case(&requested)
                || template.name.eq_ignore_ascii_case(&requested)
        }) else {
            return Ok(ToolResult::failure(
                &call.id,
                format!(
                    "No template matches '{requested}'. Available templates: {}",
                    templates
                        .iter()
                        .map(|template| template.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        };

        let existing = self.client.list_agents().await?;
        let conflicts = template
            .agents
            .iter()
            .filter(|candidate| {
                existing
                    .iter()
                    .any(|agent| agent.name.eq_ignore_ascii_case(&candidate.name))
            })
            .map(|candidate| candidate.name.as_str())
            .collect::<Vec<_>>();
        if !conflicts.is_empty() {
            return Ok(ToolResult::failure(
                &call.id,
                format!(
                    "These agents already exist: {}. Edit them instead or choose another template.",
                    conflicts.join(", ")
                ),
            ));
        }

        match self.client.instantiate_agents(&template.id).await {
            Ok(added) => {
                let channels = self.client.list_channels().await?;
                let agents = added
                    .iter()
                    .map(|agent| {
                        let conversation = channels
                            .iter()
                            .find(|channel| channel.agent_id.as_ref() == Some(&agent.id))
                            .map(|channel| channel.id.0.clone());
                        json!({
                            "id": agent.id.0,
                            "name": agent.name,
                            "role": agent.role,
                            "conversation_id": conversation,
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(ToolResult::success(
                    &call.id,
                    serde_json::to_string(&json!({
                        "template": template.name,
                        "agents": agents,
                    }))?,
                ))
            }
            Err(error) => Ok(ToolResult::failure(&call.id, error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use cowbot_client::LocalClient;

    use super::*;
    use cowbot_core::task::TaskId;
    use cowbot_core::tool::ToolCall;
    use cowbot_core::worktree::WorktreeStatus;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            name: name.to_string(),
            arguments,
        }
    }

    fn output(result: &ToolResult) -> Value {
        assert!(result.success, "tool failed: {:?}", result.error);
        serde_json::from_str(&result.output).unwrap()
    }

    fn create_task_tool(client: Arc<dyn Client>) -> (CreateTaskTool, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let tool = CreateTaskTool::new(client, root.path().join("config.json"));
        (tool, root)
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_repository(root: &Path) -> std::path::PathBuf {
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);
        fs::write(repo.join("README.md"), "initial\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "initial"]);
        repo
    }

    #[tokio::test]
    async fn workspace_tools_create_a_channel_and_task() {
        let client = Arc::new(LocalClient::open_in_memory().await.unwrap());

        let channels = CreateChannelTool::new(client.clone());
        let result = channels
            .execute(&call(
                "create_channel",
                json!({"name": "ops", "topic": "Operations"}),
            ))
            .await
            .unwrap();
        assert_eq!(output(&result)["name"], "ops");

        let teams = InstantiateTeamTool::new(client.clone());
        let result = teams
            .execute(&call(
                "instantiate_team",
                json!({"template": "Software Development Team"}),
            ))
            .await
            .unwrap();
        let team = output(&result);
        assert_eq!(team["agents"].as_array().unwrap().len(), 3);

        let (tasks, _config) = create_task_tool(client.clone());
        let result = tasks
            .execute(&call(
                "create_task",
                json!({
                    "description": "Write the weekly update",
                    "expected_output": "A short update",
                    "assignee": "Planner",
                    "channel": "ops",
                }),
            ))
            .await
            .unwrap();
        let task = output(&result);
        assert_eq!(task["assignee"], "Planner");
        assert_eq!(task["queued"], true);
        assert!(task["worktree"].is_null());
    }

    #[tokio::test]
    async fn workspace_tools_report_unknown_agents() {
        let client = Arc::new(LocalClient::open_in_memory().await.unwrap());
        let (tasks, _config) = create_task_tool(client.clone());
        let result = tasks
            .execute(&call(
                "create_task",
                json!({
                    "description": "Write the weekly update",
                    "expected_output": "A short update",
                    "assignee": "Nobody",
                }),
            ))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("No agent matches"));
    }

    #[tokio::test]
    async fn workspace_tools_queue_tasks_into_a_worktree() {
        let client = Arc::new(LocalClient::open_in_memory().await.unwrap());
        client
            .instantiate_agents("builtin-software-team")
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let repo = init_repository(root.path());
        let config_path = root.path().join("config.json");
        let mut config = crate::config::AppConfig::default();
        let project = crate::config::add_project(&mut config, repo.to_str().unwrap()).unwrap();
        crate::config::save(&config_path, &config).unwrap();

        let tasks = CreateTaskTool::new(client.clone(), config_path);
        let result = tasks
            .execute(&call(
                "create_task",
                json!({
                    "description": "Add the login form",
                    "expected_output": "Working code",
                    "assignee": "Planner",
                    "project": project.name,
                    "base_branch": "main",
                }),
            ))
            .await
            .unwrap();
        let task = output(&result);
        assert_eq!(task["worktree"]["requested"], true);
        assert_eq!(task["worktree"]["base_branch"], "main");

        let task_id = task["id"].as_str().unwrap();
        let record = client
            .task_worktree(&TaskId::new(task_id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.status, WorktreeStatus::Requested);
        assert_eq!(record.base_branch, "main");
        assert_eq!(record.project_id, project.id);
    }

    #[tokio::test]
    async fn workspace_tools_reject_unknown_projects_and_branches() {
        let client = Arc::new(LocalClient::open_in_memory().await.unwrap());
        client
            .instantiate_agents("builtin-software-team")
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let repo = init_repository(root.path());
        let config_path = root.path().join("config.json");
        let mut config = crate::config::AppConfig::default();
        let project = crate::config::add_project(&mut config, repo.to_str().unwrap()).unwrap();
        crate::config::save(&config_path, &config).unwrap();
        let tasks = CreateTaskTool::new(client, config_path);

        let result = tasks
            .execute(&call(
                "create_task",
                json!({
                    "description": "Add the login form",
                    "expected_output": "Working code",
                    "assignee": "Planner",
                    "project": "missing",
                }),
            ))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("No project matches"));

        let result = tasks
            .execute(&call(
                "create_task",
                json!({
                    "description": "Add the login form",
                    "expected_output": "Working code",
                    "assignee": "Planner",
                    "project": project.name,
                    "base_branch": "missing-branch",
                }),
            ))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("does not exist"));
    }

    #[tokio::test]
    async fn workspace_tools_list_and_delete_group_channels() {
        let client = Arc::new(LocalClient::open_in_memory().await.unwrap());
        client.create_channel("ops", "Operations").await.unwrap();
        client
            .instantiate_agents("builtin-software-team")
            .await
            .unwrap();
        let channels = client.ensure_agent_channels().await.unwrap();

        let list = ListChannelsTool::new(client.clone())
            .execute(&call("list_channels", json!({})))
            .await
            .unwrap();
        let listed = output(&list);
        assert!(listed
            .as_array()
            .unwrap()
            .iter()
            .any(|channel| channel["name"] == "ops" && channel["direct_conversation"] == false));

        let delete = DeleteChannelTool::new(client.clone());
        let result = delete
            .execute(&call("delete_channel", json!({ "channel": "ops" })))
            .await
            .unwrap();
        assert_eq!(output(&result)["deleted"], true);

        let direct = channels
            .iter()
            .find(|channel| channel.agent_id.is_some())
            .expect("a direct conversation should exist");
        let result = delete
            .execute(&call(
                "delete_channel",
                json!({ "channel": direct.name.clone() }),
            ))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("direct conversation"));
    }

    #[tokio::test]
    async fn workspace_tools_list_agents_and_templates() {
        let client = Arc::new(LocalClient::open_in_memory().await.unwrap());
        let teams = InstantiateTeamTool::new(client.clone());
        teams
            .execute(&call(
                "instantiate_team",
                json!({"template": "builtin-software-team"}),
            ))
            .await
            .unwrap();
        let again = teams
            .execute(&call(
                "instantiate_team",
                json!({"template": "builtin-software-team"}),
            ))
            .await
            .unwrap();
        assert!(!again.success);
        assert!(again.error.unwrap().contains("already exist"));

        let agents = ListAgentsTool::new(client.clone())
            .execute(&call("list_agents", json!({})))
            .await
            .unwrap();
        assert_eq!(output(&agents).as_array().unwrap().len(), 3);

        let templates = ListTemplatesTool::new(client.clone())
            .execute(&call("list_templates", json!({})))
            .await
            .unwrap();
        assert!(output(&templates).as_array().unwrap().len() >= 3);
    }
}
