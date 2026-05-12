//! JSONL パーサ (バイトオフセット追跡で差分読込)。
//!
//! TS 版 `src/infrastructure/parsers/JsonlParser.ts` の移植。
//!
//! 設計:
//! - `parse_new_entries<T>(path, &mut offset)` で任意の
//!   [`DeserializeOwned`] 型に汎用パース。
//! - `File::seek(SeekFrom::Start(offset))` + `BufReader::read_line` の
//!   ループで差分だけ読む。
//! - ファイルが truncate された場合 (= `metadata.len() < offset`) は
//!   offset を 0 にリセットして全量再読込。
//! - パース失敗行 (途中書き込みなど) は握りつぶして次の行へ進む。
//! - ファイルが存在しない／open に失敗するケースは空 Vec を返す
//!   (監視中にファイルが消えても panic しない)。

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use serde::de::DeserializeOwned;

/// 任意の JSONL 行を型 `T` にパースしながら差分読込する。
///
/// - `offset` は呼び出し側で保持する。戻った時点でファイルサイズに更新される。
/// - 失敗行は black-hole に捨てる (TS 版と同じ挙動)。
/// - ファイルが open できないと `Vec::new()` を返す。
pub fn parse_new_entries<T>(path: &Path, offset: &mut u64) -> Vec<T>
where
	T: DeserializeOwned,
{
	let mut entries: Vec<T> = Vec::new();

	let mut file = match File::open(path) {
		Ok(f) => f,
		Err(_) => return entries,
	};

	let file_size = match file.metadata() {
		Ok(m) => m.len(),
		Err(_) => return entries,
	};

	// ファイル truncate 検知: サイズが offset を下回っていたらリセット。
	if file_size < *offset {
		*offset = 0;
	}

	if file.seek(SeekFrom::Start(*offset)).is_err() {
		return entries;
	}

	let mut reader = BufReader::new(&mut file);
	let mut line = String::new();
	loop {
		line.clear();
		let n = match reader.read_line(&mut line) {
			Ok(0) => break,
			Ok(n) => n,
			Err(_) => break,
		};
		let _ = n;

		let trimmed = line.trim();
		if trimmed.is_empty() {
			continue;
		}

		if let Ok(entry) = serde_json::from_str::<T>(trimmed) {
			entries.push(entry);
		}
		// パース失敗は無視 (途中書き込みされた不完全な行など)
	}

	*offset = file_size;
	entries
}

/// JSONL パーサ (struct 版)。
///
/// 1 つのファイルに対して継続的に差分を読む用途向け。複数ファイルを
/// 扱う場合は呼び出し側で複数インスタンスを持つか、`parse_new_entries`
/// を直接使う。
#[derive(Debug, Default)]
pub struct JsonlParser {
	offset: u64,
}

impl JsonlParser {
	/// 新規パーサ (offset = 0)。
	pub fn new() -> Self {
		Self { offset: 0 }
	}

	/// 差分分の行を `T` にパースして返す。
	pub fn read_new_entries<T>(&mut self, path: &Path) -> Vec<T>
	where
		T: DeserializeOwned,
	{
		parse_new_entries(path, &mut self.offset)
	}

	/// offset を 0 にリセット (次回呼び出しで全件再読込)。
	pub fn reset_offset(&mut self) {
		self.offset = 0;
	}

	/// 現在の offset (テスト / デバッグ用)。
	pub fn offset(&self) -> u64 {
		self.offset
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::domain::entities::{MainLogEntry, SubAgentLogEntry};
	use std::fs::OpenOptions;
	use std::io::Write;
	use tempfile::NamedTempFile;

	fn write_all(path: &Path, content: &str) {
		let mut f = OpenOptions::new()
			.create(true)
			.write(true)
			.truncate(true)
			.open(path)
			.unwrap();
		f.write_all(content.as_bytes()).unwrap();
	}

	fn append_all(path: &Path, content: &str) {
		let mut f = OpenOptions::new().append(true).open(path).unwrap();
		f.write_all(content.as_bytes()).unwrap();
	}

	#[test]
	fn reads_all_entries_on_first_call() {
		let tmp = NamedTempFile::new().unwrap();
		let path = tmp.path();
		let lines = [
			r#"{"type":"user","agentId":"a1","uuid":"u1","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"hello"}}"#,
			r#"{"type":"assistant","agentId":"a1","uuid":"u2","timestamp":"2024-01-01T00:01:00Z","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#,
			r#"{"type":"user","agentId":"a1","uuid":"u3","timestamp":"2024-01-01T00:02:00Z","message":{"role":"user","content":"world"}}"#,
		];
		write_all(path, &format!("{}\n", lines.join("\n")));

		let mut parser = JsonlParser::new();
		let entries: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
		assert_eq!(entries.len(), 3);
		assert!(parser.offset() > 0);
	}

	#[test]
	fn reads_only_new_entries_on_second_call() {
		let tmp = NamedTempFile::new().unwrap();
		let path = tmp.path();
		let l1 = r#"{"type":"user","agentId":"a1","uuid":"u1","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"msg1"}}"#;
		let l2 = r#"{"type":"user","agentId":"a1","uuid":"u2","timestamp":"2024-01-01T00:01:00Z","message":{"role":"user","content":"msg2"}}"#;
		write_all(path, &format!("{l1}\n{l2}\n"));

		let mut parser = JsonlParser::new();
		let first: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
		assert_eq!(first.len(), 2);

		let l3 = r#"{"type":"user","agentId":"a1","uuid":"u3","timestamp":"2024-01-01T00:02:00Z","message":{"role":"user","content":"msg3"}}"#;
		append_all(path, &format!("{l3}\n"));

		let second: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
		assert_eq!(second.len(), 1);
	}

	#[test]
	fn skips_invalid_json_lines() {
		let tmp = NamedTempFile::new().unwrap();
		let path = tmp.path();
		let valid = r#"{"type":"user","agentId":"a1","uuid":"u1","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"ok"}}"#;
		let invalid = "{ not-json";
		write_all(path, &format!("{valid}\n{invalid}\n{valid}\n"));

		let mut parser = JsonlParser::new();
		let entries: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
		assert_eq!(entries.len(), 2);
	}

	#[test]
	fn resets_offset_on_file_truncate() {
		let tmp = NamedTempFile::new().unwrap();
		let path = tmp.path();
		let long = r#"{"type":"user","agentId":"a1","uuid":"u1","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
		write_all(path, &format!("{long}\n{long}\n"));

		let mut parser = JsonlParser::new();
		let _first: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
		assert_eq!(_first.len(), 2);

		let short = r#"{"type":"user","agentId":"a1","uuid":"u1","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"x"}}"#;
		write_all(path, &format!("{short}\n"));

		let entries: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
		assert_eq!(entries.len(), 1);
	}

	#[test]
	fn returns_empty_for_nonexistent_file() {
		let mut parser = JsonlParser::new();
		let entries: Vec<SubAgentLogEntry> =
			parser.read_new_entries(Path::new("/nonexistent/cc-chatter-test/missing.jsonl"));
		assert_eq!(entries.len(), 0);
	}

	#[test]
	fn returns_empty_for_empty_file() {
		let tmp = NamedTempFile::new().unwrap();
		let path = tmp.path();
		write_all(path, "");

		let mut parser = JsonlParser::new();
		let entries: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
		assert_eq!(entries.len(), 0);
	}

	#[test]
	fn handles_file_without_trailing_newline() {
		let tmp = NamedTempFile::new().unwrap();
		let path = tmp.path();
		let line = r#"{"type":"user","agentId":"a1","uuid":"u1","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"x"}}"#;
		write_all(path, line); // no trailing newline

		let mut parser = JsonlParser::new();
		let entries: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
		assert_eq!(entries.len(), 1);
	}

	#[test]
	fn reset_offset_causes_reread() {
		let tmp = NamedTempFile::new().unwrap();
		let path = tmp.path();
		let line = r#"{"type":"user","agentId":"a1","uuid":"u1","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"x"}}"#;
		write_all(path, &format!("{line}\n"));

		let mut parser = JsonlParser::new();
		let first: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
		assert_eq!(first.len(), 1);

		parser.reset_offset();
		let second: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
		assert_eq!(second.len(), 1);
	}

	#[test]
	fn parses_main_log_entries() {
		let tmp = NamedTempFile::new().unwrap();
		let path = tmp.path();
		let lines = [
			r#"{"type":"assistant","message":{"model":"m1","role":"assistant","content":[]}}"#,
			r#"{"type":"progress","data":{"type":"agent_progress","agentId":"a1"},"parentToolUseID":"t1"}"#,
		];
		write_all(path, &format!("{}\n", lines.join("\n")));

		let mut parser = JsonlParser::new();
		let entries: Vec<MainLogEntry> = parser.read_new_entries(path);
		assert_eq!(entries.len(), 2);
		assert!(matches!(entries[0], MainLogEntry::Assistant(_)));
		assert!(matches!(entries[1], MainLogEntry::Progress(_)));
	}
}
