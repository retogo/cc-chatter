//! フォーマット系ユーティリティ。TS 版 `src/ui/utils/format.ts` の最小移植。
//!
//! M2 スコープでは描画に必要な最小セットだけ実装する:
//! - `format_time`: `HH:MM:SS`
//! - `format_tool_preview`: ツール呼び出しのプレビュー (1 行)
//! - `format_result_preview`: ツール結果のプレビュー (1 行)

use chrono::{DateTime, Local, Utc};
use serde_json::Value;

/// UTC 時刻をローカルタイムゾーンの `HH:MM:SS` で整形する。
pub fn format_time(ts: &DateTime<Utc>) -> String {
	format_date_time(ts, "%H:%M:%S")
}

/// UTC 時刻をローカルタイムゾーンに変換してから任意の strftime 書式で整形する。
///
/// `DateTime<Utc>` に直接 `.format()` を呼ぶと UTC のまま整形されてしまうため、
/// システムローカル時刻で表示したい箇所はこのヘルパー経由で統一する。
pub fn format_date_time(ts: &DateTime<Utc>, fmt: &str) -> String {
	let local: DateTime<Local> = (*ts).into();
	local.format(fmt).to_string()
}

/// `key="value"` / `key=value` 形式で短く整形する (TS 版 `formatToolInput` 相当)。
pub fn format_tool_input(input: &Value, max_len: usize) -> String {
	let Some(obj) = input.as_object() else {
		return String::new();
	};
	let mut parts: Vec<String> = Vec::new();
	for (key, value) in obj {
		match value {
			Value::String(s) => {
				let display = if s.chars().count() > max_len {
					format!("{}...", take_chars(s, max_len))
				} else {
					s.clone()
				};
				parts.push(format!("{key}=\"{display}\""));
			}
			Value::Number(n) => parts.push(format!("{key}={n}")),
			Value::Bool(b) => parts.push(format!("{key}={b}")),
			_ => {}
		}
	}
	parts.join(" ")
}

/// ツール呼び出しのプレビュー (通常モード 2 行目)。
///
/// `Bash` / `Read` 系 / `Grep` 系 / `Web*` / `Task`/`Agent` はツール特化の
/// 整形、それ以外は `format_tool_input` の汎用整形。
pub fn format_tool_preview(name: &str, input: &Value) -> String {
	const MAX: usize = 80;

	match name {
		"Bash" => {
			let Some(cmd) = input.get("command").and_then(Value::as_str) else {
				return String::new();
			};
			let first_line = cmd.split('\n').next().unwrap_or("");
			truncate_tail(first_line, MAX)
		}
		"Read" | "Write" | "Edit" | "NotebookEdit" => {
			let Some(path) = input.get("file_path").and_then(Value::as_str) else {
				return String::new();
			};
			truncate_head(path, MAX)
		}
		"Grep" | "Glob" => {
			let Some(pattern) = input.get("pattern").and_then(Value::as_str) else {
				return String::new();
			};
			truncate_tail(pattern, MAX)
		}
		"WebFetch" | "WebSearch" => {
			let value = input
				.get("url")
				.and_then(Value::as_str)
				.or_else(|| input.get("query").and_then(Value::as_str))
				.unwrap_or("");
			if value.is_empty() {
				return String::new();
			}
			truncate_tail(value, MAX)
		}
		"Task" | "Agent" => {
			let subagent_type = input.get("subagent_type").and_then(Value::as_str);
			let description = input
				.get("description")
				.and_then(Value::as_str)
				.unwrap_or("");
			let base = match subagent_type {
				Some(t) if !t.is_empty() => format!("[{t}] {description}").trim_end().to_string(),
				_ => description.to_string(),
			};
			if base.is_empty() {
				return String::new();
			}
			truncate_tail(&base, MAX)
		}
		_ => {
			let formatted = format_tool_input(input, 30);
			if formatted.is_empty() {
				return String::new();
			}
			truncate_tail(&formatted, MAX)
		}
	}
}

/// ツール結果の 1 行プレビュー (最初の非空行を先頭 100 文字に詰める)。
pub fn format_result_preview(content: &str) -> String {
	const MAX: usize = 100;
	for line in content.split('\n') {
		let trimmed = line.trim();
		if !trimmed.is_empty() {
			return truncate_tail(trimmed, MAX);
		}
	}
	"(empty)".to_string()
}

fn truncate_tail(value: &str, max_len: usize) -> String {
	if value.chars().count() <= max_len {
		return value.to_string();
	}
	format!("{}...", take_chars(value, max_len))
}

fn truncate_head(value: &str, max_len: usize) -> String {
	if value.chars().count() <= max_len {
		return value.to_string();
	}
	let total = value.chars().count();
	let tail: String = value.chars().skip(total - max_len).collect();
	format!("...{tail}")
}

fn take_chars(s: &str, n: usize) -> String {
	s.chars().take(n).collect()
}

pub fn git_bash_preview(command: &str) -> Option<&'static str> {
	let mut tokens = command.split_whitespace();
	let mut last_label = None;
	while let Some(token) = tokens.next() {
		match token {
			"cd" => {
				let _ = tokens.next();
			}
			"&&" | ";" | "||" | "|" => {}
			"git" => {
				if let Some(label) = git_subcommand_label(&mut tokens) {
					last_label = Some(label);
				}
			}
			_ => {}
		}
	}
	last_label
}

fn git_subcommand_label<'a, I>(tokens: &mut I) -> Option<&'static str>
where
	I: Iterator<Item = &'a str>,
{
	while let Some(token) = tokens.next() {
		if token == "-C" {
			let _ = tokens.next();
			continue;
		}
		if token.starts_with('-') {
			continue;
		}
		return match token {
			"add" | "stage" => Some("Stage"),
			"commit" => Some("Commit"),
			"push" => Some("Push"),
			"pull" => Some("Pull"),
			"merge" => Some("Merge"),
			"rebase" => Some("Rebase"),
			"checkout" | "switch" | "restore" => Some("Checkout"),
			"log" => Some("Git Log"),
			_ => None,
		};
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn format_time_produces_hh_mm_ss() {
		let ts = "2024-01-01T12:34:56Z".parse::<DateTime<Utc>>().unwrap();
		let s = format_time(&ts);
		// timezone 依存なので長さだけ確認
		assert_eq!(s.len(), 8);
		assert_eq!(s.matches(':').count(), 2);
	}

	#[test]
	fn format_date_time_matches_local_timezone() {
		let ts = "2024-06-15T03:00:00Z".parse::<DateTime<Utc>>().unwrap();
		let expected: DateTime<Local> = ts.into();
		assert_eq!(
			format_date_time(&ts, "%Y-%m-%d %H:%M"),
			expected.format("%Y-%m-%d %H:%M").to_string()
		);
	}

	#[test]
	fn bash_preview_takes_first_line() {
		let input = json!({"command": "echo hi\nnext line"});
		assert_eq!(format_tool_preview("Bash", &input), "echo hi");
	}

	#[test]
	fn git_bash_preview_prefers_later_commit_over_earlier_add() {
		assert_eq!(
			git_bash_preview("git add . && git commit -m test"),
			Some("Commit")
		);
	}

	#[test]
	fn read_preview_uses_file_path_with_head_truncation() {
		let input = json!({"file_path": "/tmp/x.txt"});
		assert_eq!(format_tool_preview("Read", &input), "/tmp/x.txt");
	}

	#[test]
	fn task_preview_includes_subagent_type_bracket() {
		let input = json!({"subagent_type": "Explore", "description": "do stuff"});
		assert_eq!(format_tool_preview("Task", &input), "[Explore] do stuff");
	}

	#[test]
	fn task_preview_without_subagent_type_is_description_only() {
		let input = json!({"description": "only"});
		assert_eq!(format_tool_preview("Agent", &input), "only");
	}

	#[test]
	fn result_preview_returns_first_nonempty_line() {
		assert_eq!(format_result_preview("\n\nhello\nworld"), "hello");
		assert_eq!(format_result_preview(""), "(empty)");
	}

	#[test]
	fn truncate_tail_appends_ellipsis() {
		let s = "a".repeat(200);
		let out = truncate_tail(&s, 80);
		assert!(out.ends_with("..."));
		assert_eq!(out.chars().count(), 83);
	}

	#[test]
	fn truncate_head_prepends_ellipsis() {
		let s = "a".repeat(200);
		let out = truncate_head(&s, 80);
		assert!(out.starts_with("..."));
		assert_eq!(out.chars().count(), 83);
	}
}
