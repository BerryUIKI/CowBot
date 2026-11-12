use crate::agent::AgentId;
use crate::error::CowError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
/// Definition and schema of a callable tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// An invocation request for a tool emitted by an agent.
#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// The result returned from executing a tool.
#[cfg_attr(feature = "bindings", derive(ts_rs::TS), ts(export))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub output: String,
    pub success: bool,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn success(call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            output: output.into(),
            success: true,
            error: None,
        }
    }

    pub fn failure(call_id: impl Into<String>, error: impl Into<String>) -> Self {
        let err = error.into();
        Self {
            call_id: call_id.into(),
            output: String::new(),
            success: false,
            error: Some(err),
        }
    }
}

/// Trait implemented by any executable capability (code runner, filesystem, web, etc.).
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }

    async fn execute(&self, call: &ToolCall) -> Result<ToolResult, CowError>;
}

/// Result of checking a tool call against the approval policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolAuthorization {
    Approved { approved_by: Option<String> },
    Denied { reason: String },
}

/// Gate consulted before a tool call executes.
#[async_trait]
pub trait ToolGate: Send + Sync {
    async fn authorize(
        &self,
        agent_id: &AgentId,
        call: &ToolCall,
    ) -> Result<ToolAuthorization, CowError>;
}

/// Persists tool calls executed during an agent run.
#[async_trait]
pub trait ToolCallRecorder: Send + Sync {
    async fn record_tool_call(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        approved_by: Option<&str>,
    ) -> Result<(), CowError>;
}
