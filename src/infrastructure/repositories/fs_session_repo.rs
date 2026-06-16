//! FileSystemSessionRepository。
//! TS 版 `src/infrastructure/repositories/FileSystemSessionRepository.ts` の移植。
//!
//! `~/.claude/projects/<encoded-workspace>/*.jsonl` を走査し、部分読みと
//! mtime プレソートでセッション一覧を構築する 4-phase ロード。

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::domain::constants::{
	CLAUDE_PROJECTS_DIR, HEAD_READ_BYTES, HIDDEN_AGENT_PREFIXES, LIMIT_MIN_CANDIDATES,
	LIMIT_SAFETY_MULTIPLIER, MAPPER_INITIAL_TAIL_BYTES, TAIL_READ_BYTES,
};
use crate::domain::entities::{AgentEntity, SessionEntity, SessionMetadata};
use crate::domain::services::is_explicit_user_action;

use super::agent_mapper_impl::AgentMapperImpl;

// ---------------------------------------------------------------------------
// 公開 API
// ---------------------------------------------------------------------------

/// ファイルシステムから Claude Code セッションを列挙するリポジトリ。
#[derive(Debug, Default)]
pub struct FileSystemSessionRepository;

/// `get_all_sessions` の取得オプション。
#[derive(Debug, Clone)]
pub struct GetAllSessionsOptions {
	/// 最大返却件数 (デフォルト: 50)。
	pub limit: usize,
	/// `updatedAt` がこの期間より古いセッションを除外する。`None` で全期間。
	pub since: Option<Duration>,
	/// `SessionEntity.subagent_count` を計算するときに HIDDEN プレフィックスを
	/// 含めるかどうか。`--show-all` の状態と一致させる。
	pub show_all: bool,
	/// `~/.claude/projects` を上書きしたい場合に指定 (テスト用)。
	pub claude_projects_dir: Option<PathBuf>,
	/// cwd を上書きしたい場合に指定 (テスト用)。
	pub cwd: Option<PathBuf>,
	/// `since` の基準時刻を上書きしたい場合に指定 (テスト用)。
	/// 通常は `Utc::now()` を使う。
	pub now: Option<DateTime<Utc>>,
}

impl Default for GetAllSessionsOptions {
	fn default() -> Self {
		Self {
			limit: 50,
			since: None,
			show_all: false,
			claude_projects_dir: None,
			cwd: None,
			now: None,
		}
	}
}

impl FileSystemSessionRepository {
	pub fn new() -> Self {
		Self
	}

	/// 全セッションを updatedAt 降順で取得する。
	///
	/// `workspace_filter` が `None` のときは `cwd` を自動選択する。
	pub fn get_all_sessions(
		&self,
		workspace_filter: Option<&str>,
		options: &GetAllSessionsOptions,
	) -> Vec<SessionEntity> {
		let claude_projects_dir = options
			.claude_projects_dir
			.clone()
			.unwrap_or_else(|| PathBuf::from(resolve_tilde(CLAUDE_PROJECTS_DIR)));
		let cwd = options
			.cwd
			.clone()
			.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
		let target_workspace = normalize_workspace_filter(workspace_filter, &cwd);
		let limit = options.limit.max(1);
		let cutoff: Option<DateTime<Utc>> = options.since.map(|since| {
			let now = options.now.unwrap_or_else(Utc::now);
			now - since
		});

		// ---------------------------------------------------------------
		// Phase 1: workspace ディレクトリ走査 & 候補収集
		// ---------------------------------------------------------------
		let Ok(workspaces) = fs::read_dir(&claude_projects_dir) else {
			return Vec::new();
		};

		let mut candidates: Vec<SessionCandidate> = Vec::new();
		for ws_entry in workspaces.flatten() {
			let ws_name = match ws_entry.file_name().into_string() {
				Ok(s) => s,
				Err(_) => continue,
			};
			if ws_name != target_workspace && !ws_name.starts_with(&format!("{target_workspace}-"))
			{
				continue;
			}
			let ws_path = ws_entry.path();
			let Ok(ws_meta) = ws_entry.metadata() else {
				continue;
			};
			if !ws_meta.is_dir() {
				continue;
			}

			let Ok(files) = fs::read_dir(&ws_path) else {
				continue;
			};
			for file_entry in files.flatten() {
				let file_name = match file_entry.file_name().into_string() {
					Ok(s) => s,
					Err(_) => continue,
				};
				if !file_name.ends_with(".jsonl") {
					continue;
				}
				let file_path = file_entry.path();
				let Ok(file_meta) = file_entry.metadata() else {
					continue;
				};
				if !file_meta.is_file() {
					continue;
				}
				let session_id = file_name.trim_end_matches(".jsonl").to_string();
				let subagents_dir = ws_path.join(&session_id).join("subagents");
				let mtime = systemtime_to_utc(file_meta.modified().ok());
				candidates.push(SessionCandidate {
					session_id,
					workspace: ws_name.clone(),
					file_path,
					subagents_dir,
					mtime,
					file_size: file_meta.len(),
				});
			}
		}

		if candidates.is_empty() {
			return Vec::new();
		}

		// ---------------------------------------------------------------
		// Phase 2: mtime 降順ソート & 候補数の絞り込み
		// ---------------------------------------------------------------
		candidates.sort_by(|a, b| b.mtime.cmp(&a.mtime));
		// since 指定時は mtime cutoff で粗い段階フィルタ。mtime は updated_at の
		// 近似値で、cutoff 以前なら updated_at も確実に cutoff 以前 (last_user_action_at
		// 更新時に mtime も同時に進むため)。Phase 3 の metadata 読込を減らせる。
		if let Some(cutoff) = cutoff {
			candidates.retain(|c| c.mtime >= cutoff);
		}
		let safe_candidate_count = ((limit as f64) * LIMIT_SAFETY_MULTIPLIER).ceil() as usize;
		let candidate_count = candidates
			.len()
			.min(safe_candidate_count.max(LIMIT_MIN_CANDIDATES));
		candidates.truncate(candidate_count);

		// ---------------------------------------------------------------
		// Phase 3: metadata 部分読込 + subagent カウント
		// ---------------------------------------------------------------
		let show_all = options.show_all;
		let mut sessions: Vec<SessionEntity> = candidates
			.into_iter()
			.map(|c| {
				let metadata = get_session_metadata(&c.file_path, c.file_size);
				let updated_at = metadata.last_user_action_at.unwrap_or(c.mtime);
				let subagent_count = count_subagents(&c.subagents_dir, show_all);
				SessionEntity {
					session_id: c.session_id,
					workspace: c.workspace,
					file_path: c.file_path,
					updated_at,
					subagents_dir: c.subagents_dir,
					subagent_count,
					metadata,
				}
			})
			.collect();

		// ---------------------------------------------------------------
		// Phase 4: 最終ソート & since/limit による絞り込み
		// ---------------------------------------------------------------
		sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
		if let Some(cutoff) = cutoff {
			sessions.retain(|s| s.updated_at >= cutoff);
		}
		sessions.truncate(limit);
		sessions
	}

	/// 指定セッション配下のサブエージェント一覧。
	///
	/// `show_all=false` のとき `HIDDEN_AGENT_PREFIXES` のいずれかで始まる
	/// `agent_id` は除外する。更新日時降順にソート。
	///
	/// ## マッピング解決の優先順位
	///
	/// 1. **`agent-<id>.meta.json` サイドカー** (現行 Claude Code の公式ソース)。
	///    `{ "agentType": "...", "description"?: "..." }` を含む
	/// 2. **メインログ** の Agent/Task tool_use → progress / async tool_result の
	///    後方互換経路 (旧バージョン向け)
	/// 3. どちらも引けなければ `"unknown"`
	pub fn get_sub_agents(&self, session: &SessionEntity, show_all: bool) -> Vec<AgentEntity> {
		let refs = collect_agent_file_refs(&session.subagents_dir);
		if refs.is_empty() {
			return Vec::new();
		}

		// meta.json で解決できなかった通常 agent だけ mapper に任せる (旧ログ互換)。
		// 未解決が無ければ mapper 構築自体スキップできるので、ここでは lazy にする
		let mut needs_mapper_fallback = false;
		let mut agents: Vec<AgentEntity> = Vec::with_capacity(refs.len());
		for r in &refs {
			let Ok(meta) = fs::metadata(&r.jsonl_path) else {
				continue;
			};
			// サイドカーは各 agent の自ディレクトリ基準で解決する
			// (workflow agent は subagents/workflows/<run>/ が自ディレクトリ)
			let sidecar_type = read_meta_sidecar_agent_type(&r.meta_dir, &r.agent_id);
			let agent_type = match sidecar_type {
				Some(t) => t,
				None if r.workflow_run.is_some() => {
					// workflow agent はサイドカーが権威ソース。欠落時は generic 既定に
					// する (mapper はメインログ基準で workflow agent を知らない)
					WORKFLOW_SUBAGENT_TYPE.to_string()
				}
				None => {
					needs_mapper_fallback = true;
					String::new() // 後でまとめて埋める
				}
			};
			// generic な workflow-subagent だけ prompt 先頭からロール label を導出する
			// (カスタム型は型名がそのまま label になるので不要)
			let workflow_label = if r.workflow_run.is_some() && agent_type == WORKFLOW_SUBAGENT_TYPE
			{
				read_first_prompt_head(&r.jsonl_path).and_then(|h| derive_workflow_label(&h))
			} else {
				None
			};
			agents.push(AgentEntity {
				agent_id: r.agent_id.clone(),
				agent_type,
				output_path: r.jsonl_path.clone(),
				updated_at: systemtime_to_utc(meta.modified().ok()),
				workflow_run: r.workflow_run.clone(),
				workflow_label,
			});
		}

		if needs_mapper_fallback {
			let mapper = build_agent_mapper(&session.file_path, refs.len()).ok();
			for a in agents.iter_mut() {
				if !a.agent_type.is_empty() {
					continue;
				}
				a.agent_type = mapper
					.as_ref()
					.and_then(|m| m.get_mapping(&a.agent_id))
					.map(|d| d.agent_type.clone())
					.unwrap_or_else(|| "unknown".to_string());
			}
		}

		if !show_all {
			agents.retain(|a| {
				!HIDDEN_AGENT_PREFIXES
					.iter()
					.any(|p| a.agent_id.starts_with(p))
			});
		}

		agents.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
		agents
	}
}

// ---------------------------------------------------------------------------
// 内部ヘルパー (パス / tilde / workspace エンコード)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct SessionCandidate {
	session_id: String,
	workspace: String,
	file_path: PathBuf,
	subagents_dir: PathBuf,
	mtime: DateTime<Utc>,
	file_size: u64,
}

/// `~` をホームディレクトリに展開する。展開できなかった場合は入力を返す。
fn resolve_tilde(path: &str) -> String {
	shellexpand::tilde(path).into_owned()
}

/// 絶対パスを Claude Code のワークスペース命名規則 (`/`, `.`, `_` → `-`) に変換する。
pub(crate) fn encode_workspace_path(absolute: &Path) -> String {
	let s = absolute.to_string_lossy();
	s.chars()
		.map(|c| match c {
			'/' | '.' | '_' => '-',
			other => other,
		})
		.collect()
}

fn normalize_workspace_filter(filter: Option<&str>, cwd: &Path) -> String {
	let target: PathBuf = match filter {
		Some(f) if !f.is_empty() => {
			let expanded = resolve_tilde(f);
			let p = PathBuf::from(expanded);
			if p.is_absolute() {
				p
			} else {
				cwd.join(p)
			}
		}
		_ => cwd.to_path_buf(),
	};
	let canonical = fs::canonicalize(&target).unwrap_or(target);
	encode_workspace_path(&canonical)
}

fn systemtime_to_utc(t: Option<SystemTime>) -> DateTime<Utc> {
	t.map(DateTime::<Utc>::from).unwrap_or_else(Utc::now)
}

// ---------------------------------------------------------------------------
// メタデータ取得 (先頭 + 末尾の部分読込)
// ---------------------------------------------------------------------------

fn get_session_metadata(path: &Path, file_size: u64) -> SessionMetadata {
	let mut result = SessionMetadata {
		first_prompt: "(新規セッション)".to_string(),
		summary: None,
		last_user_action_at: None,
	};

	// --- 先頭読込: firstPrompt --------------------------------------------------
	let head_bytes = HEAD_READ_BYTES.min(file_size);
	if head_bytes > 0 {
		if let Some(content) = read_partial(path, 0, head_bytes) {
			for line in content.lines() {
				if line.trim().is_empty() {
					continue;
				}
				let Ok(value) = serde_json::from_str::<Value>(line) else {
					continue;
				};
				if value.get("type").and_then(Value::as_str) != Some("user") {
					continue;
				}
				let Some(content_str) = value
					.get("message")
					.and_then(|m| m.get("content"))
					.and_then(Value::as_str)
				else {
					continue;
				};
				if !is_explicit_user_action(content_str) {
					continue;
				}
				let collapsed = collapse_whitespace(content_str);
				let trimmed = collapsed.trim();
				let snippet: String = trimmed.chars().take(60).collect();
				result.first_prompt = snippet;
				break;
			}
		}
	}

	// --- 末尾読込: summary + lastUserActionAt ----------------------------------
	let tail_start = file_size.saturating_sub(TAIL_READ_BYTES);
	let tail_len = file_size - tail_start;
	if tail_len > 0 {
		if let Some(content) = read_partial(path, tail_start, tail_len) {
			let lines: Vec<&str> = if tail_start > 0 {
				// 最初の改行までは UTF-8 境界で欠ける恐れがあるので捨てる
				match content.find('\n') {
					Some(nl) => content[(nl + 1)..].lines().collect(),
					None => Vec::new(),
				}
			} else {
				content.lines().collect()
			};

			let mut found_summary = false;
			let mut found_last_action = false;
			for line in lines.iter().rev() {
				if found_summary && found_last_action {
					break;
				}
				if line.trim().is_empty() {
					continue;
				}
				let Ok(value) = serde_json::from_str::<Value>(line) else {
					continue;
				};
				let ty = value.get("type").and_then(Value::as_str);

				if !found_summary && ty == Some("summary") {
					if let Some(summary) = value.get("summary").and_then(Value::as_str) {
						if !summary.is_empty() {
							result.summary = Some(summary.to_string());
							found_summary = true;
						}
					}
				}

				if !found_last_action && ty == Some("user") {
					let content_str = value
						.get("message")
						.and_then(|m| m.get("content"))
						.and_then(Value::as_str);
					let ts_str = value.get("timestamp").and_then(Value::as_str);
					if let (Some(content_str), Some(ts_str)) = (content_str, ts_str) {
						if is_explicit_user_action(content_str) {
							if let Ok(ts) = ts_str.parse::<DateTime<Utc>>() {
								result.last_user_action_at = Some(ts);
								found_last_action = true;
							}
						}
					}
				}
			}
		}
	}

	result
}

/// `subagents_dir` 配下の `agent-*.jsonl` ファイル数を数える。
/// `subagents/workflows/<wf_runId>/agent-*.jsonl` (Workflow ツール経由) も含む。
/// `show_all=false` のときは `HIDDEN_AGENT_PREFIXES` にマッチする `agent_id` を除く。
/// ディレクトリが存在しない / 読めない場合は 0 を返す。
fn count_subagents(subagents_dir: &Path, show_all: bool) -> usize {
	collect_agent_file_refs(subagents_dir)
		.into_iter()
		.filter(|r| show_all || !is_hidden_agent_id(&r.agent_id))
		.count()
}

const WORKFLOW_SUBAGENT_TYPE: &str = "workflow-subagent";

/// workflow agent のロール label の最大表示文字数 (超過分は `…` で省略)。
const WORKFLOW_LABEL_MAX_CHARS: usize = 30;

/// `agent_id` が非表示プレフィックス (`HIDDEN_AGENT_PREFIXES`) のいずれかで
/// 始まるか。
fn is_hidden_agent_id(agent_id: &str) -> bool {
	HIDDEN_AGENT_PREFIXES
		.iter()
		.any(|p| agent_id.starts_with(p))
}

/// 検出した agent ログ 1 件の参照情報。
struct AgentFileRef {
	agent_id: String,
	jsonl_path: PathBuf,
	/// `agent-<id>.meta.json` を探すディレクトリ (agent の自ディレクトリ)。
	meta_dir: PathBuf,
	/// Workflow ツール経由なら run id (`wf_` プレフィックス除去済み)。通常は None。
	workflow_run: Option<String>,
}

/// `subagents/` 直下の `agent-*.jsonl` と、Workflow ツール経由の
/// `subagents/workflows/<wf_runId>/agent-*.jsonl` を 1 階層 walk して集める。
fn collect_agent_file_refs(subagents_dir: &Path) -> Vec<AgentFileRef> {
	let mut refs = Vec::new();

	// 通常のフラットなサブエージェント
	if let Ok(entries) = fs::read_dir(subagents_dir) {
		for entry in entries.flatten() {
			let Ok(name) = entry.file_name().into_string() else {
				continue;
			};
			if let Some(agent_id) = agent_id_from_jsonl(&name) {
				refs.push(AgentFileRef {
					agent_id,
					jsonl_path: entry.path(),
					meta_dir: subagents_dir.to_path_buf(),
					workflow_run: None,
				});
			}
		}
	}

	// Workflow ツール経由の agent は 1 階層深い workflows/<wf_runId>/ に出る
	let workflows_dir = subagents_dir.join("workflows");
	if let Ok(runs) = fs::read_dir(&workflows_dir) {
		for run in runs.flatten() {
			let run_path = run.path();
			if !run_path.is_dir() {
				continue;
			}
			let Ok(run_name) = run.file_name().into_string() else {
				continue;
			};
			let run_id = run_name
				.strip_prefix("wf_")
				.unwrap_or(&run_name)
				.to_string();
			let Ok(files) = fs::read_dir(&run_path) else {
				continue;
			};
			for f in files.flatten() {
				let Ok(name) = f.file_name().into_string() else {
					continue;
				};
				if let Some(agent_id) = agent_id_from_jsonl(&name) {
					refs.push(AgentFileRef {
						agent_id,
						jsonl_path: f.path(),
						meta_dir: run_path.clone(),
						workflow_run: Some(run_id.clone()),
					});
				}
			}
		}
	}

	refs
}

/// `agent-<id>.jsonl` 形式のファイル名から `<id>` を取り出す。
/// `.meta.json` 等はマッチしない。
fn agent_id_from_jsonl(file_name: &str) -> Option<String> {
	if !file_name.starts_with("agent-") || !file_name.ends_with(".jsonl") {
		return None;
	}
	Some(
		file_name
			.trim_start_matches("agent-")
			.trim_end_matches(".jsonl")
			.to_string(),
	)
}

/// jsonl の最初の非空行を読み、`message.content` のテキストを返す
/// (workflow agent のロール label 導出用)。先頭行だけ読むので軽量。
fn read_first_prompt_head(jsonl_path: &Path) -> Option<String> {
	let file = File::open(jsonl_path).ok()?;
	let mut reader = BufReader::new(file);
	let mut line = String::new();
	loop {
		line.clear();
		if reader.read_line(&mut line).ok()? == 0 {
			return None;
		}
		if !line.trim().is_empty() {
			break;
		}
	}
	let value: Value = serde_json::from_str(line.trim()).ok()?;
	let content = value.get("message")?.get("content")?;
	match content {
		Value::String(s) => Some(s.clone()),
		Value::Array(items) => items
			.iter()
			.find_map(|item| item.get("text").and_then(Value::as_str))
			.map(str::to_string),
		_ => None,
	}
}

/// workflow agent の先頭 prompt から短いロール label を導出する。
///
/// `agent()` に渡される人間可読 label は Claude Code 側で永続化されないため、
/// prompt 冒頭の「あなたは ○○ です」相当を切り出して代替する。導出できない
/// (空など) 場合は `None`。
fn derive_workflow_label(prompt_head: &str) -> Option<String> {
	let trimmed = prompt_head.trim_start();
	let body = trimmed.strip_prefix("あなたは").unwrap_or(trimmed);
	// 最初の文 / 節の区切りまでを label とする
	let end = body.find(['。', '（', '(', '\n']).unwrap_or(body.len());
	let label = body[..end].trim();
	// 末尾の丁寧表現「です」は label としては冗長なので落とす
	let label = label.strip_suffix("です").unwrap_or(label).trim_end();
	if label.is_empty() {
		return None;
	}
	if label.chars().count() > WORKFLOW_LABEL_MAX_CHARS {
		let mut truncated: String = label.chars().take(WORKFLOW_LABEL_MAX_CHARS).collect();
		truncated.push('…');
		return Some(truncated);
	}
	Some(label.to_string())
}

fn read_partial(path: &Path, offset: u64, len: u64) -> Option<String> {
	let mut f = File::open(path).ok()?;
	f.seek(SeekFrom::Start(offset)).ok()?;
	let mut buf = vec![0u8; len as usize];
	let n = f.read(&mut buf).ok()?;
	buf.truncate(n);
	Some(String::from_utf8_lossy(&buf).into_owned())
}

fn collapse_whitespace(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	let mut prev_ws = false;
	for c in s.chars() {
		if c.is_whitespace() {
			if !prev_ws {
				out.push(' ');
				prev_ws = true;
			}
		} else {
			out.push(c);
			prev_ws = false;
		}
	}
	out
}

// ---------------------------------------------------------------------------
// `.meta.json` サイドカー読込
// ---------------------------------------------------------------------------

/// `agent-<id>.meta.json` から `agentType` を取り出す。存在しない / パース失敗 /
/// `agentType` が空文字の場合は `None` を返す。
///
/// Claude Code は各サブエージェント jsonl の隣に
/// `{ "agentType": "general-purpose", "description"?: "..." }` 形式の
/// サイドカーを書く。これが現行の権威あるマッピング情報。
fn read_meta_sidecar_agent_type(agent_dir: &Path, agent_id: &str) -> Option<String> {
	let meta_path = agent_dir.join(format!("agent-{agent_id}.meta.json"));
	let content = fs::read_to_string(&meta_path).ok()?;
	let value: Value = serde_json::from_str(&content).ok()?;
	let t = value.get("agentType").and_then(Value::as_str)?;
	if t.is_empty() {
		return None;
	}
	Some(t.to_string())
}

// ---------------------------------------------------------------------------
// AgentMapper 構築 (末尾部分読み + 倍々拡大)
// ---------------------------------------------------------------------------

fn build_agent_mapper(
	file_path: &Path,
	expected_agent_count: usize,
) -> anyhow::Result<AgentMapperImpl> {
	let mut mapper = AgentMapperImpl::new();
	let meta = fs::metadata(file_path)?;
	let file_size = meta.len();
	if file_size == 0 {
		return Ok(mapper);
	}

	let mut tail_bytes = MAPPER_INITIAL_TAIL_BYTES.min(file_size);
	loop {
		let tail_start = file_size.saturating_sub(tail_bytes);
		let tail_len = file_size - tail_start;
		let Some(content) = read_partial(file_path, tail_start, tail_len) else {
			break;
		};

		let lines: Vec<&str> = if tail_start > 0 {
			match content.find('\n') {
				Some(nl) => content[(nl + 1)..].lines().collect(),
				None => Vec::new(),
			}
		} else {
			content.lines().collect()
		};

		for line in lines {
			if line.is_empty() {
				continue;
			}
			if !line_is_mapper_relevant(line) {
				continue;
			}
			let Ok(entry) = serde_json::from_str(line) else {
				continue;
			};
			mapper.process_entry(&entry);
		}

		if mapper.get_all_mappings().len() >= expected_agent_count || tail_start == 0 {
			break;
		}
		let next = tail_bytes.saturating_mul(2);
		tail_bytes = next.min(file_size);
	}

	Ok(mapper)
}

/// メインログの `assistant`/`progress`/async `tool_result` を安価に判定する
/// prefilter。JSON を舐める前に文字列マッチで弾く (Issue 002 で "Agent" も受理)。
fn line_is_mapper_relevant(line: &str) -> bool {
	let has_assistant_task = (line.contains("\"type\":\"assistant\"")
		|| line.contains("\"type\": \"assistant\""))
		&& (line.contains("\"name\":\"Task\"")
			|| line.contains("\"name\": \"Task\"")
			|| line.contains("\"name\":\"Agent\"")
			|| line.contains("\"name\": \"Agent\""));
	let has_progress =
		line.contains("\"type\":\"progress\"") || line.contains("\"type\": \"progress\"");
	let has_async_tool_result = (line.contains("\"type\":\"user\"")
		|| line.contains("\"type\": \"user\""))
		&& line.contains("\"toolUseResult\"")
		&& (line.contains("\"isAsync\":true") || line.contains("\"isAsync\": true"));
	has_assistant_task || has_progress || has_async_tool_result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
	use super::*;
	use std::fs::{create_dir_all, File};
	use std::io::Write;
	use tempfile::tempdir;

	fn write_file(path: &Path, content: &str) {
		let mut f = File::create(path).unwrap();
		f.write_all(content.as_bytes()).unwrap();
	}

	#[test]
	fn encode_workspace_path_replaces_slash_dot_underscore() {
		let p = Path::new("/Users/foo.bar/my_project");
		assert_eq!(encode_workspace_path(p), "-Users-foo-bar-my-project");
	}

	#[test]
	fn collapse_whitespace_handles_multiple_spaces_and_newlines() {
		assert_eq!(collapse_whitespace("a\n\nb  c\td"), "a b c d");
	}

	#[test]
	fn line_prefilter_accepts_agent_and_task_names() {
		assert!(line_is_mapper_relevant(
			r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Agent"}]}}"#
		));
		assert!(line_is_mapper_relevant(
			r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Task"}]}}"#
		));
		assert!(line_is_mapper_relevant(
			r#"{"type":"progress","data":{"agentId":"a"}}"#
		));
		assert!(line_is_mapper_relevant(
			r#"{"type":"user","toolUseResult":{"isAsync":true}}"#
		));
		assert!(!line_is_mapper_relevant(
			r#"{"type":"user","message":{"content":"hi"}}"#
		));
	}

	#[test]
	fn get_all_sessions_returns_sessions_sorted_by_updated_at() {
		let root = tempdir().unwrap();
		// workspace エンコード済みディレクトリ名 (例: cwd = /tmp/.../ws/project → "-private-tmp-...-ws-project" になる)。
		// テストでは cwd override + workspace の両方を与えて直接制御する。
		let projects_dir = root.path().join("projects");
		let fake_cwd = root.path().join("proj");
		create_dir_all(&fake_cwd).unwrap();
		let canonical_cwd = fs::canonicalize(&fake_cwd).unwrap();
		let encoded = encode_workspace_path(&canonical_cwd);
		let ws_dir = projects_dir.join(&encoded);
		create_dir_all(&ws_dir).unwrap();

		// 2 つのセッションを用意
		let s1_path = ws_dir.join("sess-older.jsonl");
		write_file(
			&s1_path,
			&format!(
				"{}\n{}\n",
				r#"{"type":"user","uuid":"u1","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"first old"}}"#,
				r#"{"type":"summary","summary":"old session"}"#
			),
		);
		let s2_path = ws_dir.join("sess-newer.jsonl");
		write_file(
			&s2_path,
			&format!(
				"{}\n{}\n",
				r#"{"type":"user","uuid":"u1","timestamp":"2024-06-01T00:00:00Z","message":{"role":"user","content":"first new"}}"#,
				r#"{"type":"summary","summary":"new session"}"#
			),
		);

		let repo = FileSystemSessionRepository::new();
		let options = GetAllSessionsOptions {
			limit: 10,
			claude_projects_dir: Some(projects_dir.clone()),
			cwd: Some(fake_cwd.clone()),
			..GetAllSessionsOptions::default()
		};
		let sessions = repo.get_all_sessions(None, &options);

		assert_eq!(sessions.len(), 2, "expected two sessions");
		assert_eq!(sessions[0].session_id, "sess-newer");
		assert_eq!(sessions[1].session_id, "sess-older");
		assert_eq!(sessions[0].metadata.summary.as_deref(), Some("new session"));
		assert_eq!(sessions[0].metadata.first_prompt, "first new");
	}

	#[test]
	fn get_all_sessions_respects_limit() {
		let root = tempdir().unwrap();
		let projects_dir = root.path().join("projects");
		let fake_cwd = root.path().join("proj2");
		create_dir_all(&fake_cwd).unwrap();
		let canonical_cwd = fs::canonicalize(&fake_cwd).unwrap();
		let encoded = encode_workspace_path(&canonical_cwd);
		let ws_dir = projects_dir.join(&encoded);
		create_dir_all(&ws_dir).unwrap();

		for i in 0..5 {
			let path = ws_dir.join(format!("sess-{i}.jsonl"));
			write_file(
				&path,
				&format!(
					"{{\"type\":\"user\",\"uuid\":\"u1\",\"timestamp\":\"2024-01-0{}T00:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"p{i}\"}}}}\n",
					i + 1
				),
			);
		}

		let repo = FileSystemSessionRepository::new();
		let sessions = repo.get_all_sessions(
			None,
			&GetAllSessionsOptions {
				limit: 2,
				claude_projects_dir: Some(projects_dir),
				cwd: Some(fake_cwd),
				..GetAllSessionsOptions::default()
			},
		);
		assert_eq!(sessions.len(), 2);
	}

	#[test]
	fn count_subagents_excludes_hidden_prefixes_unless_show_all() {
		let root = tempdir().unwrap();
		let dir = root.path().join("subagents");
		create_dir_all(&dir).unwrap();
		// 可視 2 件 + HIDDEN プレフィックス 2 件 + 関係ないファイル 1 件
		write_file(&dir.join("agent-visible1.jsonl"), "{}\n");
		write_file(&dir.join("agent-visible2.jsonl"), "{}\n");
		write_file(&dir.join("agent-aprompt_suggestion_a.jsonl"), "{}\n");
		write_file(&dir.join("agent-acompact-x.jsonl"), "{}\n");
		write_file(&dir.join("agent-visible1.meta.json"), "{}");
		write_file(&dir.join("README.md"), "noise");

		assert_eq!(count_subagents(&dir, false), 2, "default は HIDDEN を除外");
		assert_eq!(count_subagents(&dir, true), 4, "show_all は HIDDEN を含む");
	}

	#[test]
	fn count_subagents_returns_zero_for_missing_dir() {
		let root = tempdir().unwrap();
		let missing = root.path().join("nope/subagents");
		assert_eq!(count_subagents(&missing, false), 0);
	}

	#[test]
	fn get_all_sessions_populates_subagent_count_from_dir() {
		let root = tempdir().unwrap();
		let projects_dir = root.path().join("projects");
		let fake_cwd = root.path().join("proj-count");
		create_dir_all(&fake_cwd).unwrap();
		let canonical_cwd = fs::canonicalize(&fake_cwd).unwrap();
		let encoded = encode_workspace_path(&canonical_cwd);
		let ws_dir = projects_dir.join(&encoded);
		create_dir_all(&ws_dir).unwrap();

		let session_id = "sess-count";
		write_file(
			&ws_dir.join(format!("{session_id}.jsonl")),
			r#"{"type":"user","uuid":"u1","timestamp":"2024-06-01T00:00:00Z","message":{"role":"user","content":"hi"}}
"#,
		);
		let subagents_dir = ws_dir.join(session_id).join("subagents");
		create_dir_all(&subagents_dir).unwrap();
		write_file(&subagents_dir.join("agent-a.jsonl"), "{}\n");
		write_file(&subagents_dir.join("agent-b.jsonl"), "{}\n");
		write_file(
			&subagents_dir.join("agent-aprompt_suggestion_x.jsonl"),
			"{}\n",
		);

		let repo = FileSystemSessionRepository::new();
		let default_sessions = repo.get_all_sessions(
			None,
			&GetAllSessionsOptions {
				claude_projects_dir: Some(projects_dir.clone()),
				cwd: Some(fake_cwd.clone()),
				..GetAllSessionsOptions::default()
			},
		);
		assert_eq!(default_sessions.len(), 1);
		assert_eq!(
			default_sessions[0].subagent_count, 2,
			"show_all=false は HIDDEN を除外して 2"
		);

		let all_sessions = repo.get_all_sessions(
			None,
			&GetAllSessionsOptions {
				show_all: true,
				claude_projects_dir: Some(projects_dir),
				cwd: Some(fake_cwd),
				..GetAllSessionsOptions::default()
			},
		);
		assert_eq!(
			all_sessions[0].subagent_count, 3,
			"show_all=true は HIDDEN 含む 3"
		);
	}

	#[test]
	fn get_all_sessions_filters_by_since_window() {
		// `since` で指定した期間より古い `updated_at` (= last_user_action_at)
		// のセッションを Phase 4 で除外する回帰テスト。基準時刻 (`now`) は
		// 固定値で渡し、`updated_at` の判定だけで pass / fail が決まるようにする
		// (mtime は new file = テスト実行時刻なので Phase 2 cutoff は通る)。
		let root = tempdir().unwrap();
		let projects_dir = root.path().join("projects");
		let fake_cwd = root.path().join("proj-since");
		create_dir_all(&fake_cwd).unwrap();
		let canonical_cwd = fs::canonicalize(&fake_cwd).unwrap();
		let encoded = encode_workspace_path(&canonical_cwd);
		let ws_dir = projects_dir.join(&encoded);
		create_dir_all(&ws_dir).unwrap();

		// fixed_now はテスト実行時刻にほぼ揃える (= mtime と同じスケール)。
		// これで Phase 2 の mtime cutoff は通り、Phase 4 の updated_at cutoff
		// だけが効く。timestamps は fixed_now を起点に Duration で記述し、
		// 実時間との小さな差は問題にならない (since=7d に対して数秒の誤差)。
		let fixed_now: DateTime<Utc> = Utc::now();
		// fresh_at = fixed_now - 1d, stale_at = fixed_now - 30d
		let fresh_at = fixed_now - Duration::days(1);
		let stale_at = fixed_now - Duration::days(30);

		write_file(
			&ws_dir.join("sess-fresh.jsonl"),
			&format!(
				r#"{{"type":"user","uuid":"u1","timestamp":"{}","message":{{"role":"user","content":"fresh"}}}}
"#,
				fresh_at.to_rfc3339()
			),
		);
		write_file(
			&ws_dir.join("sess-stale.jsonl"),
			&format!(
				r#"{{"type":"user","uuid":"u1","timestamp":"{}","message":{{"role":"user","content":"stale"}}}}
"#,
				stale_at.to_rfc3339()
			),
		);

		let repo = FileSystemSessionRepository::new();
		let sessions = repo.get_all_sessions(
			None,
			&GetAllSessionsOptions {
				limit: 50,
				since: Some(Duration::days(7)),
				now: Some(fixed_now),
				claude_projects_dir: Some(projects_dir),
				cwd: Some(fake_cwd),
				..GetAllSessionsOptions::default()
			},
		);
		assert_eq!(sessions.len(), 1, "stale session must be excluded");
		assert_eq!(sessions[0].session_id, "sess-fresh");
	}

	#[test]
	fn get_all_sessions_without_since_returns_all() {
		// `since: None` のときはこれまでどおり期間で絞らない (後方互換)。
		let root = tempdir().unwrap();
		let projects_dir = root.path().join("projects");
		let fake_cwd = root.path().join("proj-no-since");
		create_dir_all(&fake_cwd).unwrap();
		let canonical_cwd = fs::canonicalize(&fake_cwd).unwrap();
		let encoded = encode_workspace_path(&canonical_cwd);
		let ws_dir = projects_dir.join(&encoded);
		create_dir_all(&ws_dir).unwrap();

		let old_path = ws_dir.join("sess-very-old.jsonl");
		write_file(
			&old_path,
			r#"{"type":"user","uuid":"u1","timestamp":"2020-01-01T00:00:00Z","message":{"role":"user","content":"old"}}
"#,
		);

		let repo = FileSystemSessionRepository::new();
		let sessions = repo.get_all_sessions(
			None,
			&GetAllSessionsOptions {
				limit: 50,
				claude_projects_dir: Some(projects_dir),
				cwd: Some(fake_cwd),
				..GetAllSessionsOptions::default()
			},
		);
		assert_eq!(sessions.len(), 1);
		assert_eq!(sessions[0].session_id, "sess-very-old");
	}

	#[test]
	fn get_sub_agents_filters_hidden_agents_by_default() {
		let root = tempdir().unwrap();
		let projects_dir = root.path().join("projects");
		let fake_cwd = root.path().join("proj3");
		create_dir_all(&fake_cwd).unwrap();
		let canonical_cwd = fs::canonicalize(&fake_cwd).unwrap();
		let encoded = encode_workspace_path(&canonical_cwd);
		let ws_dir = projects_dir.join(&encoded);
		create_dir_all(&ws_dir).unwrap();
		let session_id = "sess-1";
		let main_log = ws_dir.join(format!("{session_id}.jsonl"));
		// tool_use (name:"Agent", id:"t1") + progress(agentId:"visible1")
		let main_lines = [
			r#"{"type":"assistant","message":{"model":"m","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Agent","input":{"subagent_type":"Explore","description":"d","prompt":"p"}}]}}"#.to_string(),
			r#"{"type":"progress","data":{"type":"agent_progress","agentId":"visible1"},"parentToolUseID":"t1"}"#.to_string(),
		];
		write_file(&main_log, &format!("{}\n", main_lines.join("\n")));

		let subagents_dir = ws_dir.join(session_id).join("subagents");
		create_dir_all(&subagents_dir).unwrap();
		write_file(
			&subagents_dir.join("agent-visible1.jsonl"),
			r#"{"type":"user","message":{"role":"user","content":"hi"}}
"#,
		);
		write_file(
			&subagents_dir.join("agent-aprompt_suggestion_abc.jsonl"),
			r#"{"type":"user","message":{"role":"user","content":"x"}}
"#,
		);

		let session = SessionEntity {
			session_id: session_id.to_string(),
			workspace: encoded.clone(),
			file_path: main_log,
			updated_at: Utc::now(),
			subagents_dir,
			subagent_count: 0,
			metadata: SessionMetadata::default(),
		};

		let repo = FileSystemSessionRepository::new();

		let default = repo.get_sub_agents(&session, false);
		assert_eq!(default.len(), 1);
		assert_eq!(default[0].agent_id, "visible1");
		assert_eq!(default[0].agent_type, "Explore");

		let all = repo.get_sub_agents(&session, true);
		assert_eq!(all.len(), 2);
	}

	/// 回帰テスト: 現行 Claude Code は `progress` エントリを一切出さず、
	/// マッピング情報は `agent-<id>.meta.json` サイドカーに書かれる。
	/// サイドカーがあればそれを優先して読み取り、`agent_type` に反映する。
	#[test]
	fn get_sub_agents_reads_agent_type_from_meta_json_sidecar() {
		let root = tempdir().unwrap();
		let projects_dir = root.path().join("projects");
		let fake_cwd = root.path().join("proj-meta");
		create_dir_all(&fake_cwd).unwrap();
		let canonical_cwd = fs::canonicalize(&fake_cwd).unwrap();
		let encoded = encode_workspace_path(&canonical_cwd);
		let ws_dir = projects_dir.join(&encoded);
		create_dir_all(&ws_dir).unwrap();
		let session_id = "sess-meta";
		let main_log = ws_dir.join(format!("{session_id}.jsonl"));

		// 現行ログ相当: Agent tool_use はあるが progress / async tool_result は存在しない
		let main_lines = [
			r#"{"type":"assistant","message":{"model":"m","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Agent","input":{"subagent_type":"general-purpose","description":"d","prompt":"p"}}]}}"#,
		];
		write_file(&main_log, &format!("{}\n", main_lines.join("\n")));

		let subagents_dir = ws_dir.join(session_id).join("subagents");
		create_dir_all(&subagents_dir).unwrap();
		write_file(
			&subagents_dir.join("agent-a1.jsonl"),
			r#"{"type":"user","message":{"role":"user","content":"hi"}}
"#,
		);
		// 権威あるサイドカー
		write_file(
			&subagents_dir.join("agent-a1.meta.json"),
			r#"{"agentType":"architect-perf","description":"perf review"}"#,
		);

		let session = SessionEntity {
			session_id: session_id.to_string(),
			workspace: encoded,
			file_path: main_log,
			updated_at: Utc::now(),
			subagents_dir,
			subagent_count: 0,
			metadata: SessionMetadata::default(),
		};

		let repo = FileSystemSessionRepository::new();
		let agents = repo.get_sub_agents(&session, false);
		assert_eq!(agents.len(), 1);
		assert_eq!(agents[0].agent_id, "a1");
		// progress が無くてもサイドカーから "architect-perf" が取れる
		assert_eq!(agents[0].agent_type, "architect-perf");
	}

	/// サイドカーが空文字 / 不正 JSON の場合は旧メインログ経路にフォールバックする。
	#[test]
	fn get_sub_agents_falls_back_to_main_log_when_meta_json_invalid() {
		let root = tempdir().unwrap();
		let projects_dir = root.path().join("projects");
		let fake_cwd = root.path().join("proj-fallback");
		create_dir_all(&fake_cwd).unwrap();
		let canonical_cwd = fs::canonicalize(&fake_cwd).unwrap();
		let encoded = encode_workspace_path(&canonical_cwd);
		let ws_dir = projects_dir.join(&encoded);
		create_dir_all(&ws_dir).unwrap();
		let session_id = "sess-fb";
		let main_log = ws_dir.join(format!("{session_id}.jsonl"));

		// 旧形式: Agent tool_use + progress でマッピング確定
		let main_lines = [
			r#"{"type":"assistant","message":{"model":"m","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Agent","input":{"subagent_type":"Explore","description":"d","prompt":"p"}}]}}"#,
			r#"{"type":"progress","data":{"type":"agent_progress","agentId":"bork"},"parentToolUseID":"t1"}"#,
		];
		write_file(&main_log, &format!("{}\n", main_lines.join("\n")));

		let subagents_dir = ws_dir.join(session_id).join("subagents");
		create_dir_all(&subagents_dir).unwrap();
		write_file(
			&subagents_dir.join("agent-bork.jsonl"),
			r#"{"type":"user","message":{"role":"user","content":"hi"}}
"#,
		);
		// サイドカーは壊れた JSON → フォールバックされるべき
		write_file(
			&subagents_dir.join("agent-bork.meta.json"),
			r#"{ not valid json"#,
		);

		let session = SessionEntity {
			session_id: session_id.to_string(),
			workspace: encoded,
			file_path: main_log,
			updated_at: Utc::now(),
			subagents_dir,
			subagent_count: 0,
			metadata: SessionMetadata::default(),
		};

		let repo = FileSystemSessionRepository::new();
		let agents = repo.get_sub_agents(&session, false);
		assert_eq!(agents.len(), 1);
		assert_eq!(agents[0].agent_type, "Explore");
	}

	/// 混在: 一部 agent にのみサイドカーがあり、残りはメインログから引く
	/// ケース。`build_agent_mapper` は 1 回だけ呼ばれ、未解決分だけ埋める。
	#[test]
	fn get_sub_agents_mixes_meta_json_and_main_log_sources() {
		let root = tempdir().unwrap();
		let projects_dir = root.path().join("projects");
		let fake_cwd = root.path().join("proj-mixed");
		create_dir_all(&fake_cwd).unwrap();
		let canonical_cwd = fs::canonicalize(&fake_cwd).unwrap();
		let encoded = encode_workspace_path(&canonical_cwd);
		let ws_dir = projects_dir.join(&encoded);
		create_dir_all(&ws_dir).unwrap();
		let session_id = "sess-mix";
		let main_log = ws_dir.join(format!("{session_id}.jsonl"));

		// legacy 側 ("legacy1") はメインログから Explore を引く
		let main_lines = [
			r#"{"type":"assistant","message":{"model":"m","role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Agent","input":{"subagent_type":"Explore","description":"d","prompt":"p"}}]}}"#,
			r#"{"type":"progress","data":{"type":"agent_progress","agentId":"legacy1"},"parentToolUseID":"t1"}"#,
		];
		write_file(&main_log, &format!("{}\n", main_lines.join("\n")));

		let subagents_dir = ws_dir.join(session_id).join("subagents");
		create_dir_all(&subagents_dir).unwrap();
		// meta.json 側 ("modern1")
		write_file(
			&subagents_dir.join("agent-modern1.jsonl"),
			r#"{"type":"user","message":{"role":"user","content":"m"}}
"#,
		);
		write_file(
			&subagents_dir.join("agent-modern1.meta.json"),
			r#"{"agentType":"general-purpose"}"#,
		);
		// legacy 側 ("legacy1") — サイドカー無し
		write_file(
			&subagents_dir.join("agent-legacy1.jsonl"),
			r#"{"type":"user","message":{"role":"user","content":"l"}}
"#,
		);

		let session = SessionEntity {
			session_id: session_id.to_string(),
			workspace: encoded,
			file_path: main_log,
			updated_at: Utc::now(),
			subagents_dir,
			subagent_count: 0,
			metadata: SessionMetadata::default(),
		};

		let repo = FileSystemSessionRepository::new();
		let mut agents = repo.get_sub_agents(&session, false);
		agents.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
		assert_eq!(agents.len(), 2);
		assert_eq!(agents[0].agent_id, "legacy1");
		assert_eq!(agents[0].agent_type, "Explore");
		assert_eq!(agents[1].agent_id, "modern1");
		assert_eq!(agents[1].agent_type, "general-purpose");
	}

	/// Workflow ツール経由の agent は `subagents/workflows/<wf_runId>/` に
	/// 1 階層深く置かれる。通常の `subagents/` 直下 agent と同じフラットな
	/// リストに統合し、`workflow_run` / `workflow_label` を立てる。
	#[test]
	fn get_sub_agents_discovers_workflow_agents() {
		let root = tempdir().unwrap();
		let subagents_dir = root.path().join("sess").join("subagents");
		create_dir_all(&subagents_dir).unwrap();

		// 通常のフラット agent
		write_file(
			&subagents_dir.join("agent-flat1.jsonl"),
			"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
		);
		write_file(
			&subagents_dir.join("agent-flat1.meta.json"),
			r#"{"agentType":"Explore"}"#,
		);

		// Workflow run ディレクトリ
		let wf_dir = subagents_dir.join("workflows").join("wf_run123");
		create_dir_all(&wf_dir).unwrap();
		// generic な workflow-subagent (label は prompt 先頭から導出する)
		write_file(
			&wf_dir.join("agent-wfa.jsonl"),
			"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"あなたは code-review の finder です（角度: simplification）。recall 重視\"}}\n",
		);
		write_file(
			&wf_dir.join("agent-wfa.meta.json"),
			r#"{"agentType":"workflow-subagent"}"#,
		);
		// カスタム型 (型名がそのまま label になるので workflow_label は None)
		write_file(
			&wf_dir.join("agent-wfb.jsonl"),
			"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"レビューして\"}}\n",
		);
		write_file(
			&wf_dir.join("agent-wfb.meta.json"),
			r#"{"agentType":"consistency-reviewer"}"#,
		);

		let session = SessionEntity {
			session_id: "sess".to_string(),
			workspace: "ws".to_string(),
			file_path: root.path().join("sess.jsonl"),
			updated_at: Utc::now(),
			subagents_dir,
			subagent_count: 0,
			metadata: SessionMetadata::default(),
		};

		let repo = FileSystemSessionRepository::new();
		let mut agents = repo.get_sub_agents(&session, false);
		agents.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
		assert_eq!(agents.len(), 3, "フラット 1 + workflow 2 = 3");

		let flat = &agents[0];
		assert_eq!(flat.agent_id, "flat1");
		assert_eq!(flat.agent_type, "Explore");
		assert_eq!(flat.workflow_run, None, "通常 agent は workflow_run なし");

		let wfa = &agents[1];
		assert_eq!(wfa.agent_id, "wfa");
		assert_eq!(wfa.agent_type, "workflow-subagent");
		assert_eq!(
			wfa.workflow_run.as_deref(),
			Some("run123"),
			"wf_ プレフィックスを除いた run id"
		);
		assert_eq!(
			wfa.workflow_label.as_deref(),
			Some("code-review の finder"),
			"generic な workflow-subagent は prompt 先頭からロールを導出"
		);
		// output_path は nested の実体を指す (attach 経路で使う)
		assert!(wfa
			.output_path
			.ends_with("workflows/wf_run123/agent-wfa.jsonl"));

		let wfb = &agents[2];
		assert_eq!(wfb.agent_id, "wfb");
		assert_eq!(wfb.agent_type, "consistency-reviewer");
		assert_eq!(wfb.workflow_run.as_deref(), Some("run123"));
		assert_eq!(
			wfb.workflow_label, None,
			"カスタム型は型名が label になるので導出しない"
		);
	}

	/// セッション一覧の `[ N ]` カウントにも workflow agent を含める。
	#[test]
	fn count_subagents_includes_workflow_agents() {
		let root = tempdir().unwrap();
		let subagents_dir = root.path().join("subagents");
		create_dir_all(&subagents_dir).unwrap();
		write_file(&subagents_dir.join("agent-flat1.jsonl"), "{}\n");
		let wf_dir = subagents_dir.join("workflows").join("wf_r1");
		create_dir_all(&wf_dir).unwrap();
		write_file(&wf_dir.join("agent-w1.jsonl"), "{}\n");
		write_file(&wf_dir.join("agent-w2.jsonl"), "{}\n");
		write_file(&wf_dir.join("agent-w1.meta.json"), "{}");
		write_file(&wf_dir.join("journal.jsonl"), "{}\n");

		assert_eq!(
			count_subagents(&subagents_dir, false),
			3,
			"フラット 1 + workflow 2、journal.jsonl は除外"
		);
	}

	#[test]
	fn derive_workflow_label_strips_prefix_and_polite_suffix() {
		assert_eq!(
			derive_workflow_label(
				"あなたは code-review の finder です（角度: simplification）。recall 重視"
			)
			.as_deref(),
			Some("code-review の finder")
		);
		assert_eq!(
			derive_workflow_label("あなたは実装プランナーです。以下のタスクについて…").as_deref(),
			Some("実装プランナー")
		);
	}

	#[test]
	fn derive_workflow_label_cuts_at_first_newline() {
		assert_eq!(
			derive_workflow_label("あなたは Bash 実行係\n詳細は以下").as_deref(),
			Some("Bash 実行係")
		);
	}

	#[test]
	fn derive_workflow_label_truncates_long_head() {
		let long = "WORKDIR=/Users/foo/bar/baz/qux/corge/grault/garply/waldo/fred/plugh";
		let label = derive_workflow_label(long).unwrap();
		assert!(label.ends_with('…'), "上限超過は … で省略: {label}");
		assert_eq!(label.chars().count(), WORKFLOW_LABEL_MAX_CHARS + 1);
	}

	#[test]
	fn derive_workflow_label_returns_none_for_empty() {
		assert_eq!(derive_workflow_label(""), None);
		assert_eq!(derive_workflow_label("   \n  "), None);
		assert_eq!(derive_workflow_label("あなたは"), None);
	}
}
