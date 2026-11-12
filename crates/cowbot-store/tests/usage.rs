use chrono::{Duration, Utc};
use cowbot_core::agent::AgentId;
use cowbot_core::channel::Channel;
use cowbot_core::identity::{User, Workspace};
use cowbot_core::run::Run;
use cowbot_core::task::Task;
use cowbot_core::usage::{LlmCallKind, LlmCallRecord, TokenUsage};
use cowbot_store::Store;
use serde_json::json;

async fn test_store() -> Store {
    let store = Store::connect_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    store
}

async fn seed_workspace(store: &Store) -> Workspace {
    let user = User::new("Alice", "alice@example.com", "argon2-hash");
    let workspace = Workspace::new("Usage Lab");
    store.create_user(&user).await.unwrap();
    store.create_workspace(&workspace).await.unwrap();
    workspace
}

fn record(
    run_id: Option<&Run>,
    task_id: Option<&Task>,
    agent_id: &AgentId,
    usage: Option<TokenUsage>,
    created_at: chrono::DateTime<Utc>,
) -> LlmCallRecord {
    let mut record = LlmCallRecord::new(LlmCallKind::Agent, "deepseek-chat");
    record.run_id = run_id.map(|run| run.id.clone());
    record.task_id = task_id.map(|task| task.id.clone());
    record.agent_id = Some(agent_id.clone());
    record.raw_usage = usage
        .as_ref()
        .map(|_| json!({"prompt_cache_hit_tokens": 80}));
    record.usage = usage;
    record.created_at = created_at;
    record
}

#[tokio::test]
async fn usage_rows_round_trip_and_sum() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let agent_id = AgentId::new("agent-coder");
    let task = Task::new("Write a summary", "Summary").assign_to(agent_id.clone());
    store.create_task(&workspace.id, None, &task).await.unwrap();
    let run = Run::new(task.id.clone(), agent_id.clone());
    store.create_run(&run).await.unwrap();

    let usage = TokenUsage {
        uncached_input_tokens: 400,
        cached_input_tokens: 600,
        cache_write_input_tokens: 50,
        output_tokens: 120,
        reasoning_tokens: 10,
    };
    let first = record(
        Some(&run),
        Some(&task),
        &agent_id,
        Some(usage.clone()),
        Utc::now() - Duration::seconds(1),
    );
    let second = record(Some(&run), Some(&task), &agent_id, None, Utc::now());
    store.record_llm_call(&first).await.unwrap();
    store.record_llm_call(&second).await.unwrap();

    let calls = store.list_llm_calls_for_run(&run.id).await.unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].id, first.id);
    assert_eq!(calls[0].usage, Some(usage));
    assert_eq!(
        calls[0].raw_usage,
        Some(json!({"prompt_cache_hit_tokens": 80}))
    );
    assert_eq!(calls[1].usage, None);
    assert_eq!(calls[1].raw_usage, None);

    let by_task = store.list_llm_calls_for_task(&task.id).await.unwrap();
    assert_eq!(by_task.len(), 2);

    let channel = Channel::new("usage", "Usage");
    store.create_channel(&workspace.id, &channel).await.unwrap();
    let mut channel_record = record(None, None, &agent_id, None, Utc::now());
    channel_record.channel_id = Some(channel.id.clone());
    store.record_llm_call(&channel_record).await.unwrap();
    let by_channel = store.list_llm_calls_for_channel(&channel.id).await.unwrap();
    assert_eq!(by_channel.len(), 1);
    assert_eq!(by_channel[0].id, channel_record.id);
    assert_eq!(by_channel[0].channel_id.as_ref(), Some(&channel.id));

    let totals = store.sum_llm_usage_for_task(&task.id).await.unwrap();
    assert_eq!(totals.calls, 2);
    assert_eq!(totals.uncached_input_tokens, 400);
    assert_eq!(totals.cached_input_tokens, 600);
    assert_eq!(totals.cache_write_input_tokens, 50);
    assert_eq!(totals.output_tokens, 120);
    assert_eq!(totals.reasoning_tokens, 10);
    assert_eq!(totals.total_input_tokens(), 1050);
    assert_eq!(totals.total_tokens(), 1170);
}

#[tokio::test]
async fn deleting_a_run_cascades_its_usage_rows() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let agent_id = AgentId::new("agent-coder");
    let task = Task::new("Write a summary", "Summary").assign_to(agent_id.clone());
    store.create_task(&workspace.id, None, &task).await.unwrap();
    let run = Run::new(task.id.clone(), agent_id.clone());
    store.create_run(&run).await.unwrap();

    let attached = record(
        Some(&run),
        Some(&task),
        &agent_id,
        Some(TokenUsage::default()),
        Utc::now(),
    );
    let standalone = record(None, None, &agent_id, None, Utc::now());
    store.record_llm_call(&attached).await.unwrap();
    store.record_llm_call(&standalone).await.unwrap();

    sqlx::query("DELETE FROM runs WHERE id = ?")
        .bind(&run.id.0)
        .execute(store.pool())
        .await
        .unwrap();

    assert!(store
        .list_llm_calls_for_run(&run.id)
        .await
        .unwrap()
        .is_empty());

    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM llm_calls")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(remaining, 1);
}

#[tokio::test]
async fn unknown_call_kinds_are_reported_as_storage_errors() {
    let store = test_store().await;
    let workspace = seed_workspace(&store).await;
    let agent_id = AgentId::new("agent-coder");
    let task = Task::new("Write a summary", "Summary").assign_to(agent_id.clone());
    store.create_task(&workspace.id, None, &task).await.unwrap();
    let run = Run::new(task.id.clone(), agent_id.clone());
    store.create_run(&run).await.unwrap();

    let record = record(Some(&run), Some(&task), &agent_id, None, Utc::now());
    store.record_llm_call(&record).await.unwrap();

    sqlx::query("UPDATE llm_calls SET call_kind = 'mystery' WHERE id = ?")
        .bind(&record.id)
        .execute(store.pool())
        .await
        .unwrap();

    let error = store.list_llm_calls_for_task(&task.id).await.unwrap_err();
    assert!(error.to_string().contains("unknown LLM call kind"));
}
