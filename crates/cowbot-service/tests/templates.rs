use cowbot_core::template::{
    AgentTemplate, TaskTemplate, TemplateDocument, TemplateKind, TEMPLATE_SCHEMA_VERSION,
};
use cowbot_service::{bootstrap_local, builtin_templates, TemplateService};
use cowbot_store::Store;

async fn test_store() -> Store {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    store
}

fn sample_team(id: &str) -> TemplateDocument {
    TemplateDocument {
        schema_version: TEMPLATE_SCHEMA_VERSION,
        kind: TemplateKind::Team,
        id: id.to_string(),
        name: "Sample Team".to_string(),
        description: "A sample team for tests.".to_string(),
        version: "1.0.0".to_string(),
        tags: vec!["sample".to_string()],
        agents: vec![AgentTemplate {
            name: "Planner".to_string(),
            role: "Architect".to_string(),
            goal: "Plan the work".to_string(),
            backstory: String::new(),
            model: None,
            tools: Vec::new(),
            allow_delegation: true,
            max_iterations: 15,
        }],
        tasks: vec![TaskTemplate {
            id: "plan".to_string(),
            description: "Plan the work".to_string(),
            expected_output: "A plan".to_string(),
            agent: "Planner".to_string(),
            depends_on: Vec::new(),
        }],
    }
}

#[tokio::test]
async fn builtin_templates_are_valid() {
    let templates = builtin_templates().unwrap();
    assert_eq!(templates.len(), 3);
    for template in &templates {
        template.validate().unwrap();
    }

    let software = templates
        .iter()
        .find(|template| template.id == "builtin-software-team")
        .unwrap();
    assert_eq!(software.kind, TemplateKind::Team);
    assert_eq!(software.agents.len(), 3);
    assert_eq!(software.tasks.len(), 3);

    let general = templates
        .iter()
        .find(|template| template.id == "builtin-general-assistant")
        .unwrap();
    assert_eq!(general.kind, TemplateKind::Agent);
    assert_eq!(general.agents.len(), 1);
    assert!(general.tasks.is_empty());
}

#[tokio::test]
async fn template_crud_import_export_roundtrip() {
    let store = test_store().await;
    let context = bootstrap_local(&store).await.unwrap();
    let service = TemplateService::new(store.clone(), context.workspace.id.clone());
    let document = sample_team("user-sample-team");

    service.save(&document).await.unwrap();
    let loaded = service.get("user-sample-team").await.unwrap().unwrap();
    assert_eq!(loaded.name, "Sample Team");
    assert_eq!(loaded.agents.len(), 1);

    let listed = service.list().await.unwrap();
    assert!(listed.iter().any(|template| template.id == document.id));
    assert!(listed
        .iter()
        .any(|template| template.id == "builtin-software-team"));

    let exported = TemplateService::export_json(&document).unwrap();
    let imported = service.import_json(&exported).await.unwrap();
    assert_eq!(imported.id, document.id);
    assert_eq!(imported.tasks.len(), 1);

    service.delete("user-sample-team").await.unwrap();
    assert!(service.get("user-sample-team").await.unwrap().is_none());

    let builtin = builtin_templates().unwrap().remove(0);
    assert!(service.save(&builtin).await.is_err());
    assert!(service.delete(&builtin.id).await.is_err());
}

#[tokio::test]
async fn instantiate_agents_from_template() {
    let store = test_store().await;
    let context = bootstrap_local(&store).await.unwrap();
    let service = TemplateService::new(store.clone(), context.workspace.id.clone());

    let profiles = service
        .instantiate_agents(&context.workspace.id, "builtin-software-team")
        .await
        .unwrap();
    assert_eq!(profiles.len(), 3);

    let agents = store.list_agents(&context.workspace.id).await.unwrap();
    assert_eq!(agents.len(), 3);
    let coder = agents.iter().find(|agent| agent.name == "Coder").unwrap();
    assert!(coder.tools.contains(&"execute_code".to_string()));
    assert_eq!(coder.max_iterations, 20);
}

#[tokio::test]
async fn invalid_templates_are_rejected() {
    let mut duplicate_agents = sample_team("duplicate-agents");
    duplicate_agents
        .agents
        .push(duplicate_agents.agents[0].clone());
    assert!(duplicate_agents.validate().is_err());

    let mut wrong_schema = sample_team("wrong-schema");
    wrong_schema.schema_version = 99;
    assert!(wrong_schema.validate().is_err());

    let mut unknown_agent = sample_team("unknown-agent");
    unknown_agent.tasks[0].agent = "Ghost".to_string();
    assert!(unknown_agent.validate().is_err());

    let mut cyclic = sample_team("cyclic");
    cyclic.tasks.push(TaskTemplate {
        id: "second".to_string(),
        description: "Second task".to_string(),
        expected_output: "Second output".to_string(),
        agent: "Planner".to_string(),
        depends_on: vec!["plan".to_string()],
    });
    cyclic.tasks[0].depends_on = vec!["second".to_string()];
    assert!(cyclic.validate().is_err());
}
