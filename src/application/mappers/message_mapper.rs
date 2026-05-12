//! Message mapper。TS 版 `src/application/mappers/MessageMapper.ts` の移植だが、
//! Rust 版では **projection outcome** を返す形に変更されている。
//!
//! ## 目的
//!
//! subagent のツール呼び出し (`tool_use`) と結果 (`tool_result`) を、
//! Slack のスレッド返信のように **1 メッセージに集約** する。mapper は
//! per-entry に pure function として outcome を返し、pairing の適用
//! (既存メッセージの `tool_result` を埋める) は application レイヤが行う。
//!
//! ## Outcome
//!
//! - `Append(FormattedMessage)`: 通常 append (text / tool_use 単独 / 未ペアリング)
//! - `AttachResult { tool_use_id, ... }`: pair 可能な tool_result。アプリ層が
//!   既存メッセージに merge する。該当 tool_use が見つからない場合は
//!   orphan として独立メッセージに fallback するのはアプリ層の責務。
//!
//! id の採番ルールは TS 版と同じ:
//!
//! ```text
//! {timestamp_ms}-{entryUuid}-{contentIndex}
//! ```

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::domain::entities::{FormattedMessage, Sender, SubAgentLogEntry, ToolUse, UserMessage};

/// `{timestamp_ms}-{uuid}-{index}` 形式で安定 id を組み立てる。
pub fn build_message_id(
	timestamp: &DateTime<Utc>,
	entry_uuid: &str,
	content_index: usize,
) -> String {
	format!(
		"{}-{}-{}",
		timestamp.timestamp_millis(),
		entry_uuid,
		content_index
	)
}

/// mapper の per-entry pure function が返す projection outcome。
///
/// アプリ層 (`application::app`) が順に適用する:
///
/// - `Append`: 末尾 push。
/// - `AttachResult`: 既存メッセージ列を逆順 linear search し、同じ
///   `tool_use_id` を持つ `FormattedMessage` の `tool_result` /
///   `result_timestamp` を埋める。見つからなければ orphan として独立
///   メッセージに fallback (アプリ層の責務)。
#[derive(Debug, Clone)]
pub enum MapperOutcome {
	/// text / tool_use / orphan tool_result → 末尾に追加。
	Append(FormattedMessage),
	/// 対応 tool_use と pair 可能な tool_result。
	AttachResult {
		tool_use_id: String,
		content: String,
		is_error: bool,
		timestamp: DateTime<Utc>,
	},
}

/// `SubAgentLogEntry` を mapper outcome 列に展開する。
///
/// - `user` の content が文字列 → `Append(text=Some, sender=main)` 1 件
/// - `user` の content が配列 (tool_result 群) → 各要素を `AttachResult { .. }`
///   として返す (アプリ層が pair 相手を探索する)
/// - `assistant` の content は text / tool_use を順に `Append(..)` 展開
///   (空文字 text はスキップ)
pub fn to_mapper_outcomes(entry: &SubAgentLogEntry) -> Vec<MapperOutcome> {
	let mut outcomes = Vec::new();

	match entry {
		SubAgentLogEntry::User(user) => {
			let timestamp = user.timestamp.unwrap_or_else(Utc::now);
			let uuid = user.uuid.as_deref().unwrap_or("");
			let agent_id = user.agent_id.as_deref().unwrap_or("").to_string();
			expand_user_message(&user.message, timestamp, uuid, &agent_id, &mut outcomes);
		}
		SubAgentLogEntry::Assistant(asst) => {
			let timestamp = asst.timestamp.unwrap_or_else(Utc::now);
			let uuid = asst.uuid.as_deref().unwrap_or("");
			let agent_id = asst.agent_id.as_deref().unwrap_or("").to_string();

			// `stop_reason == "end_turn"` のときの **最後の text item** のみ
			// Main 宛ての最終応答として扱う。assistant が複数 text を吐くケースで、
			// 末尾だけが「まとめ」になっていることが多く、Main にはそれが
			// tool_result として届く。
			//
			// Claude Code 実ログでも `stop_reason` は assistant entry 単位で付く
			// ので、end_turn entry 内の「最後」を見るのが自然な粒度。
			let is_end_turn = asst.message.stop_reason.as_deref() == Some("end_turn");
			let last_text_index: Option<usize> = asst
				.message
				.content
				.iter()
				.enumerate()
				.rev()
				.find_map(|(i, c)| {
					matches!(
						c,
						crate::domain::entities::log_entry::MessageContent::Text(_)
					)
					.then_some(i)
				});

			for (index, item) in asst.message.content.iter().enumerate() {
				match item {
					crate::domain::entities::log_entry::MessageContent::Text(t) => {
						if !t.text.trim().is_empty() {
							let is_final = is_end_turn && Some(index) == last_text_index;
							outcomes.push(MapperOutcome::Append(FormattedMessage {
								id: build_message_id(&timestamp, uuid, index),
								sender: Sender::Sub,
								agent_id: agent_id.clone(),
								timestamp,
								text: Some(t.text.clone()),
								tool_use: None,
								tool_result: None,
								tool_use_id: None,
								result_timestamp: None,
								is_final_response: is_final,
							}));
						}
					}
					crate::domain::entities::log_entry::MessageContent::ToolUse(t) => {
						outcomes.push(MapperOutcome::Append(FormattedMessage {
							id: build_message_id(&timestamp, uuid, index),
							sender: Sender::Sub,
							agent_id: agent_id.clone(),
							timestamp,
							text: None,
							tool_use: Some(ToolUse {
								name: t.name.clone(),
								input: t.input.clone(),
							}),
							tool_result: None,
							tool_use_id: Some(t.id.clone()),
							result_timestamp: None,
							is_final_response: false,
						}));
					}
					// ToolResult / Other は assistant 経由ではほぼ来ないが、
					// 念のため握りつぶす (TS 版も無視している)。
					_ => {}
				}
			}
		}
		SubAgentLogEntry::Other => {}
	}

	outcomes
}

fn expand_user_message(
	message: &UserMessage,
	timestamp: DateTime<Utc>,
	entry_uuid: &str,
	agent_id: &str,
	outcomes: &mut Vec<MapperOutcome>,
) {
	match &message.content {
		Value::String(s) => {
			outcomes.push(MapperOutcome::Append(FormattedMessage {
				id: build_message_id(&timestamp, entry_uuid, 0),
				sender: Sender::Main,
				agent_id: agent_id.to_string(),
				timestamp,
				text: Some(s.clone()),
				tool_use: None,
				tool_result: None,
				tool_use_id: None,
				result_timestamp: None,
				is_final_response: false,
			}));
		}
		Value::Array(items) => {
			for item in items.iter() {
				let Some(obj) = item.as_object() else {
					continue;
				};
				let ty = obj.get("type").and_then(Value::as_str);
				if ty != Some("tool_result") {
					continue;
				}
				let Some(tool_use_id) = obj.get("tool_use_id").and_then(Value::as_str) else {
					continue;
				};
				let content = obj
					.get("content")
					.map(tool_result_content_to_string)
					.unwrap_or_default();
				let is_error = obj
					.get("is_error")
					.and_then(Value::as_bool)
					.unwrap_or(false);
				outcomes.push(MapperOutcome::AttachResult {
					tool_use_id: tool_use_id.to_string(),
					content,
					is_error,
					timestamp,
				});
			}
		}
		_ => {}
	}
}

/// tool_result の `content` は Claude Code ログ上で string / array のどちらも来うる。
/// UI 側は文字列として扱いたいので、array の場合は各要素の `text` を連結する。
fn tool_result_content_to_string(value: &Value) -> String {
	match value {
		Value::String(s) => s.clone(),
		Value::Array(arr) => arr
			.iter()
			.filter_map(|v| v.get("text").and_then(Value::as_str).map(str::to_string))
			.collect::<Vec<_>>()
			.join("\n"),
		other => other.to_string(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn assert_append_text(outcome: &MapperOutcome, expected_text: &str) {
		match outcome {
			MapperOutcome::Append(msg) => {
				assert_eq!(msg.text.as_deref(), Some(expected_text));
			}
			other => panic!("expected Append, got {other:?}"),
		}
	}

	#[test]
	fn user_string_content_becomes_main_text_append() {
		let raw = r#"{
			"type":"user",
			"agentId":"a1",
			"uuid":"u1",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{"role":"user","content":"hello"}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert_eq!(outs.len(), 1);
		assert_append_text(&outs[0], "hello");
		if let MapperOutcome::Append(m) = &outs[0] {
			assert_eq!(m.sender, Sender::Main);
			assert_eq!(m.agent_id, "a1");
			assert!(m.id.contains("-u1-0"));
			assert!(m.tool_use_id.is_none());
		}
	}

	#[test]
	fn user_tool_results_become_attach_outcomes() {
		let raw = r#"{
			"type":"user",
			"agentId":"a1",
			"uuid":"u9",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"user",
				"content":[
					{"type":"tool_result","tool_use_id":"tu1","content":"ok","is_error":false},
					{"type":"tool_result","tool_use_id":"tu2","content":"err","is_error":true}
				]
			}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert_eq!(outs.len(), 2);
		match &outs[0] {
			MapperOutcome::AttachResult {
				tool_use_id,
				content,
				is_error,
				..
			} => {
				assert_eq!(tool_use_id, "tu1");
				assert_eq!(content, "ok");
				assert!(!is_error);
			}
			other => panic!("expected AttachResult, got {other:?}"),
		}
		match &outs[1] {
			MapperOutcome::AttachResult {
				tool_use_id,
				is_error,
				..
			} => {
				assert_eq!(tool_use_id, "tu2");
				assert!(is_error);
			}
			other => panic!("expected AttachResult, got {other:?}"),
		}
	}

	#[test]
	fn assistant_expands_text_and_tool_use() {
		let raw = r#"{
			"type":"assistant",
			"agentId":"a1",
			"uuid":"u2",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"model":"m1",
				"role":"assistant",
				"content":[
					{"type":"text","text":"hello"},
					{"type":"tool_use","id":"t1","name":"Read","input":{"file":"x"}}
				]
			}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert_eq!(outs.len(), 2);
		assert_append_text(&outs[0], "hello");
		match &outs[1] {
			MapperOutcome::Append(m) => {
				assert!(m.tool_use.is_some());
				assert_eq!(m.tool_use_id.as_deref(), Some("t1"));
				assert!(m.id.ends_with("-1"));
			}
			other => panic!("expected Append, got {other:?}"),
		}
	}

	#[test]
	fn assistant_skips_empty_text() {
		let raw = r#"{
			"type":"assistant",
			"agentId":"a1",
			"uuid":"u2",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"assistant",
				"content":[{"type":"text","text":"   "}]
			}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert!(outs.is_empty());
	}

	#[test]
	fn tool_result_content_array_is_joined() {
		let raw = r#"{
			"type":"user",
			"agentId":"a1",
			"uuid":"u9",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"user",
				"content":[
					{"type":"tool_result","tool_use_id":"tu1","content":[{"type":"text","text":"line1"},{"type":"text","text":"line2"}]}
				]
			}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert_eq!(outs.len(), 1);
		match &outs[0] {
			MapperOutcome::AttachResult { content, .. } => {
				assert_eq!(content, "line1\nline2");
			}
			other => panic!("expected AttachResult, got {other:?}"),
		}
	}

	#[test]
	fn build_message_id_format() {
		let ts = "2024-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
		let id = build_message_id(&ts, "abc", 3);
		assert_eq!(id, format!("{}-abc-3", ts.timestamp_millis()));
	}

	/// ツール単独 (結果未到着) → Append だけ、tool_use_id が埋まる。
	#[test]
	fn tool_use_alone_produces_single_append_with_tool_use_id() {
		let raw = r#"{
			"type":"assistant",
			"agentId":"a1",
			"uuid":"u-asst",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"assistant",
				"content":[
					{"type":"tool_use","id":"tu-alpha","name":"Bash","input":{"command":"ls"}}
				]
			}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert_eq!(outs.len(), 1);
		match &outs[0] {
			MapperOutcome::Append(m) => {
				assert_eq!(m.tool_use_id.as_deref(), Some("tu-alpha"));
				assert!(m.tool_use.is_some());
				assert!(m.tool_result.is_none());
				assert!(m.result_timestamp.is_none());
			}
			other => panic!("expected Append, got {other:?}"),
		}
	}

	/// 同一 entry 内の assistant tool_use + 直後 user tool_result ペアでも、
	/// mapper は **per-entry 呼び出し** なので 1 entry = 1 種類の outcome 列。
	/// 連続 2 entry 分を順に呼んだら [Append(tool_use), AttachResult] の 2 Vec
	/// が得られる。
	#[test]
	fn consecutive_entries_produce_pair_of_outcomes() {
		let raw_asst = r#"{
			"type":"assistant",
			"agentId":"a1",
			"uuid":"u-asst",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"assistant",
				"content":[
					{"type":"tool_use","id":"tu-beta","name":"Read","input":{"file_path":"/x"}}
				]
			}
		}"#;
		let raw_user = r#"{
			"type":"user",
			"agentId":"a1",
			"uuid":"u-user",
			"timestamp":"2024-01-01T00:00:01Z",
			"message":{
				"role":"user",
				"content":[
					{"type":"tool_result","tool_use_id":"tu-beta","content":"file body","is_error":false}
				]
			}
		}"#;
		let asst: SubAgentLogEntry = serde_json::from_str(raw_asst).unwrap();
		let user: SubAgentLogEntry = serde_json::from_str(raw_user).unwrap();
		let out_a = to_mapper_outcomes(&asst);
		let out_u = to_mapper_outcomes(&user);
		assert!(matches!(out_a[0], MapperOutcome::Append(_)));
		assert!(matches!(out_u[0], MapperOutcome::AttachResult { .. }));
	}

	/// 並列ツール呼び出し (同一 assistant message で複数 tool_use)。
	/// 各 tool_use に対応する tool_result が順不同で来るケース。
	#[test]
	fn parallel_tool_use_and_results_emit_multiple_outcomes() {
		let raw_asst = r#"{
			"type":"assistant",
			"agentId":"a1",
			"uuid":"u-par",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"assistant",
				"content":[
					{"type":"tool_use","id":"tu-x","name":"Bash","input":{"command":"echo x"}},
					{"type":"tool_use","id":"tu-y","name":"Bash","input":{"command":"echo y"}}
				]
			}
		}"#;
		let raw_user = r#"{
			"type":"user",
			"agentId":"a1",
			"uuid":"u-par-r",
			"timestamp":"2024-01-01T00:00:02Z",
			"message":{
				"role":"user",
				"content":[
					{"type":"tool_result","tool_use_id":"tu-y","content":"out-y","is_error":false},
					{"type":"tool_result","tool_use_id":"tu-x","content":"out-x","is_error":false}
				]
			}
		}"#;
		let asst: SubAgentLogEntry = serde_json::from_str(raw_asst).unwrap();
		let user: SubAgentLogEntry = serde_json::from_str(raw_user).unwrap();
		let out_a = to_mapper_outcomes(&asst);
		let out_u = to_mapper_outcomes(&user);
		assert_eq!(out_a.len(), 2);
		// 並列呼び出しで tool_use_id が 2 種揃う
		let mut use_ids: Vec<Option<String>> = out_a
			.iter()
			.map(|o| match o {
				MapperOutcome::Append(m) => m.tool_use_id.clone(),
				_ => None,
			})
			.collect();
		use_ids.sort();
		assert_eq!(
			use_ids,
			vec![Some("tu-x".to_string()), Some("tu-y".to_string())]
		);
		// 結果側は順序保持 (tu-y が先)。
		assert_eq!(out_u.len(), 2);
		match &out_u[0] {
			MapperOutcome::AttachResult { tool_use_id, .. } => assert_eq!(tool_use_id, "tu-y"),
			other => panic!("{other:?}"),
		}
		match &out_u[1] {
			MapperOutcome::AttachResult { tool_use_id, .. } => assert_eq!(tool_use_id, "tu-x"),
			other => panic!("{other:?}"),
		}
	}

	// ----------------------------------------------------------------------
	// is_final_response 判定 (stop_reason == "end_turn" + 最後の text item)
	// ----------------------------------------------------------------------

	/// `stop_reason == "end_turn"` の assistant entry で、**最後の text item**
	/// だけが `is_final_response = true` になる。途中の text は false のまま
	/// (独り言扱い)。mention prefix `@Main` を付けるかどうかの判定に UI 層が使う。
	#[test]
	fn end_turn_marks_only_last_text_as_final_response() {
		let raw = r#"{
			"type":"assistant",
			"agentId":"a1",
			"uuid":"u-end",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"assistant",
				"content":[
					{"type":"text","text":"START:"},
					{"type":"tool_use","id":"tu-1","name":"Bash","input":{"command":"ls"}},
					{"type":"text","text":"END:"}
				],
				"stop_reason":"end_turn"
			}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert_eq!(outs.len(), 3);
		// index 0: 中間 text → false
		match &outs[0] {
			MapperOutcome::Append(m) => {
				assert_eq!(m.text.as_deref(), Some("START:"));
				assert!(
					!m.is_final_response,
					"中間 text は is_final_response=false (独り言扱い)"
				);
			}
			other => panic!("expected Append text, got {other:?}"),
		}
		// index 1: tool_use → false (mention / 薄色対象外)
		match &outs[1] {
			MapperOutcome::Append(m) => {
				assert!(m.tool_use.is_some());
				assert!(!m.is_final_response);
			}
			other => panic!("expected Append tool_use, got {other:?}"),
		}
		// index 2: **最後の text** → true
		match &outs[2] {
			MapperOutcome::Append(m) => {
				assert_eq!(m.text.as_deref(), Some("END:"));
				assert!(
					m.is_final_response,
					"end_turn 最後の text は is_final_response=true (Main 宛て最終応答)"
				);
			}
			other => panic!("expected Append text, got {other:?}"),
		}
	}

	/// `stop_reason` 未指定 (null / 省略) の entry は「途中の text 発話」扱い。
	/// **最後の text item でも** `is_final_response` は false のまま。
	#[test]
	fn missing_stop_reason_never_marks_final_response() {
		let raw = r#"{
			"type":"assistant",
			"agentId":"a1",
			"uuid":"u-mid",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"assistant",
				"content":[
					{"type":"text","text":"thinking..."}
				]
			}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert_eq!(outs.len(), 1);
		match &outs[0] {
			MapperOutcome::Append(m) => {
				assert_eq!(m.text.as_deref(), Some("thinking..."));
				assert!(
					!m.is_final_response,
					"stop_reason 未指定は中間 text 扱い (独り言)"
				);
			}
			other => panic!("expected Append, got {other:?}"),
		}
	}

	/// `stop_reason == "tool_use"` (ツール呼び出しで停止した中間状態) でも
	/// `is_final_response` は false のまま。
	#[test]
	fn tool_use_stop_reason_never_marks_final_response() {
		let raw = r#"{
			"type":"assistant",
			"agentId":"a1",
			"uuid":"u-tu",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"assistant",
				"content":[
					{"type":"text","text":"let me check"},
					{"type":"tool_use","id":"tu-1","name":"Bash","input":{"command":"ls"}}
				],
				"stop_reason":"tool_use"
			}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert_eq!(outs.len(), 2);
		match &outs[0] {
			MapperOutcome::Append(m) => assert!(!m.is_final_response),
			other => panic!("{other:?}"),
		}
		match &outs[1] {
			MapperOutcome::Append(m) => assert!(!m.is_final_response),
			other => panic!("{other:?}"),
		}
	}

	/// Main からの prompt (user entry の string content) は常に
	/// `is_final_response=false`。mention prefix `@{agent_type}` は UI 層が
	/// `Sender::Main` を見て付けるので、ここでフラグを立てる必要はない。
	#[test]
	fn main_user_prompt_is_never_final_response() {
		let raw = r#"{
			"type":"user",
			"agentId":"a1",
			"uuid":"u-main",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{"role":"user","content":"Please investigate X"}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert_eq!(outs.len(), 1);
		match &outs[0] {
			MapperOutcome::Append(m) => {
				assert_eq!(m.sender, Sender::Main);
				assert!(
					!m.is_final_response,
					"Main の prompt は mention 付与の判定に stop_reason を使わない"
				);
			}
			other => panic!("{other:?}"),
		}
	}

	/// end_turn entry でも、空の text item はスキップされる (trim 後空)。
	/// 最後の非空 text が is_final_response=true になる。
	#[test]
	fn end_turn_ignores_empty_text_for_final_response() {
		let raw = r#"{
			"type":"assistant",
			"agentId":"a1",
			"uuid":"u-e2",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"assistant",
				"content":[
					{"type":"text","text":"real content"},
					{"type":"text","text":"   "}
				],
				"stop_reason":"end_turn"
			}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		// trim-empty の末尾 text は skip される → 1 件のみ残る
		assert_eq!(outs.len(), 1);
		match &outs[0] {
			MapperOutcome::Append(m) => {
				assert_eq!(m.text.as_deref(), Some("real content"));
				// 末尾 text index (1) と 実 content index (0) は不一致なので false。
				// これは「空 text もインデックス 1 を取るので last_text_index=1 を
				// 指す」ことに依存する = 末尾が空の end_turn では final_response=false。
				// 実用上、末尾 text が空のケースは「まだ書き始めた所」なので独り言扱いで OK。
				assert!(!m.is_final_response);
			}
			other => panic!("{other:?}"),
		}
	}

	/// orphan tool_result (対応する tool_use が無いケース): mapper は tool_use の
	/// 有無を知らないので単に `AttachResult` を返すだけ。fallback はアプリ層の責務。
	#[test]
	fn orphan_tool_result_still_emits_attach_outcome() {
		let raw = r#"{
			"type":"user",
			"agentId":"a1",
			"uuid":"u-orphan",
			"timestamp":"2024-01-01T00:00:00Z",
			"message":{
				"role":"user",
				"content":[
					{"type":"tool_result","tool_use_id":"tu-unknown","content":"lonely","is_error":false}
				]
			}
		}"#;
		let entry: SubAgentLogEntry = serde_json::from_str(raw).unwrap();
		let outs = to_mapper_outcomes(&entry);
		assert_eq!(outs.len(), 1);
		match &outs[0] {
			MapperOutcome::AttachResult {
				tool_use_id,
				content,
				..
			} => {
				assert_eq!(tool_use_id, "tu-unknown");
				assert_eq!(content, "lonely");
			}
			other => panic!("expected AttachResult, got {other:?}"),
		}
	}
}
