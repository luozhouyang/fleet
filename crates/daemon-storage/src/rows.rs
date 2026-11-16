//! Row mappings for daemon storage.

use fleet_core_common::ApiError;
use fleet_core_protocol::agent::{
    AgentMessage, AgentPermissionRequest, AgentQuestion, AgentQuestionRequest, AgentSession,
    AgentToolCall, AgentTurn, MessageRole, QuestionStatus, ToolCallStatus, TurnStatus,
};
use fleet_core_protocol::git::GitFileStatus;
use fleet_core_protocol::workspace::{EntryType, Workspace, WorkspaceEntry};
use sqlx::FromRow;

use crate::helpers::*;

#[derive(Debug, FromRow)]
pub(crate) struct WorkspaceRow {
    id: String,
    machine_id: String,
    project_id: Option<String>,
    name: String,
    root_path: String,
    is_git_repository: i64,
    created_at: String,
    updated_at: String,
}

impl WorkspaceRow {
    pub(crate) fn into_workspace(self) -> Workspace {
        Workspace {
            id: self.id,
            machine_id: self.machine_id,
            project_id: self.project_id,
            name: self.name,
            root_path: self.root_path,
            is_git_repository: self.is_git_repository != 0,
            created_at: parse_ts(&self.created_at),
            updated_at: parse_ts(&self.updated_at),
        }
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct SessionRow {
    id: String,
    machine_id: String,
    workspace_id: Option<String>,
    provider: String,
    agent_type: String,
    status: String,
    cwd: String,
    title: Option<String>,
    last_error_json: Option<String>,
    metadata_json: String,
    created_at: String,
    updated_at: String,
}

impl SessionRow {
    pub(crate) fn into_session(self) -> AgentSession {
        AgentSession {
            id: self.id,
            machine_id: self.machine_id,
            workspace_id: self.workspace_id,
            provider: self.provider,
            agent_type: self.agent_type,
            status: parse_session_status(&self.status),
            cwd: self.cwd,
            title: self.title,
            created_at: parse_ts(&self.created_at),
            updated_at: parse_ts(&self.updated_at),
            last_error: decode_opt_json::<ApiError>(self.last_error_json),
            metadata: decode_json_value(&self.metadata_json),
        }
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct PermissionRow {
    id: String,
    session_id: String,
    turn_id: Option<String>,
    tool_call_id: Option<String>,
    tool_name: String,
    description: String,
    input_json: String,
    options_json: String,
    status: String,
    created_at: String,
    resolved_at: Option<String>,
    expires_at: Option<String>,
}

#[derive(Debug, FromRow)]
pub(crate) struct QuestionRow {
    id: String,
    session_id: String,
    turn_id: Option<String>,
    tool_call_id: Option<String>,
    questions_json: String,
    status: String,
    created_at: String,
    resolved_at: Option<String>,
    expires_at: Option<String>,
}

impl QuestionRow {
    pub(crate) fn into_question(self) -> AgentQuestionRequest {
        let questions: Vec<AgentQuestion> =
            serde_json::from_str(&self.questions_json).unwrap_or_default();
        AgentQuestionRequest {
            id: self.id,
            session_id: self.session_id,
            turn_id: self.turn_id,
            tool_call_id: self.tool_call_id,
            questions,
            status: parse_question_status(&self.status),
            created_at: parse_ts(&self.created_at),
            resolved_at: parse_opt_ts(self.resolved_at),
            expires_at: parse_opt_ts(self.expires_at),
        }
    }
}

fn parse_question_status(text: &str) -> QuestionStatus {
    match text {
        "answered" => QuestionStatus::Answered,
        "rejected" => QuestionStatus::Rejected,
        "timed_out" => QuestionStatus::TimedOut,
        "cancelled" => QuestionStatus::Cancelled,
        "delivery_failed" => QuestionStatus::DeliveryFailed,
        _ => QuestionStatus::Pending,
    }
}

impl PermissionRow {
    pub(crate) fn into_permission(self) -> AgentPermissionRequest {
        AgentPermissionRequest {
            id: self.id,
            session_id: self.session_id,
            turn_id: self.turn_id,
            tool_call_id: self.tool_call_id,
            tool_name: self.tool_name,
            description: self.description,
            input: decode_json_value(&self.input_json),
            options: serde_json::from_str(&self.options_json).unwrap_or_default(),
            status: parse_permission_status(&self.status),
            created_at: parse_ts(&self.created_at),
            resolved_at: parse_opt_ts(self.resolved_at),
            expires_at: parse_opt_ts(self.expires_at),
        }
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct TurnRow {
    id: String,
    session_id: String,
    status: String,
    error_json: Option<String>,
    started_at: String,
    completed_at: Option<String>,
}

impl TurnRow {
    pub(crate) fn into_turn(self) -> AgentTurn {
        AgentTurn {
            id: self.id,
            session_id: self.session_id,
            status: enum_from_text(&self.status).unwrap_or(TurnStatus::Failed),
            started_at: parse_ts(&self.started_at),
            completed_at: parse_opt_ts(self.completed_at),
            error: decode_opt_json::<ApiError>(self.error_json),
        }
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct MessageRow {
    id: String,
    session_id: String,
    turn_id: Option<String>,
    role: String,
    content: String,
    created_at: String,
}

impl MessageRow {
    pub(crate) fn into_message(self) -> AgentMessage {
        AgentMessage {
            id: self.id,
            session_id: self.session_id,
            turn_id: self.turn_id,
            role: enum_from_text::<MessageRole>(&self.role).unwrap_or(MessageRole::Assistant),
            content: self.content,
            created_at: parse_ts(&self.created_at),
        }
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct ToolCallRow {
    id: String,
    session_id: String,
    turn_id: Option<String>,
    name: String,
    status: String,
    input_json: String,
    output_json: Option<String>,
    error: Option<String>,
    started_at: String,
    completed_at: Option<String>,
}

impl ToolCallRow {
    pub(crate) fn into_tool_call(self) -> AgentToolCall {
        AgentToolCall {
            id: self.id,
            session_id: self.session_id,
            turn_id: self.turn_id,
            name: self.name,
            status: enum_from_text(&self.status).unwrap_or(ToolCallStatus::Failed),
            input: decode_json_value(&self.input_json),
            output: self
                .output_json
                .as_deref()
                .map(decode_json_value)
                .unwrap_or(serde_json::Value::Null),
            error: self.error,
            started_at: parse_ts(&self.started_at),
            completed_at: parse_opt_ts(self.completed_at),
        }
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct EntryRow {
    path: String,
    name: String,
    entry_type: String,
    size: Option<i64>,
    mtime_ns: Option<i64>,
    git_status: Option<String>,
    ignored: i64,
}

impl EntryRow {
    pub(crate) fn into_entry(self) -> WorkspaceEntry {
        WorkspaceEntry {
            path: self.path,
            name: self.name,
            entry_type: enum_from_text(&self.entry_type).unwrap_or(EntryType::File),
            size: self.size.and_then(|size| u64::try_from(size).ok()),
            modified_at: self.mtime_ns.map(nanos_to_ts),
            git_status: self
                .git_status
                .and_then(|status| enum_from_text::<GitFileStatus>(&status)),
            ignored: self.ignored != 0,
        }
    }
}
