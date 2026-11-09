use cowbot_core::agent::AgentProfile;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::template::TemplateDocument;
use cowbot_store::Store;

const SOFTWARE_TEAM: &str = include_str!("../templates/builtin/software-team.cowtemplate.json");
const RESEARCH_TEAM: &str = include_str!("../templates/builtin/research-team.cowtemplate.json");
const GENERAL_ASSISTANT: &str =
    include_str!("../templates/builtin/general-assistant.cowtemplate.json");

/// Load and validate the templates compiled into the binary.
pub fn builtin_templates() -> CowResult<Vec<TemplateDocument>> {
    let sources = [
        ("general-assistant", GENERAL_ASSISTANT),
        ("software-team", SOFTWARE_TEAM),
        ("research-team", RESEARCH_TEAM),
    ];

    let mut documents = Vec::with_capacity(sources.len());
    for (name, source) in sources {
        let document: TemplateDocument = serde_json::from_str(source).map_err(|err| {
            CowError::Config(format!("invalid built-in template '{name}': {err}"))
        })?;
        document.validate()?;
        documents.push(document);
    }
    Ok(documents)
}

/// Application service for template management and agent instantiation.
#[derive(Clone)]
pub struct TemplateService {
    store: Store,
}

impl TemplateService {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// List built-in templates followed by user templates, sorted by name.
    pub async fn list(&self) -> CowResult<Vec<TemplateDocument>> {
        let mut documents = builtin_templates()?;
        for stored in self.store.list_templates(None).await? {
            let document = TemplateDocument::from_stored(&stored)?;
            if !documents.iter().any(|existing| existing.id == document.id) {
                documents.push(document);
            }
        }
        documents.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(documents)
    }

    pub async fn get(&self, id: &str) -> CowResult<Option<TemplateDocument>> {
        if let Some(document) = builtin_templates()?
            .into_iter()
            .find(|template| template.id == id)
        {
            return Ok(Some(document));
        }
        self.store
            .get_template(id)
            .await?
            .as_ref()
            .map(TemplateDocument::from_stored)
            .transpose()
    }

    /// Validate and persist a user template. Built-in templates are read-only.
    pub async fn save(&self, document: &TemplateDocument) -> CowResult<()> {
        document.validate()?;
        if builtin_templates()?
            .iter()
            .any(|template| template.id == document.id)
        {
            return Err(CowError::Config(format!(
                "cannot overwrite built-in template '{}'",
                document.id
            )));
        }
        let stored = document.to_stored()?;
        self.store.upsert_template(&stored).await
    }

    /// Delete a user template. Built-in templates cannot be deleted.
    pub async fn delete(&self, id: &str) -> CowResult<()> {
        if builtin_templates()?
            .iter()
            .any(|template| template.id == id)
        {
            return Err(CowError::Config(format!(
                "cannot delete built-in template '{id}'"
            )));
        }
        self.store.delete_template(id).await
    }

    /// Parse, validate, and persist a `.cowtemplate` JSON document.
    pub async fn import_json(&self, json: &str) -> CowResult<TemplateDocument> {
        let document: TemplateDocument = serde_json::from_str(json)
            .map_err(|err| CowError::Config(format!("invalid template JSON: {err}")))?;
        self.save(&document).await?;
        Ok(document)
    }

    /// Serialize a template document to pretty-printed `.cowtemplate` JSON.
    pub fn export_json(document: &TemplateDocument) -> CowResult<String> {
        document.validate()?;
        Ok(serde_json::to_string_pretty(document)?)
    }

    /// Create agent profiles from a template and persist them in a workspace.
    pub async fn instantiate_agents(
        &self,
        workspace_id: &WorkspaceId,
        template_id: &str,
    ) -> CowResult<Vec<AgentProfile>> {
        let document = self
            .get(template_id)
            .await?
            .ok_or_else(|| CowError::Config(format!("template '{template_id}' was not found")))?;

        let mut profiles = Vec::with_capacity(document.agents.len());
        for agent in &document.agents {
            let profile = agent.to_profile();
            self.store.upsert_agent(workspace_id, &profile).await?;
            profiles.push(profile);
        }
        Ok(profiles)
    }
}
