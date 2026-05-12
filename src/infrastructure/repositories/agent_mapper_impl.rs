//! AgentMapperImpl。TS 版 `src/infrastructure/repositories/AgentMapperImpl.ts`
//! + `src/domain/services/AgentMappingService.ts` の統合移植。
//!
//! メインログの `assistant` エントリから Agent / Task tool_use を拾って pending
//! に入れ、`progress` イベント or Background agent の async `tool_result` で
//! `agentId` と紐付けて確定する。
//!
//! ## Issue 002 対応
//!
//! - tool_use.name は `"Agent" | "Task"` の両方を受理する
//! - `subagent_type` が `null` / 省略 / 空文字のときは `general-purpose` に
//!   フォールバック (Claude Code のデフォルトと一致)

use std::collections::HashMap;

use serde_json::Value;

use crate::domain::constants::DEFAULT_SUBAGENT_TYPE;
use crate::domain::entities::{
	AgentMapping, AgentType, MainAssistantLogEntry, MainLogEntry, MainUserLogEntry,
	ProgressLogEntry, TaskToolInput,
};

/// 確定後の agent 詳細 (`AgentMappingService.getMapping` 相当)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDetails {
	pub agent_type: AgentType,
	pub description: Option<String>,
	pub model: Option<String>,
}

/// assistant エントリで回収し、progress 到達時に確定する pending 情報。
#[derive(Debug, Clone)]
struct PendingToolUse {
	subagent_type: AgentType,
	description: Option<String>,
	model: Option<String>,
}

/// メインログを走査してマッピングを構築するオブジェクト。
///
/// TS 版 AgentMapperImpl を 1 つに畳み込み (Rust では AgentMappingService を
/// 別クラスにする利点がないため)。
#[derive(Debug, Default)]
pub struct AgentMapperImpl {
	pending: HashMap<String, PendingToolUse>,
	mappings: HashMap<String, AgentDetails>,
	default_model: Option<String>,
}

impl AgentMapperImpl {
	pub fn new() -> Self {
		Self::default()
	}

	/// メインログの 1 エントリを処理する。マッピングが確定したときだけ
	/// `Some(AgentMapping)` を返す。
	pub fn process_entry(&mut self, entry: &MainLogEntry) -> Option<AgentMapping> {
		match entry {
			MainLogEntry::Assistant(a) => {
				self.process_assistant_entry(a);
				None
			}
			MainLogEntry::Progress(p) => self.process_progress_entry(p),
			MainLogEntry::User(u) if is_async_tool_result(u) => {
				self.process_async_tool_result_entry(u)
			}
			_ => None,
		}
	}

	fn process_assistant_entry(&mut self, entry: &MainAssistantLogEntry) {
		// 最初の assistant エントリからデフォルトモデルを設定 (上書きしない)
		if self.default_model.is_none() {
			if let Some(m) = entry.message.model.as_deref() {
				if !m.is_empty() {
					self.default_model = Some(m.to_string());
				}
			}
		}

		for item in &entry.message.content {
			let Some(obj) = item.as_object() else {
				continue;
			};
			if obj.get("type").and_then(Value::as_str) != Some("tool_use") {
				continue;
			}
			let name = obj.get("name").and_then(Value::as_str).unwrap_or("");
			if name != "Agent" && name != "Task" {
				continue;
			}
			let Some(id) = obj.get("id").and_then(Value::as_str) else {
				continue;
			};
			if id.is_empty() {
				continue;
			}

			let input_value = obj.get("input").cloned().unwrap_or(Value::Null);
			let parsed_input: TaskToolInput =
				serde_json::from_value(input_value).unwrap_or(TaskToolInput {
					description: None,
					prompt: None,
					subagent_type: None,
					model: None,
				});

			let subagent_type = parsed_input
				.subagent_type
				.filter(|s| !s.is_empty())
				.unwrap_or_else(|| DEFAULT_SUBAGENT_TYPE.to_string());

			self.pending.insert(
				id.to_string(),
				PendingToolUse {
					subagent_type,
					description: parsed_input.description,
					model: parsed_input.model,
				},
			);
		}
	}

	fn process_progress_entry(&mut self, entry: &ProgressLogEntry) -> Option<AgentMapping> {
		let agent_id = entry.data.agent_id.as_str();
		if agent_id.is_empty() {
			return None;
		}
		let parent = entry.parent_tool_use_id.as_deref()?;
		if self.mappings.contains_key(agent_id) {
			return None;
		}
		let pending = self.pending.remove(parent)?;

		let model = pending.model.clone().or_else(|| self.default_model.clone());

		let mapping = AgentMapping {
			agent_id: agent_id.to_string(),
			subagent_type: pending.subagent_type.clone(),
			tool_use_id: parent.to_string(),
			description: pending.description.clone(),
			model: model.clone(),
		};

		self.mappings.insert(
			agent_id.to_string(),
			AgentDetails {
				agent_type: pending.subagent_type,
				description: pending.description,
				model,
			},
		);

		Some(mapping)
	}

	fn process_async_tool_result_entry(
		&mut self,
		entry: &MainUserLogEntry,
	) -> Option<AgentMapping> {
		// toolUseResult.agentId を取得
		let tur = entry.tool_use_result.as_ref()?;
		let agent_id = tur.as_object()?.get("agentId").and_then(Value::as_str)?;
		if agent_id.is_empty() {
			return None;
		}
		if self.mappings.contains_key(agent_id) {
			return None;
		}

		// message.content[].tool_use_id を走査して pending をヒットさせる
		let message = entry.message.as_ref()?;
		let content = message.as_object()?.get("content")?.as_array()?;
		for item in content {
			let obj = match item.as_object() {
				Some(o) => o,
				None => continue,
			};
			if obj.get("type").and_then(Value::as_str) != Some("tool_result") {
				continue;
			}
			let tool_use_id = match obj.get("tool_use_id").and_then(Value::as_str) {
				Some(s) if !s.is_empty() => s,
				_ => continue,
			};
			let Some(pending) = self.pending.remove(tool_use_id) else {
				continue;
			};
			let model = pending.model.clone().or_else(|| self.default_model.clone());
			let mapping = AgentMapping {
				agent_id: agent_id.to_string(),
				subagent_type: pending.subagent_type.clone(),
				tool_use_id: tool_use_id.to_string(),
				description: pending.description.clone(),
				model: model.clone(),
			};
			self.mappings.insert(
				agent_id.to_string(),
				AgentDetails {
					agent_type: pending.subagent_type,
					description: pending.description,
					model,
				},
			);
			return Some(mapping);
		}
		None
	}

	// -----------------------------------------------------------------------
	// Accessors
	// -----------------------------------------------------------------------

	pub fn get_mapping(&self, agent_id: &str) -> Option<&AgentDetails> {
		self.mappings.get(agent_id)
	}

	pub fn get_all_mappings(&self) -> &HashMap<String, AgentDetails> {
		&self.mappings
	}

	pub fn default_model(&self) -> Option<&str> {
		self.default_model.as_deref()
	}

	pub fn pending_count(&self) -> usize {
		self.pending.len()
	}
}

fn is_async_tool_result(entry: &MainUserLogEntry) -> bool {
	let Some(tur) = entry.tool_use_result.as_ref() else {
		return false;
	};
	let Some(obj) = tur.as_object() else {
		return false;
	};
	let is_async = obj.get("isAsync").and_then(Value::as_bool).unwrap_or(false);
	let agent_id_ok = obj
		.get("agentId")
		.and_then(Value::as_str)
		.map(|s| !s.is_empty())
		.unwrap_or(false);
	is_async && agent_id_ok
}

#[cfg(test)]
mod tests {
	use super::*;

	fn parse_main(raw: &str) -> MainLogEntry {
		serde_json::from_str(raw).expect("parse main log entry")
	}

	#[test]
	fn extracts_task_tool_use_and_sets_default_model() {
		let mut mapper = AgentMapperImpl::new();
		let entry = parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"claude-sonnet-4-5-20250929",
					"role":"assistant",
					"content":[{
						"type":"tool_use","id":"tool-1","name":"Task",
						"input":{"subagent_type":"Explore","description":"Search files","prompt":"Find all test files"}
					}]
				}
			}"#,
		);
		let result = mapper.process_entry(&entry);
		assert!(result.is_none());
		assert_eq!(mapper.pending_count(), 1);
		assert_eq!(mapper.default_model(), Some("claude-sonnet-4-5-20250929"));
	}

	#[test]
	fn progress_binds_agent_id_to_pending() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"claude-sonnet-4-5-20250929",
					"role":"assistant",
					"content":[{
						"type":"tool_use","id":"tool-1","name":"Task",
						"input":{"subagent_type":"Explore","description":"Search files","prompt":"Find all test files"}
					}]
				}
			}"#,
		));

		let result = mapper.process_entry(&parse_main(
			r#"{
				"type":"progress",
				"data":{"type":"agent_progress","agentId":"agent-abc"},
				"parentToolUseID":"tool-1"
			}"#,
		));
		let mapping = result.expect("expected mapping");
		assert_eq!(mapping.agent_id, "agent-abc");
		assert_eq!(mapping.subagent_type, "Explore");
		assert_eq!(mapping.description.as_deref(), Some("Search files"));
		assert_eq!(mapper.pending_count(), 0);

		let details = mapper.get_mapping("agent-abc").expect("mapping saved");
		assert_eq!(details.agent_type, "Explore");
	}

	#[test]
	fn agent_name_is_accepted_in_addition_to_task() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"claude-sonnet-4-5-20250929",
					"role":"assistant",
					"content":[{
						"type":"tool_use","id":"tool-ag","name":"Agent",
						"input":{"subagent_type":"Explore","description":"Search","prompt":"p"}
					}]
				}
			}"#,
		));
		let mapping = mapper
			.process_entry(&parse_main(
				r#"{
					"type":"progress",
					"data":{"type":"agent_progress","agentId":"agent-agent"},
					"parentToolUseID":"tool-ag"
				}"#,
			))
			.expect("mapping");
		assert_eq!(mapping.subagent_type, "Explore");
	}

	#[test]
	fn null_subagent_type_falls_back_to_general_purpose() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"m",
					"role":"assistant",
					"content":[{
						"type":"tool_use","id":"tool-null","name":"Agent",
						"input":{"subagent_type":null,"description":"d","prompt":"p"}
					}]
				}
			}"#,
		));
		let mapping = mapper
			.process_entry(&parse_main(
				r#"{
					"type":"progress",
					"data":{"type":"agent_progress","agentId":"a1"},
					"parentToolUseID":"tool-null"
				}"#,
			))
			.expect("mapping");
		assert_eq!(mapping.subagent_type, "general-purpose");
	}

	#[test]
	fn missing_subagent_type_falls_back_to_general_purpose() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"m",
					"role":"assistant",
					"content":[{
						"type":"tool_use","id":"tool-miss","name":"Agent",
						"input":{"description":"d","prompt":"p"}
					}]
				}
			}"#,
		));
		let mapping = mapper
			.process_entry(&parse_main(
				r#"{
					"type":"progress",
					"data":{"type":"agent_progress","agentId":"a2"},
					"parentToolUseID":"tool-miss"
				}"#,
			))
			.expect("mapping");
		assert_eq!(mapping.subagent_type, "general-purpose");
	}

	#[test]
	fn legacy_task_with_missing_subagent_type_falls_back() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"m",
					"role":"assistant",
					"content":[{
						"type":"tool_use","id":"tool-legacy","name":"Task",
						"input":{"description":"Legacy","prompt":"p"}
					}]
				}
			}"#,
		));
		let mapping = mapper
			.process_entry(&parse_main(
				r#"{
					"type":"progress",
					"data":{"type":"agent_progress","agentId":"a3"},
					"parentToolUseID":"tool-legacy"
				}"#,
			))
			.expect("mapping");
		assert_eq!(mapping.subagent_type, "general-purpose");
	}

	#[test]
	fn background_agent_async_tool_result_confirms_mapping() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"m",
					"role":"assistant",
					"content":[{
						"type":"tool_use","id":"tool-2","name":"Task",
						"input":{"subagent_type":"Plan","description":"Planning task","prompt":"Create plan"}
					}]
				}
			}"#,
		));
		let mapping = mapper
			.process_entry(&parse_main(
				r#"{
					"type":"user",
					"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-2","content":"Plan completed"}]},
					"toolUseResult":{"isAsync":true,"agentId":"agent-bg1","status":"completed"}
				}"#,
			))
			.expect("mapping");
		assert_eq!(mapping.agent_id, "agent-bg1");
		assert_eq!(mapping.subagent_type, "Plan");
		assert_eq!(mapper.pending_count(), 0);
	}

	#[test]
	fn non_task_tool_use_is_ignored() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"m",
					"role":"assistant",
					"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/x"}}]
				}
			}"#,
		));
		assert_eq!(mapper.pending_count(), 0);
	}

	#[test]
	fn multiple_tool_uses_only_task_is_kept() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"m",
					"role":"assistant",
					"content":[
						{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/x"}},
						{"type":"tool_use","id":"t1","name":"Task","input":{"subagent_type":"Explore","description":"s","prompt":"p"}}
					]
				}
			}"#,
		));
		assert_eq!(mapper.pending_count(), 1);
	}

	#[test]
	fn default_model_is_not_overwritten_by_later_entries() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{"type":"assistant","message":{"model":"claude-opus-4-6","role":"assistant","content":[{"type":"text","text":"hi"}]}}"#,
		));
		mapper.process_entry(&parse_main(
			r#"{"type":"assistant","message":{"model":"claude-sonnet-4-5-20250929","role":"assistant","content":[{"type":"text","text":"x"}]}}"#,
		));
		assert_eq!(mapper.default_model(), Some("claude-opus-4-6"));
	}

	#[test]
	fn task_input_model_wins_over_default_model() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"claude-opus-4-6",
					"role":"assistant",
					"content":[{
						"type":"tool_use","id":"tool-1","name":"Task",
						"input":{"subagent_type":"Explore","description":"s","prompt":"p","model":"claude-sonnet-4-5-20250929"}
					}]
				}
			}"#,
		));
		let mapping = mapper
			.process_entry(&parse_main(
				r#"{"type":"progress","data":{"type":"agent_progress","agentId":"agent-1"},"parentToolUseID":"tool-1"}"#,
			))
			.expect("mapping");
		assert_eq!(mapping.model.as_deref(), Some("claude-sonnet-4-5-20250929"));
	}

	#[test]
	fn progress_without_pending_returns_none() {
		let mut mapper = AgentMapperImpl::new();
		let result = mapper.process_entry(&parse_main(
			r#"{"type":"progress","data":{"type":"agent_progress","agentId":"x"},"parentToolUseID":"missing"}"#,
		));
		assert!(result.is_none());
	}

	#[test]
	fn already_mapped_agent_is_not_duplicated() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"m",
					"role":"assistant",
					"content":[{"type":"tool_use","id":"tool-1","name":"Task","input":{"subagent_type":"Explore","description":"s","prompt":"p"}}]
				}
			}"#,
		));
		let progress = r#"{"type":"progress","data":{"type":"agent_progress","agentId":"agent-abc"},"parentToolUseID":"tool-1"}"#;
		assert!(mapper.process_entry(&parse_main(progress)).is_some());
		assert_eq!(mapper.get_all_mappings().len(), 1);
		assert!(mapper.process_entry(&parse_main(progress)).is_none());
		assert_eq!(mapper.get_all_mappings().len(), 1);
	}

	#[test]
	fn empty_agent_id_in_progress_is_ignored() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"m",
					"role":"assistant",
					"content":[{"type":"tool_use","id":"tool-1","name":"Task","input":{"subagent_type":"Explore","description":"s","prompt":"p"}}]
				}
			}"#,
		));
		let result = mapper.process_entry(&parse_main(
			r#"{"type":"progress","data":{"type":"agent_progress","agentId":""},"parentToolUseID":"tool-1"}"#,
		));
		assert!(result.is_none());
	}

	#[test]
	fn unknown_entry_type_returns_none() {
		let mut mapper = AgentMapperImpl::new();
		let result = mapper.process_entry(&parse_main(r#"{"type":"unknown_type"}"#));
		assert!(result.is_none());
		assert_eq!(mapper.pending_count(), 0);
	}

	#[test]
	fn pending_cleanup_after_multiple_mappings() {
		let mut mapper = AgentMapperImpl::new();
		mapper.process_entry(&parse_main(
			r#"{
				"type":"assistant",
				"message":{
					"model":"m",
					"role":"assistant",
					"content":[
						{"type":"tool_use","id":"tool-1","name":"Task","input":{"subagent_type":"Explore","description":"s","prompt":"p"}},
						{"type":"tool_use","id":"tool-2","name":"Task","input":{"subagent_type":"Bash","description":"r","prompt":"e"}}
					]
				}
			}"#,
		));
		assert_eq!(mapper.pending_count(), 2);
		mapper.process_entry(&parse_main(
			r#"{"type":"progress","data":{"type":"agent_progress","agentId":"agent-1"},"parentToolUseID":"tool-1"}"#,
		));
		assert_eq!(mapper.pending_count(), 1);
		mapper.process_entry(&parse_main(
			r#"{"type":"progress","data":{"type":"agent_progress","agentId":"agent-2"},"parentToolUseID":"tool-2"}"#,
		));
		assert_eq!(mapper.pending_count(), 0);
		assert_eq!(mapper.get_all_mappings().len(), 2);
	}
}
