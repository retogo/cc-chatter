//! MainWatcher: サブエージェントディレクトリを監視して、新規エージェントが
//! 出現したら通知する。
//!
//! M2 スコープでは:
//! - `subagents/` ディレクトリを監視
//! - 新しい `agent-*.jsonl` が現れたら、その時点の agent 一覧を
//!   `Msg::AgentsLoaded` で通知する
//! - 個々の agent の `subagent_type` は `FileSystemSessionRepository::get_sub_agents`
//!   を再呼び出しして解決する (メインログから mapping 再構築)
//!
//! TS 版 `ChokidarMainWatcher` と似るが、agent 一覧の再取得は Repository に
//! 委譲する (独自に mapping を持たない)。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use notify::{event::EventKind, Config, PollWatcher, RecursiveMode, Watcher as NotifyWatcher};
use tokio::sync::mpsc::UnboundedSender;

use crate::application::msg::Msg;
use crate::domain::entities::SessionEntity;
use crate::infrastructure::repositories::FileSystemSessionRepository;

/// サブエージェント一覧監視。drop で停止。
pub struct MainWatcher {
	stop_flag: Arc<AtomicBool>,
	join_handle: Option<JoinHandle<()>>,
}

impl MainWatcher {
	pub fn spawn(session: SessionEntity, show_all: bool, tx: UnboundedSender<Msg>) -> Self {
		let stop_flag = Arc::new(AtomicBool::new(false));
		let stop_flag_for_thread = stop_flag.clone();
		let join_handle = thread::spawn(move || {
			run_main_watch_loop(session, show_all, tx, stop_flag_for_thread);
		});
		Self {
			stop_flag,
			join_handle: Some(join_handle),
		}
	}

	pub fn stop(mut self) {
		self.stop_internal();
	}

	fn stop_internal(&mut self) {
		self.stop_flag.store(true, Ordering::SeqCst);
		if let Some(handle) = self.join_handle.take() {
			let _ = handle.join();
		}
	}
}

impl Drop for MainWatcher {
	fn drop(&mut self) {
		self.stop_internal();
	}
}

fn run_main_watch_loop(
	session: SessionEntity,
	show_all: bool,
	tx: UnboundedSender<Msg>,
	stop_flag: Arc<AtomicBool>,
) {
	let repo = FileSystemSessionRepository::new();
	let (notify_tx, notify_rx) = std_mpsc::channel::<()>();

	let config = Config::default().with_poll_interval(Duration::from_millis(200));
	let notify_tx_cb = notify_tx.clone();
	let watcher_res = PollWatcher::new(
		move |res: notify::Result<notify::Event>| {
			if let Ok(event) = res {
				if matches!(
					event.kind,
					EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
				) {
					let _ = notify_tx_cb.send(());
				}
			}
		},
		config,
	);

	let mut watcher = match watcher_res {
		Ok(w) => w,
		Err(err) => {
			let _ = tx.send(Msg::Error(format!("MainWatcher init failed: {err}")));
			return;
		}
	};

	// subagents ディレクトリが無いとエラーになるので、存在しない場合は親を監視する
	let subagents_dir = session.subagents_dir.clone();
	let watch_target: PathBuf = if subagents_dir.exists() {
		subagents_dir.clone()
	} else if let Some(parent) = subagents_dir.parent() {
		parent.to_path_buf()
	} else {
		subagents_dir.clone()
	};
	// Recursive にして subagents/workflows/<wf_runId>/ に追加される Workflow ツール
	// 経由の agent も検出する (NonRecursive だと 1 階層下の新規作成を取りこぼす)
	if let Err(err) = watcher.watch(&watch_target, RecursiveMode::Recursive) {
		let _ = tx.send(Msg::Error(format!(
			"MainWatcher.watch failed for {}: {err}",
			watch_target.display()
		)));
	}
	// メインログ自体も監視 (新しい Agent tool call でマッピングが増える)
	if session.file_path.exists() {
		let _ = watcher.watch(&session.file_path, RecursiveMode::NonRecursive);
	}

	// 既知 agent id の set (前回送信時)。新規出現検知に使う
	let mut known_agent_ids = current_agent_ids(&repo, &session, show_all);
	// 初回送信
	send_agents(&repo, &session, show_all, &tx);

	loop {
		if stop_flag.load(Ordering::SeqCst) {
			break;
		}
		match notify_rx.recv_timeout(Duration::from_millis(250)) {
			Ok(()) => {
				while notify_rx.try_recv().is_ok() {}
				let current = repo.get_sub_agents(&session, show_all);
				let agent_ids: std::collections::HashSet<String> =
					current.iter().map(|a| a.agent_id.clone()).collect();

				// 新規 agent が居たら NewAgentAppeared も別途通知
				for a in &current {
					if !known_agent_ids.contains(&a.agent_id) {
						let _ = tx.send(Msg::NewAgentAppeared { agent: a.clone() });
					}
				}

				known_agent_ids = agent_ids;
				let _ = tx.send(Msg::AgentsLoaded {
					session_id: session.session_id.clone(),
					agents: current,
				});
			}
			Err(std_mpsc::RecvTimeoutError::Timeout) => continue,
			Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
		}
	}
}

fn current_agent_ids(
	repo: &FileSystemSessionRepository,
	session: &SessionEntity,
	show_all: bool,
) -> std::collections::HashSet<String> {
	repo.get_sub_agents(session, show_all)
		.into_iter()
		.map(|a| a.agent_id)
		.collect()
}

fn send_agents(
	repo: &FileSystemSessionRepository,
	session: &SessionEntity,
	show_all: bool,
	tx: &UnboundedSender<Msg>,
) {
	let agents = repo.get_sub_agents(session, show_all);
	let _ = tx.send(Msg::AgentsLoaded {
		session_id: session.session_id.clone(),
		agents,
	});
}
