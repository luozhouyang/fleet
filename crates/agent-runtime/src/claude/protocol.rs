//! Claude Code CLI `stream-json` protocol types.
//!
//! Grounded against Claude Code 2.1.319 (`claude --help`). The CLI writes one
//! JSON object per line to stdout; input uses the same stream-json framing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One line of Claude Code stream-json output.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ClaudeLine {
    /// Session lifecycle metadata, including `subtype: "init"`.
    #[serde(rename = "system")]
    System {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        tools: Option<Vec<String>>,
    },
    /// A complete assistant message.
    #[serde(rename = "assistant")]
    Assistant {
        message: AssistantMessage,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        parent_tool_use_id: Option<String>,
    },
    /// User-side content, usually tool results.
    #[serde(rename = "user")]
    User {
        message: UserMessage,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        parent_tool_use_id: Option<String>,
    },
    /// Final result for one turn.
    #[serde(rename = "result")]
    ResultLine {
        #[serde(default)]
        subtype: Option<String>,
        #[serde(default)]
        is_error: Option<bool>,
        #[serde(default)]
        result: Option<String>,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        num_turns: Option<u32>,
        #[serde(default)]
        total_cost_usd: Option<f64>,
    },
    /// Incremental streaming events (`--include-partial-messages`).
    #[serde(rename = "stream_event")]
    StreamEvent {
        event: StreamEvent,
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Any future line type.
    #[serde(other)]
    Unknown,
}

impl ClaudeLine {
    /// Native session id carried by the line, when present.
    pub(crate) fn session_id(&self) -> Option<&str> {
        match self {
            Self::System { session_id, .. }
            | Self::Assistant { session_id, .. }
            | Self::User { session_id, .. }
            | Self::ResultLine { session_id, .. }
            | Self::StreamEvent { session_id, .. } => session_id.as_deref(),
            Self::Unknown => None,
        }
    }
}

/// Assistant message envelope.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AssistantMessage {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub content: MessageContent,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

/// User message envelope.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UserMessage {
    pub content: MessageContent,
}

/// Claude content may be a plain string or a list of typed blocks.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum MessageContent {
    /// Plain text content.
    Text(String),
    /// Structured content blocks.
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Iterate blocks, promoting plain text to a single text block.
    pub(crate) fn blocks(&self) -> Vec<ContentBlock> {
        match self {
            Self::Text(text) => vec![ContentBlock::Text { text: text.clone() }],
            Self::Blocks(blocks) => blocks.clone(),
        }
    }
}

/// A typed content block.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ContentBlock {
    /// Text output.
    #[serde(rename = "text")]
    Text { text: String },
    /// Assistant tool invocation.
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    /// Tool result returned by the user side.
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Option<Value>,
        #[serde(default)]
        is_error: Option<bool>,
    },
    /// Extended thinking block (kept out of unified message text).
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default)]
        thinking: Option<String>,
    },
    /// Any future block type.
    #[serde(other)]
    Unknown,
}

/// Incremental stream event types.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StreamEvent {
    /// A new content block started.
    ContentBlockStart {
        #[serde(default)]
        index: Option<u32>,
        #[serde(default)]
        content_block: Option<Value>,
    },
    /// A delta was appended to the current content block.
    ContentBlockDelta {
        #[serde(default)]
        index: Option<u32>,
        delta: Delta,
    },
    /// The current content block ended.
    ContentBlockStop {
        #[serde(default)]
        index: Option<u32>,
    },
    /// Message-level metadata.
    MessageStart {
        #[serde(default)]
        message: Option<Value>,
    },
    /// Message-level delta (stop reason and usage).
    MessageDelta {
        #[serde(default)]
        delta: Option<Value>,
        #[serde(default)]
        usage: Option<Value>,
    },
    /// The message ended.
    MessageStop,
    /// Any future event type.
    #[serde(other)]
    Unknown,
}

/// A content block delta.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type")]
pub(crate) enum Delta {
    /// Assistant text.
    #[serde(rename = "text_delta")]
    Text { text: String },
    /// Partial tool input JSON.
    #[serde(rename = "input_json_delta")]
    InputJson {
        #[serde(default)]
        partial_json: String,
    },
    /// Any future delta type.
    #[serde(other)]
    Unknown,
}

/// Render tool result content as a JSON value, preserving strings.
pub(crate) fn tool_result_value(content: Option<&Value>) -> Value {
    match content {
        Some(Value::String(text)) => Value::String(text.clone()),
        Some(Value::Array(blocks)) => {
            let texts: Vec<String> = blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            if texts.is_empty() {
                Value::Array(blocks.clone())
            } else {
                Value::String(texts.join(""))
            }
        }
        Some(value) => value.clone(),
        None => Value::Null,
    }
}
