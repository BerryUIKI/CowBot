//! Local model provider configuration for the desktop app.
//!
//! The desktop reads and writes a single `config.json` in its app data
//! directory. The file holds an ordered list of OpenAI-compatible providers and
//! the id of the active one. API keys never cross the Tauri command boundary
//! back to the frontend; the UI only learns whether a provider has a stored key.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use cowbot_llm::{
    AnthropicProvider, LlmProvider, MockLlmProvider, OpenAiCompatibleProvider, ResponsesProvider,
};
use serde::{Deserialize, Serialize};

/// File name of the desktop configuration inside the app data directory.
pub const CONFIG_FILE: &str = "config.json";

/// Default model shown when no provider is active.
pub const DEFAULT_MODEL: &str = "gpt-4o";

/// Default workspace database, relative to the app data directory.
pub const DEFAULT_DATABASE: &str = "cowbot.db";

/// Directory holding imported workspace databases under the app data directory.
pub const WORKSPACES_DIR: &str = "workspaces";

/// One workspace database registered in the desktop switcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    /// Path relative to the app data directory, for example `cowbot.db`.
    pub database: String,
    pub name: String,
}

/// One local git repository that worktree tasks can target.
///
/// Projects are device-local: the path is never exported in a `.cowpkg`
/// package and never leaves this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub id: String,
    pub name: String,
    /// Canonical repository root.
    pub path: String,
}

/// A project as reported to the settings view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub path: String,
    pub current_branch: Option<String>,
    pub available: bool,
}

/// Branch listing for the worktree selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectBranches {
    pub current: Option<String>,
    pub branches: Vec<String>,
}

/// Wire protocol a saved provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProviderProtocol {
    /// OpenAI Chat Completions (`/v1/chat/completions`).
    #[default]
    #[serde(rename = "openai")]
    OpenAi,
    /// Anthropic Messages (`/v1/messages`).
    #[serde(rename = "anthropic")]
    Anthropic,
    /// OpenAI Responses (`/v1/responses`).
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
}

/// Optional context-window metadata for one model.
///
/// The desktop never guesses these values: a model without metadata is treated
/// as having an unknown window, and later context UI falls back to absolute
/// token counts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

/// One model provider saved by the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    /// Missing in files written before protocol selection existed; those
    /// providers are OpenAI-compatible.
    #[serde(default)]
    pub protocol: ProviderProtocol,
    pub base_url: String,
    /// Models known for this provider, fetched or added by hand.
    ///
    /// Missing in files written before model discovery existed; those
    /// providers load with their active model as the only entry.
    #[serde(default)]
    pub models: Vec<String>,
    /// The active model used for runs.
    pub model: String,
    /// Optional per-model context-window metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_metadata: BTreeMap<String, ModelMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// Whole desktop configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub active_provider_id: Option<String>,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// Whether the guided first-run setup has been completed or skipped.
    ///
    /// Legacy files written before onboarding existed are treated as completed
    /// so an upgrade does not show the guide to an existing user.
    #[serde(default = "onboarding_default_for_legacy")]
    pub onboarding_completed: bool,
    /// Whether the desktop checks GitHub Releases for a newer version at startup.
    #[serde(default = "auto_check_default")]
    pub auto_check_updates: bool,
    /// Database file (relative to the app data directory) the desktop opens.
    #[serde(default)]
    pub active_workspace_database: Option<String>,
    /// Workspaces offered by the desktop switcher.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
    /// Local git repositories available to worktree tasks.
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

fn onboarding_default_for_legacy() -> bool {
    true
}

fn auto_check_default() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            active_provider_id: None,
            providers: Vec::new(),
            onboarding_completed: false,
            auto_check_updates: true,
            active_workspace_database: None,
            workspaces: Vec::new(),
            projects: Vec::new(),
        }
    }
}

impl AppConfig {
    /// Look up a provider by id.
    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|provider| provider.id == id)
    }

    /// Look up a registered project by id or name.
    pub fn project(&self, id_or_name: &str) -> Option<&ProjectEntry> {
        self.projects.iter().find(|project| {
            project.id.eq_ignore_ascii_case(id_or_name)
                || project.name.eq_ignore_ascii_case(id_or_name)
        })
    }

    /// The active provider, if one is configured and still present.
    pub fn active_provider(&self) -> Option<&ProviderConfig> {
        let id = self.active_provider_id.as_deref()?;
        self.provider(id)
    }
}

/// Which backend an effective model configuration uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Mock,
    Remote(ProviderProtocol),
}

/// The model configuration currently in effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModel {
    pub kind: ProviderKind,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

/// Path of the configuration file inside the app data directory.
pub fn config_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONFIG_FILE)
}

/// Resolve a workspace database path relative to the app data directory.
///
/// Only plain relative paths are accepted; absolute paths, drive prefixes, and
/// parent components are rejected so a configuration file cannot point outside
/// the app data directory.
pub fn resolve_database_path(data_dir: &Path, database: &str) -> Result<PathBuf, String> {
    let relative = Path::new(database);
    if database.trim().is_empty() || relative.is_absolute() {
        return Err(
            "workspace database path must be relative to the app data directory".to_string(),
        );
    }
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(
            "workspace database path must not contain root or parent components".to_string(),
        );
    }
    Ok(data_dir.join(relative))
}

/// Load the configuration, treating a missing file as the default.
pub fn load(path: &Path) -> io::Result<AppConfig> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let mut config: AppConfig = serde_json::from_str(&contents).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid configuration: {error}"),
                )
            })?;
            for provider in &mut config.providers {
                provider.models = normalized_models(&provider.models, &provider.model);
            }
            Ok(config)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(AppConfig::default()),
        Err(error) => Err(error),
    }
}

/// Write the configuration atomically with owner-only permissions where the
/// platform supports it.
pub fn save(path: &Path, config: &AppConfig) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let contents = serde_json::to_string_pretty(config)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let temporary = path.with_extension("json.tmp");
    write_private(&temporary, &contents)?;
    restrict_permissions(&temporary)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Normalize a provider's model list: trim entries, drop blanks and
/// duplicates, and make sure the active model is present.
fn normalized_models(models: &[String], active_model: &str) -> Vec<String> {
    let mut normalized: Vec<String> = Vec::new();
    for model in models {
        let model = model.trim();
        if !model.is_empty() && !normalized.iter().any(|known| known == model) {
            normalized.push(model.to_string());
        }
    }
    if !normalized.iter().any(|model| model == active_model) {
        normalized.push(active_model.to_string());
    }
    normalized
}

/// Validated provider fields, trimmed and normalized.
pub fn validate_endpoint(base_url: &str, model: &str) -> Result<(String, String), String> {
    let base_url = normalize_base_url(base_url)?;
    let model = model.trim();
    if model.is_empty() {
        return Err("Model is required.".to_string());
    }
    Ok((base_url, model.to_string()))
}

/// Normalize a base URL, rejecting anything that is not an absolute HTTP(S) URL.
pub fn normalize_base_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let host = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"));
    match host {
        Some(rest) if !rest.is_empty() => Ok(trimmed.to_string()),
        _ => Err("Base URL must start with http:// or https:// and include a host.".to_string()),
    }
}

/// Trim a typed API key, treating a blank value as absent.
pub fn normalize_api_key(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// Build a validated provider.
///
/// The explicit parameters mirror the persisted provider fields; grouping them
/// into a temporary struct would not add validation value.
#[allow(clippy::too_many_arguments)]
pub fn make_provider(
    id: String,
    name: &str,
    protocol: ProviderProtocol,
    base_url: &str,
    models: &[String],
    model: &str,
    api_key: Option<&str>,
    model_metadata: BTreeMap<String, ModelMetadata>,
) -> Result<ProviderConfig, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Provider name is required.".to_string());
    }
    let (base_url, model) = validate_endpoint(base_url, model)?;
    Ok(ProviderConfig {
        id,
        name: name.to_string(),
        protocol,
        base_url,
        models: normalized_models(models, &model),
        model,
        model_metadata: normalized_metadata(model_metadata),
        api_key: normalize_api_key(api_key),
    })
}

fn normalized_metadata(
    metadata: BTreeMap<String, ModelMetadata>,
) -> BTreeMap<String, ModelMetadata> {
    metadata
        .into_iter()
        .filter(|(_, metadata)| {
            metadata.context_window.is_some() || metadata.max_output_tokens.is_some()
        })
        .collect()
}

/// Generate an id for a new provider that does not collide with existing ones.
pub fn new_provider_id(existing: &[ProviderConfig]) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    provider_id_for(millis, existing)
}

fn provider_id_for(millis: u128, existing: &[ProviderConfig]) -> String {
    let mut candidate = format!("provider-{millis}");
    let mut suffix = 1;
    while existing.iter().any(|provider| provider.id == candidate) {
        candidate = format!("provider-{millis}-{suffix}");
        suffix += 1;
    }
    candidate
}

/// Generate an id for a new project that does not collide with existing ones.
pub fn new_project_id(existing: &[ProjectEntry]) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    project_id_for(millis, existing)
}

fn project_id_for(millis: u128, existing: &[ProjectEntry]) -> String {
    let mut candidate = format!("project-{millis}");
    let mut suffix = 1;
    while existing.iter().any(|project| project.id == candidate) {
        candidate = format!("project-{millis}-{suffix}");
        suffix += 1;
    }
    candidate
}

/// Run a git command in `directory`, returning trimmed stdout.
fn git_output(directory: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .map_err(|error| {
            format!("CowBot could not run git: {error}. Install git and make sure it is on PATH.")
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            stderr
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Resolve a folder to the root of the git work tree that contains it.
pub fn resolve_project_root(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Choose a folder to register.".to_string());
    }
    let folder = Path::new(trimmed);
    if !folder.is_dir() {
        return Err(format!(
            "`{trimmed}` is not a folder. Choose an existing repository folder."
        ));
    }
    let root = git_output(folder, &["rev-parse", "--show-toplevel"]).map_err(|error| {
        format!("`{trimmed}` is not inside a git work tree. Choose a repository folder. ({error})")
    })?;
    let root = PathBuf::from(root);
    if root.as_os_str().is_empty() {
        return Err(format!(
            "`{trimmed}` is not inside a git work tree. Choose a repository folder."
        ));
    }
    fs::canonicalize(&root).map_err(|error| {
        format!(
            "failed to resolve the repository root {}: {error}",
            root.display()
        )
    })
}

fn project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Project")
        .to_string()
}

fn same_path(left: &str, right: &str) -> bool {
    #[cfg(windows)]
    {
        left.eq_ignore_ascii_case(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// Register a git repository. The returned entry is added to `config`.
pub fn add_project(config: &mut AppConfig, path: &str) -> Result<ProjectEntry, String> {
    let root = resolve_project_root(path)?;
    let canonical = root.display().to_string();
    if let Some(existing) = config
        .projects
        .iter()
        .find(|project| same_path(&project.path, &canonical))
    {
        return Err(format!(
            "`{canonical}` is already registered as '{}'.",
            existing.name
        ));
    }

    let entry = ProjectEntry {
        id: new_project_id(&config.projects),
        name: project_name(&root),
        path: canonical,
    };
    config.projects.push(entry.clone());
    Ok(entry)
}

/// Remove a project from the registry. Files are never touched.
pub fn remove_project(config: &mut AppConfig, id_or_name: &str) -> Result<ProjectEntry, String> {
    let index = config
        .projects
        .iter()
        .position(|project| {
            project.id.eq_ignore_ascii_case(id_or_name)
                || project.name.eq_ignore_ascii_case(id_or_name)
        })
        .ok_or_else(|| format!("project '{id_or_name}' was not found"))?;
    Ok(config.projects.remove(index))
}

/// The branch a project currently has checked out, when it can be read.
pub fn project_current_branch(path: &str) -> Option<String> {
    let directory = Path::new(path);
    if !directory.is_dir() {
        return None;
    }
    git_output(directory, &["symbolic-ref", "--short", "HEAD"])
        .or_else(|_| git_output(directory, &["rev-parse", "--abbrev-ref", "HEAD"]))
        .ok()
        .filter(|branch| !branch.is_empty() && branch != "HEAD")
}

/// Local branches of a project plus the current one.
pub fn project_branches(path: &str) -> Result<ProjectBranches, String> {
    let directory = Path::new(path);
    if !directory.is_dir() {
        return Err(format!("`{path}` is not available on this machine."));
    }
    let current = project_current_branch(path);
    let listing = git_output(
        directory,
        &["for-each-ref", "refs/heads", "--format=%(refname:short)"],
    )?;
    let mut branches = listing
        .lines()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if let Some(current) = &current {
        if !branches.iter().any(|branch| branch == current) {
            branches.push(current.clone());
        }
    }
    Ok(ProjectBranches { current, branches })
}

/// Report a project's availability and current branch for the settings view.
pub fn project_view(project: &ProjectEntry) -> ProjectView {
    let available = resolve_project_root(&project.path).is_ok();
    ProjectView {
        id: project.id.clone(),
        name: project.name.clone(),
        path: project.path.clone(),
        current_branch: available
            .then(|| project_current_branch(&project.path))
            .flatten(),
        available,
    }
}

/// Drop stale worktree administrative entries for a project.
///
/// Missing folders are already reconciled in the database; this only clears
/// git's own bookkeeping so a new `worktree add` is not blocked by a stale
/// entry.
pub fn prune_project_worktrees(project: &ProjectEntry) {
    let directory = Path::new(&project.path);
    if directory.is_dir() {
        let _ = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["worktree", "prune"])
            .output();
    }
}

/// Resolve the active provider, falling back to the offline mock model.
pub fn resolve_active(config: &AppConfig) -> ResolvedModel {
    match config.active_provider() {
        Some(provider) => ResolvedModel {
            kind: ProviderKind::Remote(provider.protocol),
            model: provider.model.clone(),
            base_url: Some(provider.base_url.clone()),
            api_key: provider.api_key.clone(),
        },
        None => ResolvedModel {
            kind: ProviderKind::Mock,
            model: DEFAULT_MODEL.to_string(),
            base_url: None,
            api_key: None,
        },
    }
}

/// Build the LLM client for an effective model configuration.
pub fn build_provider(resolved: &ResolvedModel) -> Arc<dyn LlmProvider> {
    match resolved.kind {
        ProviderKind::Mock => Arc::new(MockLlmProvider::echo()),
        ProviderKind::Remote(protocol) => build_remote_provider(
            protocol,
            resolved.base_url.as_deref().unwrap_or_default(),
            resolved.api_key.clone(),
        ),
    }
}

/// Build a remote LLM client for a protocol and endpoint.
pub fn build_remote_provider(
    protocol: ProviderProtocol,
    base_url: &str,
    api_key: Option<String>,
) -> Arc<dyn LlmProvider> {
    match protocol {
        ProviderProtocol::OpenAi => Arc::new(OpenAiCompatibleProvider::new(base_url, api_key)),
        ProviderProtocol::Anthropic => Arc::new(AnthropicProvider::new(base_url, api_key)),
        ProviderProtocol::OpenAiResponses => Arc::new(ResponsesProvider::new(base_url, api_key)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_provider(id: &str, name: &str) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            name: name.to_string(),
            protocol: ProviderProtocol::OpenAi,
            base_url: "https://api.example.com/v1".to_string(),
            models: vec!["example-model".to_string()],
            model: "example-model".to_string(),
            model_metadata: BTreeMap::new(),
            api_key: Some("secret".to_string()),
        }
    }

    #[test]
    fn missing_file_loads_defaults() {
        let root = tempfile::tempdir().unwrap();
        let config = load(&root.path().join(CONFIG_FILE)).unwrap();
        assert_eq!(config, AppConfig::default());
        assert!(config.providers.is_empty());
        assert!(config.active_provider_id.is_none());
        assert!(!config.onboarding_completed);
        assert!(config.auto_check_updates);
    }

    #[test]
    fn legacy_config_files_are_treated_as_onboarded() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        fs::write(
            &path,
            r#"{"version":1,"active_provider_id":null,"providers":[]}"#,
        )
        .unwrap();

        assert!(load(&path).unwrap().onboarding_completed);
    }

    #[test]
    fn explicit_onboarding_state_is_preserved() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        fs::write(
            &path,
            r#"{"version":1,"providers":[],"onboarding_completed":false}"#,
        )
        .unwrap();

        assert!(!load(&path).unwrap().onboarding_completed);
    }

    #[test]
    fn legacy_config_files_enable_update_checks() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        fs::write(&path, r#"{"version":1,"providers":[]}"#).unwrap();

        assert!(load(&path).unwrap().auto_check_updates);
    }

    #[test]
    fn explicit_update_preference_is_preserved() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        fs::write(
            &path,
            r#"{"version":1,"providers":[],"auto_check_updates":false}"#,
        )
        .unwrap();

        assert!(!load(&path).unwrap().auto_check_updates);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        let config = AppConfig {
            active_provider_id: Some("one".to_string()),
            providers: vec![sample_provider("one", "Example")],
            onboarding_completed: true,
            auto_check_updates: true,
            active_workspace_database: Some("workspaces/one.db".to_string()),
            workspaces: vec![WorkspaceEntry {
                database: "workspaces/one.db".to_string(),
                name: "Team".to_string(),
            }],
            projects: vec![ProjectEntry {
                id: "project-1".to_string(),
                name: "cowbot".to_string(),
                path: "/repos/cowbot".to_string(),
            }],
        };

        save(&path, &config).unwrap();

        assert_eq!(load(&path).unwrap(), config);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn load_tolerates_unknown_fields() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        fs::write(
            &path,
            r#"{"version":1,"active_provider_id":null,"providers":[],"future":true}"#,
        )
        .unwrap();

        let config = load(&path).unwrap();
        assert!(config.providers.is_empty());
    }

    #[test]
    fn legacy_files_have_no_workspace_registry() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        fs::write(&path, r#"{"version":1,"providers":[]}"#).unwrap();

        let config = load(&path).unwrap();

        assert!(config.active_workspace_database.is_none());
        assert!(config.workspaces.is_empty());
        assert!(config.projects.is_empty());
    }

    #[test]
    fn workspace_database_paths_stay_inside_app_data() {
        let root = Path::new("app-data");

        assert_eq!(
            resolve_database_path(root, DEFAULT_DATABASE).unwrap(),
            root.join(DEFAULT_DATABASE)
        );
        assert_eq!(
            resolve_database_path(root, "workspaces/one.db").unwrap(),
            root.join("workspaces/one.db")
        );
        assert!(resolve_database_path(root, "").is_err());
        assert!(resolve_database_path(root, "..").is_err());
        assert!(resolve_database_path(root, "../escape.db").is_err());
        assert!(resolve_database_path(root, "/absolute.db").is_err());
    }

    #[test]
    fn providers_without_a_protocol_default_to_openai() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        fs::write(
            &path,
            r#"{"version":1,"active_provider_id":"p1","providers":[{"id":"p1","name":"Legacy","base_url":"https://api.example.com/v1","model":"m"}]}"#,
        )
        .unwrap();

        let config = load(&path).unwrap();

        assert_eq!(config.providers[0].protocol, ProviderProtocol::OpenAi);
    }

    #[test]
    fn protocols_serialize_to_stable_names() {
        let config = AppConfig {
            providers: vec![
                ProviderConfig {
                    protocol: ProviderProtocol::OpenAi,
                    ..sample_provider("one", "One")
                },
                ProviderConfig {
                    protocol: ProviderProtocol::Anthropic,
                    ..sample_provider("two", "Two")
                },
                ProviderConfig {
                    protocol: ProviderProtocol::OpenAiResponses,
                    ..sample_provider("three", "Three")
                },
            ],
            ..AppConfig::default()
        };

        let value = serde_json::to_value(&config).unwrap();

        assert_eq!(value["providers"][0]["protocol"], "openai");
        assert_eq!(value["providers"][1]["protocol"], "anthropic");
        assert_eq!(value["providers"][2]["protocol"], "openai-responses");
    }

    #[test]
    fn resolve_uses_the_active_provider() {
        let config = AppConfig {
            providers: vec![sample_provider("one", "One"), sample_provider("two", "Two")],
            active_provider_id: Some("two".to_string()),
            ..AppConfig::default()
        };

        let resolved = resolve_active(&config);

        assert_eq!(
            resolved.kind,
            ProviderKind::Remote(ProviderProtocol::OpenAi)
        );
        assert_eq!(resolved.model, "example-model");
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(resolved.api_key.as_deref(), Some("secret"));
    }

    #[test]
    fn resolve_carries_the_provider_protocol() {
        let config = AppConfig {
            providers: vec![ProviderConfig {
                protocol: ProviderProtocol::Anthropic,
                ..sample_provider("claude", "Claude")
            }],
            active_provider_id: Some("claude".to_string()),
            ..AppConfig::default()
        };

        assert_eq!(
            resolve_active(&config).kind,
            ProviderKind::Remote(ProviderProtocol::Anthropic)
        );
    }

    #[test]
    fn resolve_falls_back_to_mock_without_an_active_provider() {
        let config = AppConfig {
            providers: vec![sample_provider("one", "One")],
            ..AppConfig::default()
        };

        let resolved = resolve_active(&config);

        assert_eq!(resolved.kind, ProviderKind::Mock);
        assert_eq!(resolved.model, DEFAULT_MODEL);
        assert!(resolved.base_url.is_none());
        assert!(resolved.api_key.is_none());
    }

    #[test]
    fn resolve_falls_back_to_mock_when_the_active_id_is_unknown() {
        let config = AppConfig {
            providers: vec![sample_provider("one", "One")],
            active_provider_id: Some("missing".to_string()),
            ..AppConfig::default()
        };

        assert_eq!(resolve_active(&config).kind, ProviderKind::Mock);
    }

    #[test]
    fn providers_without_models_load_with_the_active_model() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        fs::write(
            &path,
            r#"{"version":1,"active_provider_id":"p1","providers":[{"id":"p1","name":"Legacy","base_url":"https://api.example.com/v1","model":"legacy-model"}]}"#,
        )
        .unwrap();

        let config = load(&path).unwrap();

        assert_eq!(config.providers[0].models, vec!["legacy-model".to_string()]);
    }

    #[test]
    fn loaded_models_are_deduplicated_and_include_the_active_one() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        fs::write(
            &path,
            r#"{"version":1,"active_provider_id":"p1","providers":[{"id":"p1","name":"Hand edited","base_url":"https://api.example.com/v1","models":["gpt-4o"," gpt-4o ","","gpt-4o-mini"],"model":"gpt-4o-mini"}]}"#,
        )
        .unwrap();

        let config = load(&path).unwrap();

        assert_eq!(
            config.providers[0].models,
            vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()]
        );
    }

    #[test]
    fn saved_models_are_normalized_and_include_the_active_model() {
        let provider = make_provider(
            "id".into(),
            "Name",
            ProviderProtocol::OpenAi,
            "https://a.test/v1",
            &[
                "gpt-4o".to_string(),
                " gpt-4o ".to_string(),
                " ".to_string(),
                "gpt-4o-mini".to_string(),
            ],
            "gpt-4o-mini",
            None,
            BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(
            provider.models,
            vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()]
        );

        let provider = make_provider(
            "id".into(),
            "Name",
            ProviderProtocol::OpenAi,
            "https://a.test/v1",
            &["gpt-4o".to_string()],
            "o3",
            None,
            BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(
            provider.models,
            vec!["gpt-4o".to_string(), "o3".to_string()]
        );
    }

    #[test]
    fn legacy_provider_files_load_without_model_metadata() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        fs::write(
            &path,
            r#"{"version":1,"active_provider_id":"p1","providers":[{"id":"p1","name":"Legacy","base_url":"https://api.example.com/v1","model":"m"}]}"#,
        )
        .unwrap();

        let config = load(&path).unwrap();
        assert!(config.providers[0].model_metadata.is_empty());
    }

    #[test]
    fn model_metadata_round_trips_and_empty_values_are_dropped() {
        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        let mut metadata = BTreeMap::new();
        metadata.insert(
            "deepseek-chat".to_string(),
            ModelMetadata {
                context_window: Some(128_000),
                max_output_tokens: Some(8_192),
            },
        );
        metadata.insert("empty-model".to_string(), ModelMetadata::default());

        let provider = make_provider(
            "p1".into(),
            "DeepSeek",
            ProviderProtocol::OpenAi,
            "https://api.deepseek.com",
            &["deepseek-chat".to_string(), "empty-model".to_string()],
            "deepseek-chat",
            None,
            metadata,
        )
        .unwrap();
        assert_eq!(provider.model_metadata.len(), 1);

        let config = AppConfig {
            providers: vec![provider],
            active_provider_id: Some("p1".to_string()),
            ..AppConfig::default()
        };
        save(&path, &config).unwrap();

        assert_eq!(load(&path).unwrap(), config);
        let raw = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let metadata = &value["providers"][0]["model_metadata"];
        assert_eq!(metadata["deepseek-chat"]["context_window"], 128_000);
        assert!(metadata.get("empty-model").is_none());
    }

    #[test]
    fn base_url_requires_an_absolute_http_url() {
        assert!(normalize_base_url("api.example.com/v1").is_err());
        assert!(normalize_base_url("ftp://api.example.com").is_err());
        assert!(normalize_base_url("https://").is_err());
        assert_eq!(
            normalize_base_url("https://api.example.com/v1/").unwrap(),
            "https://api.example.com/v1"
        );
    }

    #[test]
    fn provider_requires_a_name_model_and_url() {
        assert!(make_provider(
            "id".into(),
            " ",
            ProviderProtocol::OpenAi,
            "https://a.test/v1",
            &[],
            "m",
            None,
            BTreeMap::new()
        )
        .is_err());
        assert!(make_provider(
            "id".into(),
            "Name",
            ProviderProtocol::OpenAi,
            "",
            &[],
            "m",
            None,
            BTreeMap::new()
        )
        .is_err());
        assert!(make_provider(
            "id".into(),
            "Name",
            ProviderProtocol::OpenAi,
            "https://a.test/v1",
            &[],
            " ",
            None,
            BTreeMap::new()
        )
        .is_err());
        assert!(make_provider(
            "id".into(),
            "Name",
            ProviderProtocol::Anthropic,
            "https://a.test/v1",
            &[],
            "m",
            None,
            BTreeMap::new()
        )
        .is_ok());
    }

    #[test]
    fn blank_api_keys_are_treated_as_absent() {
        assert_eq!(normalize_api_key(None), None);
        assert_eq!(normalize_api_key(Some("   ")), None);
        assert_eq!(normalize_api_key(Some(" key ")), Some("key".to_string()));
    }

    #[test]
    fn new_provider_ids_do_not_collide() {
        let existing = vec![sample_provider("provider-42", "One")];
        assert_eq!(provider_id_for(42, &existing), "provider-42-1");
        assert_eq!(provider_id_for(43, &existing), "provider-43");
        assert!(new_provider_id(&existing).starts_with("provider-"));
    }

    fn init_repository(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        fs::write(repo.join("README.md"), "initial\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "initial"]);
        repo
    }

    #[test]
    fn project_ids_do_not_collide() {
        let existing = vec![ProjectEntry {
            id: "project-42".to_string(),
            name: "one".to_string(),
            path: "/one".to_string(),
        }];
        assert_eq!(project_id_for(42, &existing), "project-42-1");
        assert_eq!(project_id_for(43, &existing), "project-43");
        assert!(new_project_id(&existing).starts_with("project-"));
    }

    #[test]
    fn resolving_a_folder_uses_the_repository_root() {
        let root = tempfile::tempdir().unwrap();
        let repo = init_repository(root.path());
        let nested = repo.join("apps").join("web");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            resolve_project_root(nested.to_str().unwrap()).unwrap(),
            fs::canonicalize(&repo).unwrap()
        );
    }

    #[test]
    fn registering_projects_rejects_duplicates_and_non_repositories() {
        let root = tempfile::tempdir().unwrap();
        let repo = init_repository(root.path());
        let plain = root.path().join("plain");
        fs::create_dir_all(&plain).unwrap();

        let mut config = AppConfig::default();
        let added = add_project(&mut config, repo.to_str().unwrap()).unwrap();
        assert_eq!(added.name, "repo");
        assert_eq!(config.projects.len(), 1);
        assert_eq!(
            config.project("repo").map(|project| project.id.as_str()),
            Some(added.id.as_str())
        );

        let nested = repo.join("apps");
        fs::create_dir_all(&nested).unwrap();
        let duplicate = add_project(&mut config, nested.to_str().unwrap()).unwrap_err();
        assert!(duplicate.contains("already registered"), "{duplicate}");

        let error = add_project(&mut config, plain.to_str().unwrap()).unwrap_err();
        assert!(error.contains("not inside a git work tree"), "{error}");
        assert_eq!(config.projects.len(), 1);
    }

    #[test]
    fn project_branches_include_the_current_branch() {
        let root = tempfile::tempdir().unwrap();
        let repo = init_repository(root.path());
        let output = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["branch", "feature/other"])
            .output()
            .unwrap();
        assert!(output.status.success());

        let listing = project_branches(repo.to_str().unwrap()).unwrap();

        assert_eq!(listing.current.as_deref(), Some("main"));
        assert!(listing.branches.iter().any(|branch| branch == "main"));
        assert!(listing
            .branches
            .iter()
            .any(|branch| branch == "feature/other"));
    }

    #[test]
    fn removing_a_project_keeps_its_files() {
        let root = tempfile::tempdir().unwrap();
        let repo = init_repository(root.path());
        let mut config = AppConfig::default();
        let added = add_project(&mut config, repo.to_str().unwrap()).unwrap();

        let removed = remove_project(&mut config, "repo").unwrap();

        assert_eq!(removed, added);
        assert!(config.projects.is_empty());
        assert!(repo.exists());
        assert!(remove_project(&mut config, "repo").is_err());
    }

    #[test]
    fn project_views_report_availability() {
        let root = tempfile::tempdir().unwrap();
        let repo = init_repository(root.path());
        let mut config = AppConfig::default();
        let added = add_project(&mut config, repo.to_str().unwrap()).unwrap();

        let view = project_view(&added);
        assert!(view.available);
        assert_eq!(view.current_branch.as_deref(), Some("main"));

        let missing = ProjectEntry {
            id: "project-missing".to_string(),
            name: "missing".to_string(),
            path: root.path().join("gone").display().to_string(),
        };
        let view = project_view(&missing);
        assert!(!view.available);
        assert!(view.current_branch.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn saved_config_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = config_path(root.path());
        save(&path, &AppConfig::default()).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
