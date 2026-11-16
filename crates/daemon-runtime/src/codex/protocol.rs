//! Codex app-server JSON-RPC protocol types.
//!
//! Grounded against `codex-cli 0.116.0`
//! (`codex app-server generate-json-schema --out /tmp/codex-schema`).
//! Messages are one JSON object per line over stdio.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A client request written to the app server.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RpcRequest {
    pub id: u64,
    pub method: String,
    pub params: Value,
}

impl RpcRequest {
    /// Serialize the request as a single JSONL line.
    pub(crate) fn to_line(&self) -> serde_json::Result<String> {
        let mut line = serde_json::to_string(self)?;
        line.push('\n');
        Ok(line)
    }
}

/// A successful or failed response to a client request.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RpcResponse {
    pub id: Value,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// A server notification (no response expected).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RpcNotification {
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// A server-initiated request (for example an approval prompt).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RpcServerRequest {
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// Any decoded incoming line.
#[derive(Debug, Clone)]
pub(crate) enum Incoming {
    Response(RpcResponse),
    Notification(RpcNotification),
    ServerRequest(RpcServerRequest),
    Unknown(Value),
}

/// Classify a decoded JSON line.
pub(crate) fn parse_incoming(value: &Value) -> Incoming {
    let has_id = value.get("id").is_some();
    let has_method = value.get("method").is_some();
    let has_result = value.get("result").is_some();
    let has_error = value.get("error").is_some();

    if has_id && has_method && !has_result && !has_error {
        match serde_json::from_value::<RpcServerRequest>(value.clone()) {
            Ok(request) => return Incoming::ServerRequest(request),
            Err(_) => return Incoming::Unknown(value.clone()),
        }
    }
    if has_id && (has_result || has_error) {
        if let Ok(response) = serde_json::from_value::<RpcResponse>(value.clone()) {
            return Incoming::Response(response);
        }
    }
    if has_method && !has_id {
        if let Ok(notification) = serde_json::from_value::<RpcNotification>(value.clone()) {
            return Incoming::Notification(notification);
        }
    }
    Incoming::Unknown(value.clone())
}

/// Normalize a response id to the string key used by the pending-request map.
pub(crate) fn id_key(id: &Value) -> Option<String> {
    match id {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// `initialize` result.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct InitializeResponse {
    #[serde(rename = "userAgent", default)]
    pub user_agent: Option<String>,
    #[serde(rename = "platformOs", default)]
    pub platform_os: Option<String>,
}

/// `thread/start` and `thread/resume` result (only the fields we use).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ThreadResponse {
    pub thread: ThreadRef,
}

/// Minimal thread reference.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ThreadRef {
    pub id: String,
    #[serde(default)]
    pub preview: Option<String>,
}

/// `turn/start` result.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TurnResponse {
    pub turn: Turn,
}

/// A turn lifecycle object.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Turn {
    pub id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub error: Option<TurnError>,
}

/// Turn failure details.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TurnError {
    pub message: String,
}

/// `thread/started` notification.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ThreadStarted {
    pub thread: ThreadRef,
}

/// `turn/started` and `turn/completed` notification.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TurnNotification {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    pub turn: Turn,
}

/// `item/tool/requestUserInput` server request params.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolRequestUserInputParams {
    pub item_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub questions: Vec<ToolRequestUserInputQuestion>,
}

/// One question inside `item/tool/requestUserInput`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolRequestUserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub options: Option<Vec<ToolRequestUserInputOption>>,
    #[serde(default)]
    pub is_other: bool,
    #[serde(default)]
    pub is_secret: bool,
}

/// One selectable option for `item/tool/requestUserInput`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ToolRequestUserInputOption {
    pub label: String,
    pub description: String,
}

/// `item/started` and `item/completed` notification.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ItemNotification {
    pub item: ThreadItem,
    #[serde(rename = "threadId", default)]
    pub thread_id: Option<String>,
    #[serde(rename = "turnId", default)]
    pub turn_id: Option<String>,
}

/// `item/agentMessage/delta` notification.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AgentMessageDelta {
    pub delta: String,
    #[serde(rename = "itemId")]
    pub item_id: String,
    #[serde(rename = "threadId", default)]
    pub thread_id: Option<String>,
    #[serde(rename = "turnId", default)]
    pub turn_id: Option<String>,
}

/// `thread/status/changed` notification.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ThreadStatusChanged {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    pub status: ThreadStatus,
}

/// Thread runtime status.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ThreadStatus {
    #[serde(rename = "type")]
    pub kind: String,
}

/// Thread items used by the unified mapper.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ThreadItem {
    /// Assistant message item.
    #[serde(rename = "agentMessage")]
    AgentMessage {
        id: String,
        #[serde(default)]
        text: String,
    },
    /// User message item.
    #[serde(rename = "userMessage")]
    UserMessage { id: String },
    /// Shell command execution.
    #[serde(rename = "commandExecution")]
    CommandExecution {
        id: String,
        #[serde(default)]
        command: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        status: String,
        #[serde(rename = "aggregatedOutput", default)]
        aggregated_output: Option<String>,
        #[serde(rename = "exitCode", default)]
        exit_code: Option<i32>,
    },
    /// File change / patch application.
    #[serde(rename = "fileChange")]
    FileChange {
        id: String,
        #[serde(default)]
        status: String,
        #[serde(default)]
        changes: Value,
    },
    /// MCP tool invocation.
    #[serde(rename = "mcpToolCall")]
    McpToolCall {
        id: String,
        #[serde(default)]
        server: String,
        #[serde(default)]
        tool: String,
        #[serde(default)]
        arguments: Value,
        #[serde(default)]
        status: String,
        #[serde(default)]
        error: Option<McpToolCallError>,
        #[serde(default)]
        result: Option<Value>,
    },
    /// Any other item type (reasoning, plan, collaboration, ...).
    #[serde(other)]
    Other,
}

/// MCP tool failure.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct McpToolCallError {
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_messages() {
        let response = json!({"id": 7, "result": {"ok": true}});
        assert!(matches!(parse_incoming(&response), Incoming::Response(_)));

        let notification = json!({"method": "turn/started", "params": {}});
        assert!(matches!(
            parse_incoming(&notification),
            Incoming::Notification(_)
        ));

        let request =
            json!({"id": 9, "method": "item/commandExecution/requestApproval", "params": {}});
        assert!(matches!(
            parse_incoming(&request),
            Incoming::ServerRequest(_)
        ));

        assert!(matches!(
            parse_incoming(&json!({"hello": 1})),
            Incoming::Unknown(_)
        ));
    }

    #[test]
    fn normalizes_ids() {
        assert_eq!(id_key(&json!(12)), Some("12".to_string()));
        assert_eq!(id_key(&json!("abc")), Some("abc".to_string()));
        assert_eq!(id_key(&json!(null)), None);
    }

    #[test]
    fn request_serializes_as_one_line() {
        let request = RpcRequest {
            id: 1,
            method: "initialize".to_string(),
            params: json!({"clientInfo": {"name": "agent-fleet", "version": "0.1.0"}}),
        };
        let line = request.to_line().expect("serialize");
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        let value: Value = serde_json::from_str(line.trim()).expect("parse");
        assert_eq!(value["id"], 1);
        assert_eq!(value["method"], "initialize");
    }
}
