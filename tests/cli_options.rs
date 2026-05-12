//! CLI オプションの integration test。
//!
//! 目的: `clap` の parse 結果が repository 層と結びついたときに spec どおり
//! ふるまうかを end-to-end で確認する。unit test (`src/cli.rs`) が parse 値
//! のみを検証するのに対し、こちらは `FileSystemSessionRepository` と組み合わせ
//! て「値が実際に効くか」まで見る。
//!
//! fixture は TS 版の workspace 命名規則 (`/`, `.`, `_` → `-`) を再現した
//! tempdir を作り、`GetAllSessionsOptions.claude_projects_dir` / `cwd` override
//! を使って `$HOME/.claude/projects` を読まずにテストする。

use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::Path;

use clap::Parser;
use tempfile::tempdir;

use cc_chatter::cli::CliOptions;
use cc_chatter::domain::entities::{SessionEntity, SessionMetadata};
use cc_chatter::infrastructure::repositories::{
	FileSystemSessionRepository, GetAllSessionsOptions,
};

fn write_file(path: &Path, content: &str) {
	let mut f = File::create(path).unwrap();
	f.write_all(content.as_bytes()).unwrap();
}

/// workspace 命名規則を再現した projects ディレクトリを作る。
///
/// `#[allow(dead_code)]`: 各テストが使うフィールドが異なるので、特定テスト
/// だけから見ると未使用に見える。fixture ヘルパーとしては全フィールド必要。
#[allow(dead_code)]
struct Workspace {
	projects_dir: std::path::PathBuf,
	fake_cwd: std::path::PathBuf,
	ws_dir: std::path::PathBuf,
	encoded: String,
}

fn setup_workspace(root: &Path, proj_name: &str) -> Workspace {
	let projects_dir = root.join("projects");
	let fake_cwd = root.join(proj_name);
	create_dir_all(&fake_cwd).unwrap();
	let canonical_cwd = std::fs::canonicalize(&fake_cwd).unwrap();
	let encoded: String = canonical_cwd
		.to_string_lossy()
		.chars()
		.map(|c| match c {
			'/' | '.' | '_' => '-',
			other => other,
		})
		.collect();
	let ws_dir = projects_dir.join(&encoded);
	create_dir_all(&ws_dir).unwrap();
	Workspace {
		projects_dir,
		fake_cwd,
		ws_dir,
		encoded,
	}
}

fn write_session(ws_dir: &Path, session_id: &str, timestamp: &str) {
	let path = ws_dir.join(format!("{session_id}.jsonl"));
	let user_line = format!(
		r#"{{"type":"user","uuid":"u1","timestamp":"{timestamp}","message":{{"role":"user","content":"prompt {session_id}"}}}}"#,
	);
	let summary_line = format!(r#"{{"type":"summary","summary":"sess {session_id}"}}"#);
	write_file(&path, &format!("{user_line}\n{summary_line}\n"));
}

// ---------------------------------------------------------------------------
// --limit
// ---------------------------------------------------------------------------

#[test]
fn limit_flag_caps_session_list() {
	let root = tempdir().unwrap();
	let root_canon = std::fs::canonicalize(root.path()).unwrap();
	let ws = setup_workspace(&root_canon, "proj-limit");

	for i in 0..8 {
		// ISO 月を分けて updated_at を別々に設定する
		write_session(
			&ws.ws_dir,
			&format!("sess-{i}"),
			&format!("2024-0{}-01T00:00:00Z", i + 1),
		);
	}

	// CLI 解析 → repository が limit を尊重することを確認
	let cli = CliOptions::try_parse_from(["cc-chatter", "--limit", "5"]).unwrap();
	assert_eq!(cli.limit, 5, "clap が --limit を正しく parse する");

	let repo = FileSystemSessionRepository::new();
	let sessions = repo.get_all_sessions(
		None,
		&GetAllSessionsOptions {
			limit: cli.limit,
			claude_projects_dir: Some(ws.projects_dir),
			cwd: Some(ws.fake_cwd),
			..GetAllSessionsOptions::default()
		},
	);

	assert_eq!(sessions.len(), 5, "limit=5 なら 5 件に絞られる");
	// updatedAt 降順の先頭が最新 (sess-7 → 08 月)
	assert_eq!(sessions[0].session_id, "sess-7");
}

#[test]
fn limit_default_is_fifty() {
	// CLI の default (50) を他のテストで間違えて上書きしないよう守る
	let cli = CliOptions::try_parse_from(["cc-chatter"]).unwrap();
	assert_eq!(cli.limit, 50);
}

#[test]
fn since_default_is_seven_days() {
	let cli = CliOptions::try_parse_from(["cc-chatter"]).unwrap();
	assert_eq!(cli.since, chrono::Duration::days(7));
}

// ---------------------------------------------------------------------------
// --show-all
// ---------------------------------------------------------------------------

#[test]
fn show_all_flag_exposes_hidden_agents() {
	let root = tempdir().unwrap();
	let root_canon = std::fs::canonicalize(root.path()).unwrap();
	let ws = setup_workspace(&root_canon, "proj-show-all");

	let session_id = "sess-show";
	let main_log = ws.ws_dir.join(format!("{session_id}.jsonl"));
	// Agent tool_use → progress で visible1 を作る。aprompt_suggestion_x は
	// mapping なしで HIDDEN_AGENT_PREFIXES にマッチする。
	let main_lines = [
		r#"{"type":"assistant","message":{"model":"m","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Agent","input":{"subagent_type":"Explore","description":"d","prompt":"p"}}]}}"#,
		r#"{"type":"progress","data":{"type":"agent_progress","agentId":"visible1"},"parentToolUseID":"t1"}"#,
	];
	write_file(&main_log, &format!("{}\n", main_lines.join("\n")));

	let subagents_dir = ws.ws_dir.join(session_id).join("subagents");
	create_dir_all(&subagents_dir).unwrap();
	write_file(
		&subagents_dir.join("agent-visible1.jsonl"),
		"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
	);
	write_file(
		&subagents_dir.join("agent-aprompt_suggestion_xyz.jsonl"),
		"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"x\"}}\n",
	);

	let session = SessionEntity {
		session_id: session_id.to_string(),
		workspace: ws.encoded.clone(),
		file_path: main_log,
		updated_at: chrono::Utc::now(),
		subagents_dir,
		subagent_count: 0,
		metadata: SessionMetadata::default(),
	};

	let repo = FileSystemSessionRepository::new();

	// 1. `--show-all` 無し: HIDDEN プレフィックスは除外
	let cli_default = CliOptions::try_parse_from(["cc-chatter"]).unwrap();
	assert!(!cli_default.show_all);
	let default = repo.get_sub_agents(&session, cli_default.show_all);
	assert_eq!(default.len(), 1);
	assert_eq!(default[0].agent_id, "visible1");

	// 2. `--show-all`: HIDDEN も出る
	let cli_all = CliOptions::try_parse_from(["cc-chatter", "--show-all"]).unwrap();
	assert!(cli_all.show_all);
	let all = repo.get_sub_agents(&session, cli_all.show_all);
	assert_eq!(all.len(), 2);
	assert!(all
		.iter()
		.any(|a| a.agent_id.starts_with("aprompt_suggestion")));
}

// ---------------------------------------------------------------------------
// -w / --workspace (パスエンコード)
// ---------------------------------------------------------------------------

#[test]
fn workspace_flag_drives_session_lookup_with_path_encoding() {
	// `-w <path>` で指定したパスを canonicalize → `/`, `.`, `_` → `-` の
	// エンコードで projects/<encoded>/ に解決することを end-to-end で確認する。
	// カレントディレクトリ無関係に動くことも同時にカバーする。
	let root = tempdir().unwrap();
	let root_canon = std::fs::canonicalize(root.path()).unwrap();

	// 意図的に `.` と `_` を含むプロジェクト名でパスエンコードを効かせる
	let ws = setup_workspace(&root_canon, "my.proj_x");
	write_session(&ws.ws_dir, "sess-ws", "2024-01-01T00:00:00Z");

	// 別のパス (cwd) で起動したつもりだが `-w <target>` で target に解決される
	let other_cwd = root_canon.join("unrelated");
	create_dir_all(&other_cwd).unwrap();
	let canonical_target = std::fs::canonicalize(&ws.fake_cwd).unwrap();

	let cli = CliOptions::try_parse_from(["cc-chatter", "-w", canonical_target.to_str().unwrap()])
		.unwrap();
	assert_eq!(
		cli.workspace.as_deref(),
		Some(canonical_target.to_str().unwrap())
	);

	let repo = FileSystemSessionRepository::new();
	let sessions = repo.get_all_sessions(
		cli.workspace.as_deref(),
		&GetAllSessionsOptions {
			limit: cli.limit,
			claude_projects_dir: Some(ws.projects_dir.clone()),
			cwd: Some(other_cwd.clone()),
			..GetAllSessionsOptions::default()
		},
	);

	assert_eq!(sessions.len(), 1, "-w で target ws のセッションが見える");
	assert_eq!(sessions[0].session_id, "sess-ws");
	assert_eq!(sessions[0].workspace, ws.encoded);

	// 対照実験: `-w` なしだと unrelated cwd のエンコードでは何も見えない
	let cli_no_ws = CliOptions::try_parse_from(["cc-chatter"]).unwrap();
	let sessions_default = repo.get_all_sessions(
		cli_no_ws.workspace.as_deref(),
		&GetAllSessionsOptions {
			limit: cli_no_ws.limit,
			claude_projects_dir: Some(ws.projects_dir),
			cwd: Some(other_cwd),
			..GetAllSessionsOptions::default()
		},
	);
	assert!(
		sessions_default.is_empty(),
		"-w 無し + cwd=unrelated では target ws のセッションは見えない"
	);
}

// ---------------------------------------------------------------------------
// --no-mouse
// ---------------------------------------------------------------------------

#[test]
fn no_mouse_flag_propagates_to_cli_struct() {
	// `--no-mouse` は TerminalGuard の options に渡る想定だが、TerminalGuard
	// を test 内で enter すると tty 要件があるので、ここでは CLI parse の
	// 事実のみを確認する。CLI → TerminalGuardOptions の結線は main.rs の
	// code review 範囲。
	let cli_default = CliOptions::try_parse_from(["cc-chatter"]).unwrap();
	assert!(!cli_default.no_mouse, "デフォルトは mouse 有効");

	let cli_no = CliOptions::try_parse_from(["cc-chatter", "--no-mouse"]).unwrap();
	assert!(cli_no.no_mouse, "--no-mouse で true になる");

	// 他のフラグとの組み合わせ
	let cli_combo =
		CliOptions::try_parse_from(["cc-chatter", "--latest", "--no-mouse", "--show-all"]).unwrap();
	assert!(cli_combo.no_mouse);
	assert!(cli_combo.latest);
	assert!(cli_combo.show_all);
}
