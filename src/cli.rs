//! CLI オプション定義。
//!
//! 仕様の参照は `docs/spec.md` の「CLI」節。

use chrono::Duration;
use clap::Parser;

/// cc-chatter CLI オプション。
#[derive(Parser, Debug, Clone)]
#[command(
	name = "cc-chatter",
	version,
	about = "A real-time TUI viewer for Claude Code subagent interactions",
	long_about = None,
)]
pub struct CliOptions {
	/// 対象 workspace を絞る (指定なしはカレントディレクトリ配下を使う)
	#[arg(short = 'w', long, value_name = "PATH")]
	pub workspace: Option<String>,

	/// 最新セッションに自動アタッチ
	#[arg(long)]
	pub latest: bool,

	/// セッション ID 前方一致指定 (マッチ 1 件で自動選択、複数/0 件はエラー)
	#[arg(long, value_name = "ID")]
	pub session: Option<String>,

	/// 特定エージェントにアタッチ (`--latest` or `--session` と併用)
	#[arg(long, value_name = "AGENT_ID")]
	pub agent: Option<String>,

	/// セッション一覧を直近 N 期間以内に絞る (例: 7d / 24h / 30m)
	#[arg(
		long,
		value_name = "DURATION",
		default_value = "7d",
		value_parser = parse_since_filter,
	)]
	pub since: Duration,

	/// セッション一覧の最大表示件数 (`--since` で絞った後さらに件数で切る)
	#[arg(long, default_value_t = 50, value_name = "NUM")]
	pub limit: usize,

	/// 非表示エージェント (`aprompt_suggestion` 等) も表示する
	#[arg(long = "show-all")]
	pub show_all: bool,

	/// WATCHING 画面のマウス reporting を無効化する
	#[arg(long = "no-mouse")]
	pub no_mouse: bool,
}

impl CliOptions {
	/// `--latest` / `--session` のいずれかが指定されているか。
	///
	/// 両方指定された場合の優先順位は `--session` > `--latest`。
	pub fn wants_auto_attach(&self) -> bool {
		self.latest || self.session.is_some()
	}
}

/// `--since` の値を `chrono::Duration` に変換する。
///
/// 受理形式: `Nd` (日) / `Nh` (時間) / `Nm` (分)。`N` は 1 以上の整数。
/// 期間は相対のみ。絶対日時 (`2026-04-20` など) はサポートしない。
pub fn parse_since_filter(s: &str) -> Result<Duration, String> {
	let s = s.trim();
	if s.len() < 2 {
		return Err(format!(
			"`{s}` は期間として無効です。`Nd` / `Nh` / `Nm` 形式で指定してください (例: 7d)"
		));
	}
	let (num_str, unit) = s.split_at(s.len() - 1);
	let n: i64 = num_str.parse().map_err(|_| {
		format!("`{s}` の数値部分 `{num_str}` をパースできません。1 以上の整数を指定してください")
	})?;
	if n < 1 {
		return Err(format!("`{s}` の値は 1 以上を指定してください"));
	}
	match unit {
		"d" => Ok(Duration::days(n)),
		"h" => Ok(Duration::hours(n)),
		"m" => Ok(Duration::minutes(n)),
		other => Err(format!(
			"`{s}` の単位 `{other}` はサポートされていません。`d` / `h` / `m` を使ってください"
		)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_minimal_invocation() {
		let opts = CliOptions::try_parse_from(["cc-chatter"]).unwrap();
		assert_eq!(opts.limit, 50);
		assert_eq!(opts.since, Duration::days(7));
		assert!(!opts.latest);
		assert!(!opts.show_all);
		assert!(!opts.no_mouse);
		assert!(opts.workspace.is_none());
		assert!(opts.session.is_none());
		assert!(opts.agent.is_none());
	}

	#[test]
	fn parses_latest_with_agent() {
		let opts =
			CliOptions::try_parse_from(["cc-chatter", "--latest", "--agent", "abc-123"]).unwrap();
		assert!(opts.latest);
		assert_eq!(opts.agent.as_deref(), Some("abc-123"));
		assert!(opts.wants_auto_attach());
	}

	#[test]
	fn parses_session_prefix() {
		let opts = CliOptions::try_parse_from(["cc-chatter", "--session", "sess-xy"]).unwrap();
		assert_eq!(opts.session.as_deref(), Some("sess-xy"));
		assert!(opts.wants_auto_attach());
	}

	#[test]
	fn parses_workspace_flags() {
		let opts = CliOptions::try_parse_from([
			"cc-chatter",
			"-w",
			".",
			"--show-all",
			"--no-mouse",
			"--limit",
			"25",
		])
		.unwrap();
		assert_eq!(opts.workspace.as_deref(), Some("."));
		assert!(opts.show_all);
		assert!(opts.no_mouse);
		assert_eq!(opts.limit, 25);
	}

	#[test]
	fn no_auto_attach_without_latest_or_session() {
		let opts = CliOptions::try_parse_from(["cc-chatter", "-w", "."]).unwrap();
		assert!(!opts.wants_auto_attach());
	}

	#[test]
	fn parses_since_in_supported_units() {
		let cases = [
			("7d", Duration::days(7)),
			("1d", Duration::days(1)),
			("24h", Duration::hours(24)),
			("30m", Duration::minutes(30)),
		];
		for (input, expected) in cases {
			let opts = CliOptions::try_parse_from(["cc-chatter", "--since", input]).unwrap();
			assert_eq!(opts.since, expected, "input={input}");
		}
	}

	#[test]
	fn rejects_invalid_since_values() {
		let invalids = [
			"", "d", "h", "0d", "-3d", "7", "7w", "7days", "1.5d", "abc", "7 d",
		];
		for input in invalids {
			let res = CliOptions::try_parse_from(["cc-chatter", "--since", input]);
			assert!(res.is_err(), "expected error for input={input:?}");
		}
	}
}
