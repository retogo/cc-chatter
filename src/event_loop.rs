//! Event loop: tokio::select! で Key/Mouse/File/Tick を Msg に集約し、
//! App.update(msg) の返り値 Cmd を副作用として実行する。
//!
//! ## フロー
//!
//! ```text
//! crossterm EventStream ─┐
//! tokio::time::interval  ├──► select! ──► Msg ──► App.update ──► Vec<Cmd>
//! mpsc<Msg> (watchers) ──┘                                          │
//!                                                                   ▼
//!                                                            spawn_cmd_run
//!                                                            (watcher 起動 / 再取得など)
//!                                                                   │
//!                                                                   ▼
//!                                                             mpsc<Msg> (戻り値)
//! ```

use std::time::{Duration, Instant};

use crossterm::event::{Event as CtEvent, EventStream};
use futures_util::StreamExt;
use ratatui::backend::Backend;
use ratatui::Terminal;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time;
use tracing::{debug, warn};

use crate::application::msg::{Cmd, Msg};
use crate::application::App;
use crate::cli::CliOptions;
use crate::infrastructure::repositories::{FileSystemSessionRepository, GetAllSessionsOptions};
use crate::infrastructure::watchers::{MainWatcher, SubAgentWatcher};

/// event_loop の生存期間でアクティブなウォッチャーを持つコンテナ。
///
/// `sub_agents` は multi-attach 対応で複数の `SubAgentWatcher` を保持する。
/// `detach` で全 watcher が drop され、Drop 内で notify ハンドルが解放される。
#[derive(Default)]
struct Watchers {
	main: Option<MainWatcher>,
	sub_agents: Vec<SubAgentWatcher>,
}

impl Watchers {
	fn detach(&mut self) {
		self.main = None;
		self.sub_agents.clear();
	}
}

/// ratatui Terminal と App を受け取り、TUI ループを回す。
///
/// `initial_cmds` は startup skip 経路で watcher 起動等を行うための Cmd 列。
/// 呼び出し前に `Cmd::RefreshSessions` は常に一度発火する。
pub async fn run<B: Backend>(
	mut terminal: Terminal<B>,
	mut app: App,
	cmd_tx: UnboundedSender<Msg>,
	mut cmd_rx: UnboundedReceiver<Msg>,
	initial_cmds: Vec<Cmd>,
) -> anyhow::Result<()> {
	let mut crossterm_events = EventStream::new();
	// Tick 周期 = ChatBubble スピナーの 1 フレーム時間 (100ms)。10 frame 周期で
	// 1 周 1 秒。`App::handle_tick` は viewport 内に未完了 tool_use があるとき
	// だけ `mark_dirty` するので、静止状態では `take_needs_redraw` が false で
	// draw skip され、CPU 負荷はゼロに近い。
	let mut tick = time::interval(Duration::from_millis(100));
	// 既定の `Burst` だと主ループがキー連打等でビジーになった直後に
	// 蓄積された Tick が連続発火し、select! ループが tight になる。
	// `Skip` にしておけば遅延した Tick は捨てられる。
	tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
	let mut watchers = Watchers::default();

	// 必ずセッション一覧を一度は引く
	spawn_cmd(Cmd::RefreshSessions, &cmd_tx, &app.options);

	// startup skip 由来の Cmd を適用
	for cmd in initial_cmds {
		apply_cmd(cmd, &mut app, &mut watchers, &cmd_tx);
	}

	// 初期描画 (App::new が needs_redraw=true で始まるので最初は必ず描画される)
	draw_if_dirty(&mut terminal, &mut app)?;

	loop {
		// 次の Msg を待つ。`biased;` は宣言順に評価 → キー/マウスが tick より
		// 優先されるのでレスポンスがブレない (通常の select! は ready な枝から
		// ランダム選択で、キー直後でも tick が先に選ばれるケースがある)。
		let msg = tokio::select! {
			biased;
			Some(ev) = crossterm_events.next() => match ev {
				Ok(CtEvent::Key(k)) => Msg::Key(k),
				Ok(CtEvent::Mouse(m)) => Msg::Mouse(m),
				Ok(CtEvent::Resize(c, r)) => Msg::Resize { cols: c, rows: r },
				Ok(_) => continue,
				Err(_) => continue,
			},
			Some(m) = cmd_rx.recv() => m,
			_ = tick.tick() => Msg::Tick,
			else => break,
		};

		let cmds = app.update(msg);
		for cmd in cmds {
			apply_cmd(cmd, &mut app, &mut watchers, &cmd_tx);
		}

		// watcher 系 Msg (MessagesAppended など) のバーストを 1 draw にまとめる。
		// crossterm event (Key/Mouse) は drain しない: トラックパッドフリックの
		// ScrollDown が 40+ 件で 1 draw に coalesce されると「画面が大きく
		// ジャンプする」カクつきになる。入力は biased select で優先されるので
		// 1 event = 1 draw のループで自然にスムーズスクロールが実現される。
		let _ = drain_ready_messages(&mut cmd_rx, &mut app, &mut watchers, &cmd_tx);

		// dirty flag が立っているときだけ再描画する。
		// Msg::Tick 単独では needs_redraw=false のままで無駄な描画を省く。
		draw_if_dirty(&mut terminal, &mut app)?;

		if app.should_quit {
			break;
		}
	}

	// ループ終了時に watcher を明示停止 (Drop にも任せるが念のため)
	watchers.detach();
	Ok(())
}

/// 1 フレーム内でまとめて処理する watcher Msg の上限。これを超えたら一旦 draw に
/// 譲る (永遠に event が流入し続けた場合に描画が止まるのを防ぐ)。
const MAX_DRAIN_PER_FRAME: usize = 64;

/// 非ブロッキングで **`cmd_rx` (watcher) のみ** 消化する。
///
/// 以前は crossterm event (Key/Mouse/Resize) も drain していたが、トラックパッドの
/// 1 フリックで ScrollDown が 40+ 件連続発火するケースで全部 1 draw に coalesce
/// されてしまい、「1 フリック → 画面が大きくジャンプ」というカクつきの原因に
/// なっていた。ユーザー入力は biased select で優先されるので、そのまま 1 event
/// = 1 draw のループで回す方が自然に見える (event 間隔 10ms × draw 1ms = スムーズ)。
///
/// watcher 系 (`Msg::MessagesAppended` / `Msg::AgentsLoaded` 等) は streaming 中に
/// バーストするので drain にメリットがある (複数件を 1 draw にまとめる)。
fn drain_ready_messages(
	cmd_rx: &mut UnboundedReceiver<Msg>,
	app: &mut App,
	watchers: &mut Watchers,
	cmd_tx: &UnboundedSender<Msg>,
) -> usize {
	let mut drained = 0;

	while drained < MAX_DRAIN_PER_FRAME {
		match cmd_rx.try_recv() {
			Ok(msg) => {
				let cmds = app.update(msg);
				for cmd in cmds {
					apply_cmd(cmd, app, watchers, cmd_tx);
				}
				drained += 1;
			}
			Err(_) => {
				// Empty / Disconnected どちらもこのフレームでは終了
				break;
			}
		}
	}
	drained
}

/// App.needs_redraw が立っているときだけ `terminal.draw` を呼ぶ。
///
/// draw の elapsed を tracing::debug で出す。`RUST_LOG=cc_chatter=debug` +
/// `tracing_appender` でファイルに吐かれる (stdout は TUI が使うので不可)。
fn draw_if_dirty<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> anyhow::Result<()> {
	if !app.take_needs_redraw() {
		return Ok(());
	}
	let view = format!("{:?}", app.view.current_view);
	let t0 = Instant::now();
	terminal.draw(|f| crate::ui::view::view(f, app))?;
	let elapsed = t0.elapsed();
	let us = elapsed.as_micros() as u64;
	if us >= 8_000 {
		// 60fps 枠 (16ms) の半分を超えたら要注意。SubAgentSelect 画面でこれが
		// 連発していれば backend 書き込み or view render のどちらかがボトルネック。
		warn!(
			view = %view,
			messages = app.domain.messages.len(),
			elapsed_us = us,
			"slow draw (>= 8ms)"
		);
	} else {
		debug!(
			view = %view,
			messages = app.domain.messages.len(),
			elapsed_us = us,
			"terminal.draw"
		);
	}
	Ok(())
}

/// App が発行した Cmd を副作用として実行する。
///
/// watcher の起動/停止は App state と直接関係しない所有権のため、
/// event_loop 側の `Watchers` で持つ。
fn apply_cmd(cmd: Cmd, app: &mut App, watchers: &mut Watchers, cmd_tx: &UnboundedSender<Msg>) {
	match cmd {
		Cmd::RefreshSessions => {
			spawn_cmd(Cmd::RefreshSessions, cmd_tx, &app.options);
		}
		Cmd::RefreshAgents { session_id } => {
			spawn_cmd(Cmd::RefreshAgents { session_id }, cmd_tx, &app.options);
		}
		Cmd::AttachToAgents {
			session_id,
			agent_ids,
		} => {
			// 既存 watcher を detach してから新規 attach
			watchers.detach();
			// 対象 session を domain state から引く。なければエラー
			let Some(session) = app
				.domain
				.sessions
				.iter()
				.find(|s| s.session_id == session_id)
				.cloned()
			else {
				let _ = cmd_tx.send(Msg::Error(format!(
					"AttachToAgents: session {session_id} not found in domain state"
				)));
				return;
			};
			// 各 agent_id に対して output_path を解決し、SubAgentWatcher を起動。
			// 1 つでもファイル特定できれば watcher が立つ (fallback 経路あり)。
			for agent_id in &agent_ids {
				let path = app
					.domain
					.agents
					.iter()
					.find(|a| &a.agent_id == agent_id)
					.map(|a| a.output_path.clone())
					.unwrap_or_else(|| {
						// agent がまだ AgentsLoaded 経路で取れていない fallback
						session
							.subagents_dir
							.join(format!("agent-{agent_id}.jsonl"))
					});
				watchers.sub_agents.push(SubAgentWatcher::spawn(
					path,
					agent_id.clone(),
					cmd_tx.clone(),
				));
			}
			// MainWatcher は session 単位で 1 つ。multi-attach でも共用できる。
			watchers.main = Some(MainWatcher::spawn(
				session,
				app.options.show_all,
				cmd_tx.clone(),
			));
		}
		Cmd::Detach => {
			watchers.detach();
		}
		Cmd::WaitForNewAgent { session_id } => {
			// 既存 watcher を detach し、MainWatcher のみ起動する (SubAgentWatcher は
			// まだ attach する agent が決まっていない)
			watchers.detach();
			let Some(session) = app
				.domain
				.sessions
				.iter()
				.find(|s| s.session_id == session_id)
				.cloned()
			else {
				let _ = cmd_tx.send(Msg::Error(format!(
					"WaitForNewAgent: session {session_id} not found"
				)));
				return;
			};
			watchers.main = Some(MainWatcher::spawn(
				session,
				app.options.show_all,
				cmd_tx.clone(),
			));
		}
		Cmd::Quit => {
			watchers.detach();
			app.should_quit = true;
		}
	}
}

/// 同期的に実行できる Cmd はここで実行し、結果を Msg として `tx` に戻す。
///
/// `RefreshSessions` / `RefreshAgents` はどちらも FS アクセスなので tokio の
/// `spawn_blocking` で別スレッドに投げる。
fn spawn_cmd(cmd: Cmd, tx: &UnboundedSender<Msg>, options: &CliOptions) {
	let tx = tx.clone();
	let options = options.clone();
	match cmd {
		Cmd::RefreshSessions => {
			tokio::task::spawn_blocking(move || {
				let repo = FileSystemSessionRepository::new();
				let opts = GetAllSessionsOptions {
					limit: options.limit,
					since: Some(options.since),
					show_all: options.show_all,
					..GetAllSessionsOptions::default()
				};
				let sessions = repo.get_all_sessions(options.workspace.as_deref(), &opts);
				let _ = tx.send(Msg::SessionsLoaded(sessions));
			});
		}
		Cmd::RefreshAgents { session_id } => {
			tokio::task::spawn_blocking(move || {
				let repo = FileSystemSessionRepository::new();
				let opts = GetAllSessionsOptions {
					limit: options.limit,
					since: Some(options.since),
					show_all: options.show_all,
					..GetAllSessionsOptions::default()
				};
				let sessions = repo.get_all_sessions(options.workspace.as_deref(), &opts);
				let Some(session) = sessions
					.iter()
					.find(|s| s.session_id == session_id)
					.cloned()
				else {
					let _ = tx.send(Msg::AgentsLoaded {
						session_id,
						agents: Vec::new(),
					});
					return;
				};
				let agents = repo.get_sub_agents(&session, options.show_all);
				let _ = tx.send(Msg::AgentsLoaded {
					session_id: session.session_id.clone(),
					agents,
				});
			});
		}
		_ => {}
	}
}

/// 新しい mpsc チャネルを作る (main.rs から使う)。
pub fn make_channel() -> (UnboundedSender<Msg>, UnboundedReceiver<Msg>) {
	mpsc::unbounded_channel()
}
