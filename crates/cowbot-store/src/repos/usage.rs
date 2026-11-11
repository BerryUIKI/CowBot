use cowbot_core::agent::AgentId;
use cowbot_core::channel::ChannelId;
use cowbot_core::error::{CowError, CowResult};
use cowbot_core::run::RunId;
use cowbot_core::task::TaskId;
use cowbot_core::usage::{LlmCallKind, LlmCallRecord, TokenUsage, UsageTotals};

use crate::error::{db, parse_datetime, parse_json};
use crate::store::Store;

#[derive(sqlx::FromRow)]
struct LlmCallRow {
    id: String,
    run_id: Option<String>,
    task_id: Option<String>,
    channel_id: Option<String>,
    agent_id: Option<String>,
    call_kind: String,
    model: String,
    uncached_input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_write_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    raw_usage: Option<String>,
    created_at: String,
}

impl LlmCallRow {
    fn into_domain(self) -> CowResult<LlmCallRecord> {
        let call_kind = self.call_kind.parse::<LlmCallKind>().map_err(|_| {
            CowError::Storage(format!("unknown LLM call kind '{}'", self.call_kind))
        })?;

        let has_usage = self.uncached_input_tokens.is_some()
            || self.cached_input_tokens.is_some()
            || self.cache_write_input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.reasoning_tokens.is_some();
        let usage = has_usage.then(|| TokenUsage {
            uncached_input_tokens: token_count(self.uncached_input_tokens),
            cached_input_tokens: token_count(self.cached_input_tokens),
            cache_write_input_tokens: token_count(self.cache_write_input_tokens),
            output_tokens: token_count(self.output_tokens),
            reasoning_tokens: token_count(self.reasoning_tokens),
        });

        Ok(LlmCallRecord {
            id: self.id,
            run_id: self.run_id.map(RunId),
            task_id: self.task_id.map(TaskId),
            channel_id: self.channel_id.map(ChannelId::new),
            agent_id: self.agent_id.map(AgentId),
            call_kind,
            model: self.model,
            usage,
            raw_usage: self
                .raw_usage
                .as_deref()
                .map(|value| parse_json(value, "llm_calls.raw_usage"))
                .transpose()?,
            created_at: parse_datetime(&self.created_at)?,
        })
    }
}

fn token_count(value: Option<i64>) -> u32 {
    value
        .map(|tokens| tokens.clamp(0, u32::MAX as i64) as u32)
        .unwrap_or(0)
}

impl Store {
    pub async fn record_llm_call(&self, record: &LlmCallRecord) -> CowResult<()> {
        let usage = record.usage.as_ref();
        sqlx::query(
            "INSERT INTO llm_calls
                (id, run_id, task_id, channel_id, agent_id, call_kind, model,
                 uncached_input_tokens, cached_input_tokens, cache_write_input_tokens,
                 output_tokens, reasoning_tokens, raw_usage, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&record.id)
        .bind(record.run_id.as_ref().map(|id| &id.0))
        .bind(record.task_id.as_ref().map(|id| &id.0))
        .bind(record.channel_id.as_ref().map(|id| &id.0))
        .bind(record.agent_id.as_ref().map(|id| &id.0))
        .bind(record.call_kind.as_str())
        .bind(&record.model)
        .bind(usage.map(|usage| i64::from(usage.uncached_input_tokens)))
        .bind(usage.map(|usage| i64::from(usage.cached_input_tokens)))
        .bind(usage.map(|usage| i64::from(usage.cache_write_input_tokens)))
        .bind(usage.map(|usage| i64::from(usage.output_tokens)))
        .bind(usage.map(|usage| i64::from(usage.reasoning_tokens)))
        .bind(
            record
                .raw_usage
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(record.created_at.to_rfc3339())
        .execute(self.pool())
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn list_llm_calls_for_run(&self, run_id: &RunId) -> CowResult<Vec<LlmCallRecord>> {
        let rows = sqlx::query_as::<_, LlmCallRow>(
            "SELECT id, run_id, task_id, channel_id, agent_id, call_kind, model,
                    uncached_input_tokens, cached_input_tokens, cache_write_input_tokens,
                    output_tokens, reasoning_tokens, raw_usage, created_at
             FROM llm_calls WHERE run_id = ? ORDER BY created_at, id",
        )
        .bind(&run_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(LlmCallRow::into_domain).collect()
    }

    pub async fn list_llm_calls_for_task(&self, task_id: &TaskId) -> CowResult<Vec<LlmCallRecord>> {
        let rows = sqlx::query_as::<_, LlmCallRow>(
            "SELECT id, run_id, task_id, channel_id, agent_id, call_kind, model,
                    uncached_input_tokens, cached_input_tokens, cache_write_input_tokens,
                    output_tokens, reasoning_tokens, raw_usage, created_at
             FROM llm_calls WHERE task_id = ? ORDER BY created_at, id",
        )
        .bind(&task_id.0)
        .fetch_all(self.pool())
        .await
        .map_err(db)?;
        rows.into_iter().map(LlmCallRow::into_domain).collect()
    }

    pub async fn sum_llm_usage_for_task(&self, task_id: &TaskId) -> CowResult<UsageTotals> {
        let row = sqlx::query_as::<_, UsageTotalsRow>(
            "SELECT COUNT(*) AS calls,
                    COALESCE(SUM(uncached_input_tokens), 0) AS uncached_input_tokens,
                    COALESCE(SUM(cached_input_tokens), 0) AS cached_input_tokens,
                    COALESCE(SUM(cache_write_input_tokens), 0) AS cache_write_input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(reasoning_tokens), 0) AS reasoning_tokens
             FROM llm_calls WHERE task_id = ?",
        )
        .bind(&task_id.0)
        .fetch_one(self.pool())
        .await
        .map_err(db)?;
        Ok(row.into_domain())
    }

    /// Billed usage of every call attributed to a conversation channel.
    pub async fn sum_llm_usage_for_channel(
        &self,
        channel_id: &ChannelId,
    ) -> CowResult<UsageTotals> {
        let row = sqlx::query_as::<_, UsageTotalsRow>(
            "SELECT COUNT(*) AS calls,
                    COALESCE(SUM(uncached_input_tokens), 0) AS uncached_input_tokens,
                    COALESCE(SUM(cached_input_tokens), 0) AS cached_input_tokens,
                    COALESCE(SUM(cache_write_input_tokens), 0) AS cache_write_input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(reasoning_tokens), 0) AS reasoning_tokens
             FROM llm_calls WHERE channel_id = ?",
        )
        .bind(&channel_id.0)
        .fetch_one(self.pool())
        .await
        .map_err(db)?;
        Ok(row.into_domain())
    }

    /// Billed usage of the most recent run in a conversation channel.
    pub async fn latest_run_usage_for_channel(
        &self,
        channel_id: &ChannelId,
    ) -> CowResult<UsageTotals> {
        let run_id: Option<String> = sqlx::query_scalar(
            "SELECT run_id FROM llm_calls
             WHERE channel_id = ? AND run_id IS NOT NULL
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(&channel_id.0)
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;

        let Some(run_id) = run_id else {
            return Ok(UsageTotals::default());
        };

        let row = sqlx::query_as::<_, UsageTotalsRow>(
            "SELECT COUNT(*) AS calls,
                    COALESCE(SUM(uncached_input_tokens), 0) AS uncached_input_tokens,
                    COALESCE(SUM(cached_input_tokens), 0) AS cached_input_tokens,
                    COALESCE(SUM(cache_write_input_tokens), 0) AS cache_write_input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(reasoning_tokens), 0) AS reasoning_tokens
             FROM llm_calls WHERE channel_id = ? AND run_id = ?",
        )
        .bind(&channel_id.0)
        .bind(&run_id)
        .fetch_one(self.pool())
        .await
        .map_err(db)?;
        Ok(row.into_domain())
    }

    /// Billed usage of the latest run one agent owned in a conversation channel.
    ///
    /// Router calls are skipped when picking the run so a per-participant meter
    /// never attributes a routing decision to the participant.
    pub async fn latest_run_usage_for_channel_agent(
        &self,
        channel_id: &ChannelId,
        agent_id: &AgentId,
    ) -> CowResult<UsageTotals> {
        let run_id: Option<String> = sqlx::query_scalar(
            "SELECT run_id FROM llm_calls
             WHERE channel_id = ? AND agent_id = ? AND run_id IS NOT NULL
               AND call_kind <> ?
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(&channel_id.0)
        .bind(&agent_id.0)
        .bind(LlmCallKind::Router.as_str())
        .fetch_optional(self.pool())
        .await
        .map_err(db)?;

        let Some(run_id) = run_id else {
            return Ok(UsageTotals::default());
        };

        let row = sqlx::query_as::<_, UsageTotalsRow>(
            "SELECT COUNT(*) AS calls,
                    COALESCE(SUM(uncached_input_tokens), 0) AS uncached_input_tokens,
                    COALESCE(SUM(cached_input_tokens), 0) AS cached_input_tokens,
                    COALESCE(SUM(cache_write_input_tokens), 0) AS cache_write_input_tokens,
                    COALESCE(SUM(output_tokens), 0) AS output_tokens,
                    COALESCE(SUM(reasoning_tokens), 0) AS reasoning_tokens
             FROM llm_calls WHERE channel_id = ? AND agent_id = ? AND run_id = ?",
        )
        .bind(&channel_id.0)
        .bind(&agent_id.0)
        .bind(&run_id)
        .fetch_one(self.pool())
        .await
        .map_err(db)?;
        Ok(row.into_domain())
    }
}

#[derive(sqlx::FromRow)]
struct UsageTotalsRow {
    calls: i64,
    uncached_input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
}

impl UsageTotalsRow {
    fn into_domain(self) -> UsageTotals {
        let count = |value: i64| value.max(0) as u64;
        UsageTotals {
            calls: count(self.calls),
            uncached_input_tokens: count(self.uncached_input_tokens),
            cached_input_tokens: count(self.cached_input_tokens),
            cache_write_input_tokens: count(self.cache_write_input_tokens),
            output_tokens: count(self.output_tokens),
            reasoning_tokens: count(self.reasoning_tokens),
        }
    }
}
