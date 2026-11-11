use chrono::Utc;
use cowbot_core::agent::{AgentId, AgentProfile};
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::identity::WorkspaceId;
use cowbot_core::template::{Template, TemplateKind};

use crate::error::{db, parse_datetime, parse_json};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct AgentRow {
    id: String,
    name: String,
    role: String,
    goal: String,
    backstory: String,
    model: String,
    tools: String,
    allow_delegation: i64,
    max_iterations: i64,
    verbose: i64,
}

impl AgentRow {
    fn into_domain(self) -> CowResult<AgentProfile> {
        Ok(AgentProfile {
            id: AgentId(self.id),
            name: self.name,
            role: self.role,
            goal: self.goal,
            backstory: self.backstory,
            model: if self.model.is_empty() {
                None
            } else {
                Some(self.model)
            },
            tools: parse_json(&self.tools, "agent_profiles.tools")?,
            allow_delegation: self.allow_delegation != 0,
            max_iterations: self.max_iterations.max(0) as usize,
            verbose: self.verbose != 0,
        })
    }
}

#[derive(sqlx::FromRow)]
struct TemplateRow {
    id: String,
    kind: String,
    name: String,
    description: String,
    version: String,
    payload: String,
    created_at: String,
}

impl TemplateRow {
    fn into_domain(self) -> CowResult<Template> {
        Ok(Template {
            id: self.id,
            kind: template_kind_from_str(&self.kind)?,
            name: self.name,
            description: self.description,
            version: self.version,
            payload: parse_json(&self.payload, "templates.payload")?,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

fn template_kind_to_str(kind: TemplateKind) -> &'static str {
    match kind {
        TemplateKind::Agent => "agent",
        TemplateKind::Team => "team",
    }
}

fn template_kind_from_str(value: &str) -> CowResult<TemplateKind> {
    match value {
        "agent" => Ok(TemplateKind::Agent),
        "team" => Ok(TemplateKind::Team),
        other => Err(CowError::Storage(format!(
            "unknown template kind '{other}'"
        ))),
    }
}

impl Store {
    pub async fn upsert_agent(
        &self,
        workspace_id: &WorkspaceId,
        profile: &AgentProfile,
    ) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO agent_profiles
                (id, workspace_id, name, role, goal, backstory, model, tools,
                 allow_delegation, max_iterations, verbose, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                workspace_id = excluded.workspace_id,
                name = excluded.name,
                role = excluded.role,
                goal = excluded.goal,
                backstory = excluded.backstory,
                model = excluded.model,
                tools = excluded.tools,
                allow_delegation = excluded.allow_delegation,
                max_iterations = excluded.max_iterations,
                verbose = excluded.verbose",
        )
        .bind(&profile.id.0)
        .bind(&workspace_id.0)
        .bind(&profile.name)
        .bind(&profile.role)
        .bind(&profile.goal)
        .bind(&profile.backstory)
        .bind(profile.model.clone().unwrap_or_default())
        .bind(serde_json::to_string(&profile.tools)?)
        .bind(if profile.allow_delegation { 1i64 } else { 0i64 })
        .bind(profile.max_iterations as i64)
        .bind(if profile.verbose { 1i64 } else { 0i64 })
        .bind(Utc::now().to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    /// Whether an agent belongs to the given workspace.
    pub async fn agent_in_workspace(
        &self,
        id: &AgentId,
        workspace_id: &WorkspaceId,
    ) -> CowResult<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_profiles WHERE id = ? AND workspace_id = ?",
        )
        .bind(&id.0)
        .bind(&workspace_id.0)
        .fetch_one(self.pool())
        .await
        .map_err(db)?;
        Ok(count > 0)
    }

    pub async fn get_agent(&self, id: &AgentId) -> CowResult<Option<AgentProfile>> {
        let row = sqlx::query_as::<_, AgentRow>(
            "SELECT id, name, role, goal, backstory, model, tools,
                    allow_delegation, max_iterations, verbose
             FROM agent_profiles WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(AgentRow::into_domain).transpose()
    }

    pub async fn list_agents(&self, workspace_id: &WorkspaceId) -> CowResult<Vec<AgentProfile>> {
        let rows = sqlx::query_as::<_, AgentRow>(
            "SELECT id, name, role, goal, backstory, model, tools,
                    allow_delegation, max_iterations, verbose
             FROM agent_profiles WHERE workspace_id = ? ORDER BY name",
        )
        .bind(&workspace_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(AgentRow::into_domain).collect()
    }

    pub async fn delete_agent(&self, id: &AgentId) -> CowResult<()> {
        sqlx::query("DELETE FROM agent_profiles WHERE id = ?")
            .bind(&id.0)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn upsert_template(&self, template: &Template) -> CowResult<()> {
        sqlx::query(
            "INSERT INTO templates (id, kind, name, description, version, payload, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                kind = excluded.kind,
                name = excluded.name,
                description = excluded.description,
                version = excluded.version,
                payload = excluded.payload",
        )
        .bind(&template.id)
        .bind(template_kind_to_str(template.kind))
        .bind(&template.name)
        .bind(&template.description)
        .bind(&template.version)
        .bind(serde_json::to_string(&template.payload)?)
        .bind(template.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn get_template(&self, id: &str) -> CowResult<Option<Template>> {
        let row = sqlx::query_as::<_, TemplateRow>(
            "SELECT id, kind, name, description, version, payload, created_at
             FROM templates WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;
        row.map(TemplateRow::into_domain).transpose()
    }

    pub async fn list_templates(&self, kind: Option<TemplateKind>) -> CowResult<Vec<Template>> {
        let rows = match kind {
            Some(kind) => sqlx::query_as::<_, TemplateRow>(
                "SELECT id, kind, name, description, version, payload, created_at
                     FROM templates WHERE kind = ? ORDER BY name",
            )
            .bind(template_kind_to_str(kind))
            .fetch_all(self.pool())
            .await
            .map_err(db)?,
            None => sqlx::query_as::<_, TemplateRow>(
                "SELECT id, kind, name, description, version, payload, created_at
                     FROM templates ORDER BY name",
            )
            .fetch_all(self.pool())
            .await
            .map_err(db)?,
        };
        rows.into_iter().map(TemplateRow::into_domain).collect()
    }

    pub async fn delete_template(&self, id: &str) -> CowResult<()> {
        sqlx::query("DELETE FROM templates WHERE id = ?")
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(db)?;
        Ok(())
    }
}
