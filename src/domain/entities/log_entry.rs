//! Log entry 型群。TS 版 `src/domain/entities/LogEntry.ts` の移植。
//!
//! JSONL の `type` タグを `#[serde(tag = "type")]` で discriminated union に
//! 乗せる。未知 `type` は `Other` variant に吸収してパース失敗にしない
//! （Claude Code のログスキーマは予告なく拡張されるため、前方互換を優先）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::agent::AgentType;

// ---------------------------------------------------------------------------
// メッセージコンテンツ (sub agent log の message.content 要素)
// ---------------------------------------------------------------------------

/// テキスト／ツール呼び出し／ツール結果の union。
///
/// 未知 type は `Other` variant に吸収する。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageContent {
	Text(TextContent),
	ToolUse(ToolUseContent),
	ToolResult(ToolResultContent),
	#[serde(other)]
	Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextContent {
	pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseContent {
	pub id: String,
	pub name: String,
	#[serde(default)]
	pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultContent {
	pub tool_use_id: String,
	#[serde(default)]
	pub content: Value,
	#[serde(default)]
	pub is_error: bool,
}

// ---------------------------------------------------------------------------
// Sub agent log entry
// ---------------------------------------------------------------------------

/// サブエージェント JSONL の 1 行 (`user` or `assistant`)。
///
/// 未知 `type` は `Other` で吸収する。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SubAgentLogEntry {
	User(UserLogEntry),
	Assistant(AssistantLogEntry),
	#[serde(other)]
	Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserLogEntry {
	#[serde(default)]
	pub parent_uuid: Option<String>,
	#[serde(default)]
	pub is_sidechain: bool,
	#[serde(default)]
	pub agent_id: Option<String>,
	#[serde(default)]
	pub session_id: Option<String>,
	#[serde(default)]
	pub uuid: Option<String>,
	#[serde(default)]
	pub timestamp: Option<DateTime<Utc>>,
	pub message: UserMessage,
	#[serde(default)]
	pub tool_use_result: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
	pub role: String,
	/// 文字列 or `ToolResultContent[]` のいずれか (raw Value で受ける)。
	pub content: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantLogEntry {
	#[serde(default)]
	pub parent_uuid: Option<String>,
	#[serde(default)]
	pub is_sidechain: bool,
	#[serde(default)]
	pub agent_id: Option<String>,
	#[serde(default)]
	pub session_id: Option<String>,
	#[serde(default)]
	pub uuid: Option<String>,
	#[serde(default)]
	pub timestamp: Option<DateTime<Utc>>,
	pub message: AssistantMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
	#[serde(default)]
	pub model: Option<String>,
	pub role: String,
	pub content: Vec<MessageContent>,
	/// Claude Code のサブエージェントログでは、assistant entry の
	/// `message.stop_reason` で「Main への最終応答か」「ツール呼び出しの停止か」
	/// 「途中 text (独り言)」を区別できる。
	///
	/// - `"end_turn"`: Main に tool_result として届く最終応答
	/// - `"tool_use"`: ツール呼び出しのための停止 (中間状態)
	/// - `null` / 省略: 中間の text 発話 (Main には届かない)
	///
	/// 未知値は受理するが `is_final_response` 判定は `"end_turn"` リテラル一致のみ。
	#[serde(default)]
	pub stop_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Main log entry (メインセッション jsonl)
// ---------------------------------------------------------------------------

/// メインログ 1 行。`AgentMapperImpl` が走査してマッピングを構築する。
///
/// Issue 002 の知見: `tool_use` の `name` は `"Agent" | "Task"` の両方を受理。
/// `subagent_type` が未指定なら `general-purpose` にフォールバック。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MainLogEntry {
	Assistant(MainAssistantLogEntry),
	Progress(ProgressLogEntry),
	User(MainUserLogEntry),
	Summary(SummaryLogEntry),
	#[serde(other)]
	Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MainAssistantLogEntry {
	pub message: MainAssistantMessage,
	#[serde(default)]
	pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MainAssistantMessage {
	#[serde(default)]
	pub model: Option<String>,
	pub role: String,
	/// Agent/Task tool_use を含む可能性がある。個別要素は `MessageContent` で
	/// カバーしきれないので Value で受けて、マッパー側で `type` と `name` を
	/// 見て振り分ける。
	pub content: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressLogEntry {
	pub data: AgentProgressData,
	/// TS 側のフィールド名は末尾が `ID` (大文字) なので `camelCase` rename_all
	/// では合わず、手動で rename する。
	#[serde(default, rename = "parentToolUseID")]
	pub parent_tool_use_id: Option<String>,
	#[serde(default)]
	pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProgressData {
	#[serde(rename = "type")]
	pub kind: String,
	pub agent_id: String,
	#[serde(default)]
	pub prompt: Option<String>,
}

/// メインログの `user` エントリ。Background agent の非同期 tool_result 判定に使う。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MainUserLogEntry {
	#[serde(default)]
	pub message: Option<Value>,
	#[serde(default)]
	pub tool_use_result: Option<Value>,
	#[serde(default)]
	pub timestamp: Option<DateTime<Utc>>,
}

/// セッション要約エントリ (セッション一覧の `summary` 表示に使う)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryLogEntry {
	pub summary: String,
	#[serde(default)]
	pub leaf_uuid: Option<String>,
}

/// Agent / Task tool call の input ペイロード (マッパーで `serde_json::from_value`
/// して取り出す)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskToolInput {
	#[serde(default)]
	pub description: Option<String>,
	#[serde(default)]
	pub prompt: Option<String>,
	/// 省略/null のときは `DEFAULT_SUBAGENT_TYPE` にフォールバックするのは
	/// マッパー側の責務。ここでは Option で受けるだけ。
	#[serde(default)]
	pub subagent_type: Option<AgentType>,
	#[serde(default)]
	pub model: Option<String>,
}

// ---------------------------------------------------------------------------
// 表示用 FormattedMessage (application/ui 層向け)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sender {
	Main,
	Sub,
}

/// UI が描画に使う整形済みメッセージ。
///
/// ツール呼び出し (tool_use) と対応する結果 (tool_result) は 1 メッセージに
/// **ペアリング** される (Slack のスレッド返信相当)。ペアリングのキーは
/// `tool_use_id` で、後から到着した `tool_result` は `application::app`
/// レイヤで逆順 linear search により同じ id を持つメッセージに attach される
/// (`MapperOutcome::AttachResult`)。
///
/// - `tool_use=Some, tool_result=None`: 待機中 (結果未到着 or orphan tool_use)
/// - `tool_use=Some, tool_result=Some`: 統合バブル (Slack スレッド風表示)
/// - `tool_use=None,  tool_result=Some`: orphan 結果 (先頭 drain 済み等)
/// - `text=Some`: 通常テキスト
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormattedMessage {
	/// 安定した識別子 (`expanded_ids` のキー)。`${timestamp_ms}-${uuid}-${idx}`。
	pub id: String,
	pub sender: Sender,
	pub agent_id: String,
	pub timestamp: DateTime<Utc>,
	#[serde(default)]
	pub text: Option<String>,
	#[serde(default)]
	pub tool_use: Option<ToolUse>,
	#[serde(default)]
	pub tool_result: Option<ToolResult>,
	/// 対応する tool_use / tool_result の `tool_use_id`。ペアリングのキー。
	/// `tool_use` / `tool_result` いずれかが Some なら基本的に埋まる。
	#[serde(default)]
	pub tool_use_id: Option<String>,
	/// `tool_result` が attach された時刻 (統合バブルでは結果側の timestamp、
	/// orphan tool_result のときは結果の timestamp をそのまま入れる)。
	#[serde(default)]
	pub result_timestamp: Option<DateTime<Utc>>,
	/// assistant の `stop_reason == "end_turn"` のときの**最後の text item** のみ
	/// true。Main に tool_result として届く「最終応答」を意味する。
	///
	/// UI はこのフラグを見て:
	/// - Sub + text + true → mention prefix `@Main` を付ける (Main 宛てアナウンス)
	/// - Sub + text + false → mention なし + 本文を薄色 (独り言 / 中間発話)
	///
	/// tool_use / tool_result バブルは常に false (mention / 薄色対象ではない)。
	/// Main からの prompt (user entry 経由) は常に false。
	#[serde(default)]
	pub is_final_response: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUse {
	pub name: String,
	pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
	pub content: String,
	pub is_error: bool,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn sub_agent_log_entry_parses_user_variant() {
		let raw = r#"{
			"type": "user",
			"message": {"role": "user", "content": "hi"},
			"timestamp": "2026-04-22T00:00:00Z"
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		match entry {
			SubAgentLogEntry::User(u) => {
				assert_eq!(u.message.role, "user");
			}
			_ => panic!("expected User variant"),
		}
	}

	#[test]
	fn sub_agent_log_entry_falls_through_to_other_for_unknown_type() {
		let raw = r#"{"type":"mystery","payload":42}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		assert!(matches!(entry, SubAgentLogEntry::Other));
	}

	#[test]
	fn main_log_entry_progress_variant() {
		let raw = r#"{
			"type": "progress",
			"data": {
				"type": "agent_progress",
				"agentId": "abc123"
			},
			"parentToolUseID": "tool_001"
		}"#;
		let entry: MainLogEntry = serde_json::from_str(raw).unwrap();
		match entry {
			MainLogEntry::Progress(p) => {
				assert_eq!(p.data.agent_id, "abc123");
				assert_eq!(p.parent_tool_use_id.as_deref(), Some("tool_001"));
			}
			_ => panic!("expected Progress variant"),
		}
	}

	#[test]
	fn message_content_tool_use_parses_agent_name() {
		let raw = r#"{
			"type": "tool_use",
			"id": "t1",
			"name": "Agent",
			"input": {"subagent_type": null, "description": "x", "prompt": "y"}
		}"#;
		let content: MessageContent = serde_json::from_str(raw).unwrap();
		match content {
			MessageContent::ToolUse(t) => {
				assert_eq!(t.name, "Agent");
				assert_eq!(t.id, "t1");
			}
			_ => panic!("expected ToolUse variant"),
		}
	}

	#[test]
	fn assistant_message_stop_reason_end_turn() {
		let raw = r#"{
			"role":"assistant",
			"content":[{"type":"text","text":"done"}],
			"stop_reason":"end_turn"
		}"#;
		let msg: AssistantMessage = serde_json::from_str(raw).unwrap();
		assert_eq!(msg.stop_reason.as_deref(), Some("end_turn"));
	}

	#[test]
	fn assistant_message_stop_reason_explicit_null() {
		let raw = r#"{
			"role":"assistant",
			"content":[{"type":"text","text":"mid"}],
			"stop_reason":null
		}"#;
		let msg: AssistantMessage = serde_json::from_str(raw).unwrap();
		assert!(msg.stop_reason.is_none());
	}

	#[test]
	fn assistant_message_stop_reason_absent() {
		let raw = r#"{
			"role":"assistant",
			"content":[{"type":"text","text":"mid"}]
		}"#;
		let msg: AssistantMessage = serde_json::from_str(raw).unwrap();
		assert!(msg.stop_reason.is_none());
	}

	#[test]
	fn task_tool_input_accepts_null_subagent_type() {
		let raw = r#"{"description": "d", "prompt": "p", "subagent_type": null}"#;
		let input: TaskToolInput = serde_json::from_str(raw).unwrap();
		assert!(input.subagent_type.is_none());
	}
}
