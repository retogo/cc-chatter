//! Message / Command 定義 (Elm アーキテクチャ)。
//!
//! `Msg` は event_loop が tokio::select! で集約して App に渡すイベント。
//! `Cmd` は App の update が副作用を要求するときに返す記述子。event_loop 側が
//! `Cmd::run(tx)` を spawn し、結果を `Msg` として mpsc に戻す。

use crossterm::event::{KeyEvent, MouseEvent};

use crate::application::mappers::MapperOutcome;
use crate::domain::entities::{AgentEntity, SessionEntity};

/// App が受け取るイベント。
#[derive(Debug, Clone)]
pub enum Msg {
	/// 端末キー入力。
	Key(KeyEvent),
	/// マウスイベント (M2 では WATCHING でも no-op)。
	Mouse(MouseEvent),
	/// リサイズ通知。
	Resize { cols: u16, rows: u16 },
	/// 250ms tick (末尾追従やリトライの trigger)。
	Tick,

	// -------------------------------------------------------------------
	// Domain / I/O 由来
	// -------------------------------------------------------------------
	/// セッション一覧のロード完了。
	SessionsLoaded(Vec<SessionEntity>),
	/// 指定セッションのサブエージェント一覧ロード完了。
	AgentsLoaded {
		session_id: String,
		agents: Vec<AgentEntity>,
	},
	/// 対象エージェントの新規メッセージ追記 (mapper outcome の列)。
	///
	/// `Append` は末尾に追加、`AttachResult` は既存メッセージに tool_result を
	/// merge する (アプリ層 `handle_messages_appended` が適用)。Slack スレッド
	/// のような pairing を実現するため、watcher からは **outcome 列** を渡す
	/// (`FormattedMessage` 列ではない)。
	MessagesAppended {
		agent_id: String,
		outcomes: Vec<MapperOutcome>,
	},
	/// 新規サブエージェントが誕生した (WAITING → WATCHING 自動遷移用)。
	NewAgentAppeared { agent: AgentEntity },
	/// エラー通知 (status に出す)。
	Error(String),
}

/// App が event_loop に依頼する副作用。
#[derive(Debug, Clone)]
pub enum Cmd {
	/// セッション一覧を再取得。
	RefreshSessions,
	/// 指定セッションのサブエージェント一覧を再取得。
	RefreshAgents { session_id: String },
	/// 1 つ以上のエージェントへアタッチ (watcher 起動)。
	///
	/// 単数 attach は `agent_ids` に 1 件、multi-attach は 2 件以上を渡す。
	/// event_loop は既存 watcher を全て detach してから、`agent_ids` の各 ID
	/// に対して `SubAgentWatcher` を起動する。
	AttachToAgents {
		session_id: String,
		agent_ids: Vec<String>,
	},
	/// 現在の watcher を解除。
	Detach,
	/// 新規サブエージェント待機を開始。
	WaitForNewAgent { session_id: String },
	/// プロセスを終了する。
	Quit,
}
