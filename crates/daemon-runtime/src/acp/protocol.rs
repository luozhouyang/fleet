//! Re-exports of the ACP v1 schema types used by the provider and mapper.

pub use agent_client_protocol::schema::ProtocolVersion;
pub use agent_client_protocol::v1::schema::{
    CancelNotification, ContentBlock, ContentChunk, CreateElicitationRequest,
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, ElicitationContentValue,
    ElicitationMode, ElicitationPropertySchema, ElicitationScope, ElicitationSessionScope,
    MultiSelectItems, PermissionOption, PermissionOptionKind, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, StopReason, TextContent,
    ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
