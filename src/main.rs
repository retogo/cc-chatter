//! cc-chatter (Rust) CLI エントリーポイント。
//!
//! tokio runtime を立ち上げ、ターミナルを raw mode / alternate screen に
//! 遷移させたうえで `event_loop::run` を回す。
//!
//! ## 終了時の責務
//!
//! - **Drop** で TerminalGuard が raw mode / alternate screen / mouse capture
//!   を解除する (Ctrl+C 正常終了経路)
//! - **panic hook** で panic = "abort" 時でも後始末を試みる
//! - Ctrl+C は crossterm が `KeyEvent(Char('c'), CONTROL)` として返すので、
//!   App.update 側で `should_quit=true` + `Cmd::Quit` を返して event_loop が
//!   通常終了する
//!
//! ## 起動フロー
//!
//! 1. CliOptions をパース
//! 2. raw mode + alternate screen + mouse capture
//! 3. `resolve_startup_target` で `--latest` / `--session` / `--agent` を
//!    sessions/agents 抽出 + `resolve_startup_state` で解決
//! 4. `App::new` + `apply_startup_skip` で初期状態を作る
//! 5. `event_loop::run` で TUI ループ開始

use std::io;

use clap::Parser;
use ratatui::{backend::CrosstermBackend, Terminal};

use cc_chatter::application::msg::Cmd;
use cc_chatter::application::App;
use cc_chatter::cli::CliOptions;
use cc_chatter::domain::entities::{AgentEntity, SessionEntity};
use cc_chatter::event_loop;
use cc_chatter::infrastructure::input::{
	terminal_guard::{install_panic_hook, install_signal_hook, TerminalGuardOptions},
	TerminalGuard,
};
use cc_chatter::infrastructure::repositories::{
	FileSystemSessionRepository, GetAllSessionsOptions,
};

fn main() -> anyhow::Result<()> {
	// color-eyre で panic / error メッセージを整形 + Drop の前にハンドラを入れて
	// ターミナル復元を試みる。`panic = "abort"` でも Drop は走らないので hook
	// が最後の砦。
	color_eyre::install().ok();
	install_panic_hook();
	// 別プロセスから `kill <pid>` / tmux kill-session / セッション hangup で
	// 落ちるケース向けの保険。Ctrl+C は crossterm 経由で event_loop が拾う。
	install_signal_hook().ok();

	// logging: raw mode 下で stderr を出すと UI と混ざるので、`RUST_LOG` が
	// 明示されているときは `$TMPDIR/cc-chatter.log` にファイル出力する。
	// それ以外 (通常起動) は logger 無効 (環境フィルタ "warn" + ノイズなし)。
	//
	// 計測用途: `RUST_LOG=cc_chatter=debug` で起動 → 別ターミナルで
	// `tail -f $TMPDIR/cc-chatter.log` (macOS なら `/var/folders/.../T/`) を
	// 見ると `terminal.draw` の elapsed_us が観察できる。
	let _log_guard = init_logging();

	let opts = CliOptions::parse();

	// tokio runtime を自前で block_on する。#[tokio::main] を使わないのは:
	// エラー時 / panic 時の後始末パスを明示的に制御したいため。
	let runtime = tokio::runtime::Builder::new_multi_thread()
		.enable_all()
		.build()?;

	runtime.block_on(async move { run_tui(opts).await })
}

/// `RUST_LOG` が設定されているときだけ `$TMPDIR/cc-chatter.log` にログ出力を
/// セットアップする。返り値の `WorkerGuard` をドロップしないよう main に
/// 抱えさせる。
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
	let Ok(filter_raw) = std::env::var("RUST_LOG") else {
		return None;
	};
	if filter_raw.trim().is_empty() {
		return None;
	}
	let filter = tracing_subscriber::EnvFilter::try_new(&filter_raw)
		.unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

	let tmp_dir = std::env::temp_dir();
	let file_appender = tracing_appender::rolling::never(tmp_dir, "cc-chatter.log");
	let (nb_writer, guard) = tracing_appender::non_blocking(file_appender);
	tracing_subscriber::fmt()
		.with_env_filter(filter)
		.with_writer(nb_writer)
		.with_ansi(false)
		.init();
	Some(guard)
}

async fn run_tui(opts: CliOptions) -> anyhow::Result<()> {
	let guard_options = TerminalGuardOptions {
		enable_mouse: !opts.no_mouse,
	};
	let _guard = TerminalGuard::enter(guard_options)?;
	let backend = CrosstermBackend::new(io::stdout());
	let terminal = Terminal::new(backend)?;

	let size = terminal.size().unwrap_or_default();
	let (cmd_tx, cmd_rx) = event_loop::make_channel();

	let mut app = App::new(opts.clone(), size.height, size.width);

	// --- Startup skip --------------------------------------------------------
	let mut initial_cmds: Vec<Cmd> = Vec::new();
	if opts.wants_auto_attach() {
		if let Some(skip) = resolve_startup_target(&opts).await {
			match skip {
				StartupTarget::Watching {
					sessions,
					agents,
					session_id,
					agent_id,
				} => {
					// App の domain state にも sessions/agents を事前に流し込んでおく
					// (event_loop の AttachToAgent が domain state を参照するため)
					app.domain.sessions = sessions;
					app.domain.agents = agents;
					initial_cmds = app.apply_startup_skip(Some(session_id), Some(agent_id));
				}
				StartupTarget::Waiting {
					sessions,
					session_id,
				} => {
					app.domain.sessions = sessions;
					initial_cmds = app.apply_startup_skip(Some(session_id), None);
				}
			}
		}
	}

	// --- Event loop ----------------------------------------------------------
	event_loop::run(terminal, app, cmd_tx, cmd_rx, initial_cmds).await?;

	// _guard の drop でターミナル復元される
	Ok(())
}

/// startup skip で決まったターゲット。
enum StartupTarget {
	Watching {
		sessions: Vec<SessionEntity>,
		agents: Vec<AgentEntity>,
		session_id: String,
		agent_id: String,
	},
	Waiting {
		sessions: Vec<SessionEntity>,
		session_id: String,
	},
}

/// `--latest` / `--session` / `--agent` を解決する。
///
/// - sessions を 1 回ロード → prefix 解決
/// - 当選 session の agents をロード
/// - `resolve_startup_state` の 2 段階呼び出しで最終判定
/// - エラーは raw mode 解除後に表示するため `None` 返しで抑える
fn resolve_startup_target_inner(opts: &CliOptions) -> Option<StartupTarget> {
	let repo = FileSystemSessionRepository::new();
	let options = GetAllSessionsOptions {
		limit: opts.limit,
		since: Some(opts.since),
		show_all: opts.show_all,
		..GetAllSessionsOptions::default()
	};
	let sessions = repo.get_all_sessions(opts.workspace.as_deref(), &options);
	if sessions.is_empty() {
		return None;
	}

	let first = cc_chatter::application::usecases::resolve_startup_state(
		opts.latest,
		opts.session.as_deref(),
		opts.agent.as_deref(),
		&sessions,
		&[],
	)
	.ok()?;

	let session_id = first.target_session_id?;
	let session = sessions
		.iter()
		.find(|s| s.session_id == session_id)?
		.clone();
	let agents = repo.get_sub_agents(&session, opts.show_all);

	let final_resolve = cc_chatter::application::usecases::resolve_startup_state(
		opts.latest,
		opts.session.as_deref(),
		opts.agent.as_deref(),
		&sessions,
		&agents,
	)
	.ok()?;

	match final_resolve.view {
		Some(cc_chatter::application::usecases::ResolvedView::Watching {
			session_id,
			agent_id,
		}) => Some(StartupTarget::Watching {
			sessions,
			agents,
			session_id,
			agent_id,
		}),
		Some(cc_chatter::application::usecases::ResolvedView::Waiting { session_id }) => {
			Some(StartupTarget::Waiting {
				sessions,
				session_id,
			})
		}
		None => None,
	}
}

async fn resolve_startup_target(opts: &CliOptions) -> Option<StartupTarget> {
	let opts_clone = opts.clone();
	tokio::task::spawn_blocking(move || resolve_startup_target_inner(&opts_clone))
		.await
		.ok()
		.flatten()
}
