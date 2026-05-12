//! Domain entities (Session / Agent / LogEntry).

pub mod agent;
pub mod log_entry;
pub mod session;

pub use agent::{AgentEntity, AgentMapping, AgentType};
pub use log_entry::{
	AssistantLogEntry, AssistantMessage, FormattedMessage, MainAssistantLogEntry,
	MainAssistantMessage, MainLogEntry, MainUserLogEntry, MessageContent, ProgressLogEntry, Sender,
	SubAgentLogEntry, SummaryLogEntry, TaskToolInput, TextContent, ToolResult, ToolResultContent,
	ToolUse, ToolUseContent, UserLogEntry, UserMessage,
};
pub use session::{SessionEntity, SessionMetadata};
