CREATE TABLE llm_calls (
    id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT,
    task_id TEXT,
    channel_id TEXT,
    agent_id TEXT,
    call_kind TEXT NOT NULL,
    model TEXT NOT NULL,
    uncached_input_tokens INTEGER,
    cached_input_tokens INTEGER,
    cache_write_input_tokens INTEGER,
    output_tokens INTEGER,
    reasoning_tokens INTEGER,
    raw_usage TEXT,
    created_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE CASCADE
);

CREATE INDEX idx_llm_calls_run ON llm_calls(run_id);
CREATE INDEX idx_llm_calls_task ON llm_calls(task_id);
CREATE INDEX idx_llm_calls_channel ON llm_calls(channel_id, created_at);
CREATE INDEX idx_llm_calls_agent ON llm_calls(agent_id, created_at);
