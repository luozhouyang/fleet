//! Per-session mapper state for ACP v2 upserts.
//!
//! The state is deliberately owned by one session and protected by a mutex.
//! It maps native ids to Fleet ids and applies v2 patch semantics so partial
//! updates never erase previously received content.

use std::collections::HashMap;

use fleet_core_protocol::agent::{AgentPlan, MessageRole, PlanEntry, ToolCallStatus};

/// One native message tracked by the mapper.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageMapping {
    /// ACP message id.
    pub native_message_id: String,
    /// Fleet message id. Stable for the lifetime of the native id.
    pub fleet_message_id: String,
    /// Message role.
    pub role: MessageRole,
    /// Accumulated text.
    pub content: String,
    /// Whether `message.started` was emitted.
    pub started_emitted: bool,
    /// Whether `message.completed` was emitted.
    pub completed: bool,
    /// Insertion/update sequence used for bounded trimming.
    pub sequence: u64,
}

impl MessageMapping {
    /// Create a mapping for a newly observed native message.
    pub fn new(native_message_id: impl Into<String>, role: MessageRole) -> Self {
        let native_message_id = native_message_id.into();
        Self {
            fleet_message_id: fleet_core_common::new_id("message"),
            native_message_id,
            role,
            content: String::new(),
            started_emitted: false,
            completed: false,
            sequence: 0,
        }
    }

    /// Map key that keeps the user and agent id spaces independent.
    pub fn key(&self) -> String {
        message_key(self.role, &self.native_message_id)
    }
}

/// Build the state key for a native message id.
pub fn message_key(role: MessageRole, native_message_id: &str) -> String {
    let role = match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::System => "system",
    };
    format!("{role}:{native_message_id}")
}

/// One native tool call tracked by the mapper.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallMapping {
    /// ACP tool call id.
    pub native_tool_call_id: String,
    /// Fleet tool call id. Equal to the native id today.
    pub fleet_tool_call_id: String,
    /// Tool name.
    pub name: String,
    /// Last known status.
    pub status: ToolCallStatus,
    /// Raw input sent to the tool.
    pub input: serde_json::Value,
    /// Output reported by the tool.
    pub output: serde_json::Value,
    /// Failure message.
    pub error: Option<String>,
    /// Accumulated streamed content.
    pub content: String,
    /// Whether `tool.started` was emitted.
    pub started_emitted: bool,
    /// Monotonic update counter.
    pub last_update_sequence: u64,
}

impl ToolCallMapping {
    /// Create a mapping for a newly observed tool call.
    pub fn new(native_tool_call_id: impl Into<String>) -> Self {
        let native_tool_call_id = native_tool_call_id.into();
        Self {
            fleet_tool_call_id: native_tool_call_id.clone(),
            native_tool_call_id,
            name: "tool".to_string(),
            status: ToolCallStatus::Pending,
            input: serde_json::Value::Null,
            output: serde_json::Value::Null,
            error: None,
            content: String::new(),
            started_emitted: false,
            last_update_sequence: 0,
        }
    }

    /// Whether the current status is terminal.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            ToolCallStatus::Completed | ToolCallStatus::Failed | ToolCallStatus::Cancelled
        )
    }
}

/// Correlation between the Fleet user message and the agent's native message
/// id for the prompt currently in flight.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptCorrelation {
    /// Fleet message id recorded by the runtime before sending.
    pub fleet_message_id: String,
    /// Fleet turn waiting for this prompt.
    pub turn_id: Option<String>,
    /// Native id reported by the prompt response, once known.
    pub native_message_id: Option<String>,
}

/// A user-message echo that arrived before the prompt response.
#[derive(Debug, Clone, PartialEq)]
pub struct BufferedUserMessage {
    /// Accumulated text.
    pub content: String,
}

/// One native plan tracked by the mapper.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanMapping {
    /// ACP plan id.
    pub native_plan_id: String,
    /// Fleet plan id.
    pub fleet_plan_id: String,
    /// Current entries.
    pub entries: Vec<PlanEntry>,
    /// Whether the agent removed the plan.
    pub removed: bool,
}

impl PlanMapping {
    /// Create a mapping for a newly observed plan.
    pub fn new(native_plan_id: impl Into<String>) -> Self {
        Self {
            native_plan_id: native_plan_id.into(),
            fleet_plan_id: fleet_core_common::new_id("plan"),
            entries: Vec::new(),
            removed: false,
        }
    }
}

/// Foreground activity reported by ACP v2 state updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForegroundStatus {
    /// The agent has not reported a state yet.
    #[default]
    Unknown,
    /// Foreground work is in progress.
    Running,
    /// Foreground work is blocked on user action.
    RequiresAction,
    /// No foreground work is active.
    Idle,
}

impl ForegroundStatus {
    /// Stable wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Running => "running",
            Self::RequiresAction => "requires_action",
            Self::Idle => "idle",
        }
    }
}

/// Session foreground activity tracker.
///
/// ACP v2 state updates describe the session, not a specific prompt. This
/// tracker decides whether an activity belongs to the current Fleet turn or
/// to agent-initiated work, and never invents a user turn for the latter.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ForegroundActivityState {
    /// Latest reported state.
    pub status: ForegroundStatus,
    /// Fleet turn the activity is attributed to, when one exists.
    pub turn_id: Option<String>,
    /// Whether the agent started the activity without a user prompt.
    pub agent_initiated: bool,
    /// Background work kinds reported in the latest snapshot.
    pub background: Vec<String>,
    /// Last stop reason reported with an idle state.
    pub last_stop_reason: Option<String>,
}

impl ForegroundActivityState {
    /// Apply a v2 state update.
    ///
    /// `current_turn` is the Fleet turn that is waiting for this activity, if
    /// any. When it is `None`, a `running` transition is agent-initiated.
    pub fn apply(
        &mut self,
        next: ForegroundStatus,
        current_turn: Option<String>,
        background: Vec<String>,
        stop_reason: Option<String>,
    ) {
        self.status = next;
        self.background = background;
        match current_turn {
            Some(turn_id) => {
                self.turn_id = Some(turn_id);
                self.agent_initiated = false;
            }
            None => {
                self.turn_id = None;
                // With no Fleet turn waiting for this activity, a running or
                // blocked foreground state was started by the agent itself.
                if matches!(
                    next,
                    ForegroundStatus::Running | ForegroundStatus::RequiresAction
                ) {
                    self.agent_initiated = true;
                }
            }
        }
        if next == ForegroundStatus::Idle {
            self.last_stop_reason = stop_reason;
        }
    }

    /// Clear the turn correlation after the turn reaches a terminal state.
    pub fn clear_turn(&mut self) {
        self.turn_id = None;
        self.agent_initiated = false;
    }
}

/// Mapper state for one ACP v2 session.
#[derive(Debug, Clone, Default)]
pub struct AcpUpdateState {
    /// Native message id (keyed by role) to mapping.
    pub messages: HashMap<String, MessageMapping>,
    /// Native tool call id to mapping.
    pub tools: HashMap<String, ToolCallMapping>,
    /// Native plan id to mapping.
    pub plans: HashMap<String, PlanMapping>,
    /// Foreground activity.
    pub foreground: ForegroundActivityState,
    /// Latest plan snapshot keyed by Fleet plan id (for UI convenience).
    pub latest_plan: Option<AgentPlan>,
    /// Prompt currently awaiting acceptance.
    pub pending_prompt: Option<PromptCorrelation>,
    /// User-message echoes keyed by native id that arrived before the prompt
    /// response.
    pub buffered_user_messages: HashMap<String, BufferedUserMessage>,
    /// Update sequence counter.
    pub sequence: u64,
}

/// Upper bound on tracked messages and tool calls per session.
///
/// The runtime transcript keeps the authoritative history; the mapper only
/// needs recent ids for correlation and upserts.
const MAX_TRACKED_MESSAGES: usize = 5_000;
const MAX_TRACKED_TOOLS: usize = 5_000;
const MAX_TRACKED_PLANS: usize = 256;

impl AcpUpdateState {
    /// Allocate the next update sequence.
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    /// Bound memory for very long sessions by dropping the oldest completed
    /// messages and terminal tool calls. Late updates for a trimmed id are
    /// still safe: a new mapping is created, and terminal guards apply.
    pub fn trim(&mut self) {
        if self.messages.len() > MAX_TRACKED_MESSAGES {
            let mut sequences: Vec<(u64, String)> = self
                .messages
                .iter()
                .map(|(key, mapping)| (mapping.sequence, key.clone()))
                .collect();
            sequences.sort_unstable();
            let excess = self.messages.len() - MAX_TRACKED_MESSAGES / 2;
            for (_, key) in sequences.into_iter().take(excess) {
                if self
                    .messages
                    .get(&key)
                    .is_some_and(|mapping| mapping.completed)
                {
                    self.messages.remove(&key);
                }
            }
        }
        if self.tools.len() > MAX_TRACKED_TOOLS {
            let mut sequences: Vec<(u64, String)> = self
                .tools
                .iter()
                .map(|(key, mapping)| (mapping.last_update_sequence, key.clone()))
                .collect();
            sequences.sort_unstable();
            let excess = self.tools.len() - MAX_TRACKED_TOOLS / 2;
            for (_, key) in sequences.into_iter().take(excess) {
                if self.tools.get(&key).is_some_and(|tool| tool.is_terminal()) {
                    self.tools.remove(&key);
                }
            }
        }
        if self.plans.len() > MAX_TRACKED_PLANS {
            if let Some(key) = self.plans.keys().next().cloned() {
                self.plans.remove(&key);
            }
        }
    }

    /// Record that the runtime is about to send a prompt.
    pub fn begin_prompt(&mut self, fleet_message_id: impl Into<String>, turn_id: Option<String>) {
        self.pending_prompt = Some(PromptCorrelation {
            fleet_message_id: fleet_message_id.into(),
            turn_id,
            native_message_id: None,
        });
    }

    /// Record the prompt response's native message id.
    ///
    /// Returns the correlated Fleet message id and buffered content, when a
    /// user-message echo arrived first.
    pub fn accept_prompt(
        &mut self,
        native_message_id: impl Into<String>,
    ) -> Option<CorrelatedUserMessage> {
        let native_message_id = native_message_id.into();
        let pending = self.pending_prompt.as_ref()?.clone();
        if let Some(correlation) = self.pending_prompt.as_mut() {
            correlation.native_message_id = Some(native_message_id.clone());
        }
        let buffered = self.buffered_user_messages.remove(&native_message_id);
        let mapping = self
            .messages
            .entry(message_key(MessageRole::User, &native_message_id))
            .or_insert_with(|| MessageMapping::new(native_message_id.clone(), MessageRole::User));
        mapping.fleet_message_id = pending.fleet_message_id.clone();
        mapping.started_emitted = true;
        mapping.completed = true;
        if let Some(buffered) = &buffered {
            mapping.content = buffered.content.clone();
        }
        Some(CorrelatedUserMessage {
            fleet_message_id: pending.fleet_message_id,
            turn_id: pending.turn_id,
            content: buffered.map(|buffered| buffered.content),
        })
    }

    /// Clear prompt correlation when the turn ends.
    pub fn clear_prompt(&mut self) {
        self.pending_prompt = None;
        self.buffered_user_messages.clear();
        self.foreground.clear_turn();
    }

    /// Drop an unconfirmed prompt correlation.
    ///
    /// Used when the prompt was definitely rejected, so no native message id
    /// will ever complete the correlation. Unlike [`Self::clear_prompt`] this
    /// leaves foreground tracking untouched.
    pub fn abandon_prompt(&mut self) {
        self.pending_prompt = None;
        self.buffered_user_messages.clear();
    }
}

/// Result of correlating a prompt response with a buffered user-message echo.
#[derive(Debug, Clone, PartialEq)]
pub struct CorrelatedUserMessage {
    /// Fleet user message id.
    pub fleet_message_id: String,
    /// Fleet turn id.
    pub turn_id: Option<String>,
    /// Content echoed by the agent, when it arrived before the response.
    pub content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_ids_are_independent_per_role() {
        let mut state = AcpUpdateState::default();
        let user = MessageMapping::new("m1", MessageRole::User);
        let agent = MessageMapping::new("m1", MessageRole::Assistant);
        assert_ne!(user.key(), agent.key());
        state.messages.insert(user.key(), user);
        state.messages.insert(agent.key(), agent);
        assert_eq!(state.messages.len(), 2);
    }

    #[test]
    fn foreground_tracks_agent_initiated_work() {
        let mut foreground = ForegroundActivityState::default();
        foreground.apply(ForegroundStatus::Running, None, vec![], None);
        assert!(foreground.agent_initiated);
        assert!(foreground.turn_id.is_none());

        let mut foreground = ForegroundActivityState::default();
        foreground.apply(
            ForegroundStatus::Running,
            Some("turn_1".into()),
            vec![],
            None,
        );
        assert!(!foreground.agent_initiated);
        assert_eq!(foreground.turn_id.as_deref(), Some("turn_1"));
        foreground.apply(
            ForegroundStatus::Idle,
            None,
            vec![],
            Some("end_turn".into()),
        );
        assert_eq!(foreground.status, ForegroundStatus::Idle);
        assert_eq!(foreground.last_stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn tool_terminal_state_is_detected() {
        let mut tool = ToolCallMapping::new("t1");
        assert!(!tool.is_terminal());
        tool.status = ToolCallStatus::Completed;
        assert!(tool.is_terminal());
    }
}
