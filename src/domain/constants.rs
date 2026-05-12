//! Domain 定数。TS 版 `src/domain/constants.ts` の移植。

/// デフォルトで非表示にするエージェント ID のプレフィックス。
///
/// `--show-all` CLI フラグが指定された場合のみ表示する。
pub const HIDDEN_AGENT_PREFIXES: &[&str] = &["aprompt_suggestion", "acompact-"];

/// ローカルコマンド関連のプレフィックス。
///
/// `updatedAt` / `firstPrompt` 探索時にこれらで始まるメッセージはスキップする。
pub const LOCAL_COMMAND_PREFIXES: &[&str] = &[
	"<local-command-caveat>",
	"<command-name>",
	"<command-message>",
	"<local-command-stdout>",
];

/// メッセージの最大保持件数 (over the wire 表示)。
pub const MAX_MESSAGES: usize = 500;

/// Claude プロジェクトディレクトリ (チルダ展開が必要)。
pub const CLAUDE_PROJECTS_DIR: &str = "~/.claude/projects";

/// tmp ディレクトリベース (サブエージェント出力のシンボリックリンク置き場)。
pub const TMP_TASKS_BASE: &str = "/private/tmp/claude-501";

/// `get_session_metadata`: `firstPrompt` 探索用の先頭読み込みバイト数。
pub const HEAD_READ_BYTES: u64 = 32 * 1024;

/// `get_session_metadata`: `summary` / `lastUserActionAt` 探索用の末尾読み込みバイト数。
pub const TAIL_READ_BYTES: u64 = 16 * 1024;

/// mtime プレソートの候補数倍率。
pub const LIMIT_SAFETY_MULTIPLIER: f64 = 2.0;

/// mtime プレソートの候補数下限。
pub const LIMIT_MIN_CANDIDATES: usize = 30;

/// ファイル I/O のバッチ並列数上限。
pub const IO_BATCH_SIZE: usize = 32;

/// `get_sub_agents`: メインログマッピング構築の初期読み込みバイト数。
pub const MAPPER_INITIAL_TAIL_BYTES: u64 = 512 * 1024;

/// `subagent_type` がログ上で未指定のときのフォールバック。
///
/// Claude Code の Agent ツールは `subagent_type` を省略した場合
/// `general-purpose` として動作する。
pub const DEFAULT_SUBAGENT_TYPE: &str = "general-purpose";
