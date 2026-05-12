//! SubAgentWatcher: 指定エージェントの JSONL を監視して FormattedMessage
//! を event_loop に流す。
//!
//! TS 版 `ChokidarSubAgentWatcher` 相当だが、Rust 側は notify::PollWatcher
//! + 別 thread での差分読込で構成する。
//!
//! ## 設計
//!
//! - `SubAgentWatcher::spawn` で blocking thread を起動する
//! - thread 内で `notify::PollWatcher` を 100ms interval で回す
//! - 変更 event が来たら `JsonlParser` で差分を読み、`MessageMapper` で
//!   `FormattedMessage` に変換して `tx` (tokio mpsc) に `Msg::MessagesAppended`
//!   を送信する
//! - 監視開始直後に 1 回「既存行を全部読む」呼び出しを行う (初期同期)
//! - drop シグナルは `CancellationToken` 的な `Arc<AtomicBool>` で行う
//!
//! notify のコールバックは同期スレッドで呼ばれるため、tokio runtime に依存
//! しない `mpsc::UnboundedSender::send` (非 async) をそのまま使える。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use notify::{event::EventKind, Config, PollWatcher, RecursiveMode, Watcher as NotifyWatcher};
use tokio::sync::mpsc::UnboundedSender;

use crate::application::mappers::{to_mapper_outcomes, MapperOutcome};
use crate::application::msg::Msg;
use crate::domain::entities::SubAgentLogEntry;
use crate::infrastructure::parsers::JsonlParser;

/// 起動中のウォッチャーハンドル。drop で background thread を停止させる。
pub struct SubAgentWatcher {
	stop_flag: Arc<AtomicBool>,
	join_handle: Option<JoinHandle<()>>,
}

impl SubAgentWatcher {
	/// ウォッチ開始。指定 agent の JSONL に対して差分読み → `Msg::MessagesAppended`
	/// を `tx` に流す background thread を起動する。
	///
	/// 監視対象ファイルが存在しなくてもエラーにはせず、ファイル作成イベントを
	/// そのまま処理する (新規エージェント待機と同じ経路)。
	pub fn spawn(path: PathBuf, agent_id: String, tx: UnboundedSender<Msg>) -> Self {
		let stop_flag = Arc::new(AtomicBool::new(false));
		let stop_flag_for_thread = stop_flag.clone();
		let join_handle = thread::spawn(move || {
			run_sub_agent_watch_loop(path, agent_id, tx, stop_flag_for_thread);
		});
		Self {
			stop_flag,
			join_handle: Some(join_handle),
		}
	}

	/// 停止フラグを立てて background thread を join する (best-effort)。
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

impl Drop for SubAgentWatcher {
	fn drop(&mut self) {
		self.stop_internal();
	}
}

fn run_sub_agent_watch_loop(
	path: PathBuf,
	agent_id: String,
	tx: UnboundedSender<Msg>,
	stop_flag: Arc<AtomicBool>,
) {
	let (notify_tx, notify_rx) = std_mpsc::channel::<()>();

	// notify::PollWatcher (interval 100ms) を立ち上げる
	let config = Config::default().with_poll_interval(Duration::from_millis(100));
	let notify_tx_cb = notify_tx.clone();
	let watcher_res = PollWatcher::new(
		move |res: notify::Result<notify::Event>| {
			if let Ok(event) = res {
				if matches!(
					event.kind,
					EventKind::Modify(_) | EventKind::Create(_) | EventKind::Any
				) {
					let _ = notify_tx_cb.send(());
				}
			}
		},
		config,
	);

	let mut watcher = match watcher_res {
		Ok(w) => w,
		Err(_) => {
			// watcher 構築失敗: ポーリング無しで初回読みだけやって終了する
			emit_diff(&path, &agent_id, &tx, &mut JsonlParser::new());
			return;
		}
	};

	// ファイル自体が存在しない場合は親ディレクトリを監視する (新規作成を拾う)
	let watch_target: PathBuf = if path.exists() {
		path.clone()
	} else if let Some(parent) = path.parent() {
		parent.to_path_buf()
	} else {
		path.clone()
	};
	// Recursive にすると隣接 agent のログも拾うのでコスト増。NonRecursive で足りる
	if let Err(err) = watcher.watch(&watch_target, RecursiveMode::NonRecursive) {
		let _ = tx.send(Msg::Error(format!(
			"watcher.watch failed for {}: {err}",
			watch_target.display()
		)));
	}

	let mut parser = JsonlParser::new();

	// 初回同期: 既存の行を全て読んでから event 待ちに入る
	emit_diff(&path, &agent_id, &tx, &mut parser);

	loop {
		if stop_flag.load(Ordering::SeqCst) {
			break;
		}
		// 200ms で block して、tick は stop 監視用
		match notify_rx.recv_timeout(Duration::from_millis(200)) {
			Ok(()) => {
				// バースト対策: チャネルに溜まっている分を drain して 1 回だけ読む
				while notify_rx.try_recv().is_ok() {}
				emit_diff(&path, &agent_id, &tx, &mut parser);
			}
			Err(std_mpsc::RecvTimeoutError::Timeout) => continue,
			Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
		}
	}
}

fn emit_diff(path: &Path, agent_id: &str, tx: &UnboundedSender<Msg>, parser: &mut JsonlParser) {
	if !path.exists() {
		return;
	}
	let entries: Vec<SubAgentLogEntry> = parser.read_new_entries(path);
	if entries.is_empty() {
		return;
	}
	let outcomes: Vec<MapperOutcome> = entries.iter().flat_map(to_mapper_outcomes).collect();
	if outcomes.is_empty() {
		return;
	}
	let _ = tx.send(Msg::MessagesAppended {
		agent_id: agent_id.to_string(),
		outcomes,
	});
}
