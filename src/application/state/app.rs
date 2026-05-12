//! 画面遷移 state。TS 版 `src/application/state/appReducer.ts` の移植。

use std::collections::HashSet;

/// 現在表示中の画面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppView {
	/// セッション選択画面。
	SessionSelect,
	/// サブエージェント選択画面。
	SubAgentSelect,
	/// 新規エージェント待機画面。
	Waiting,
	/// 監視中 (メッセージ表示)。
	Watching,
}

/// 画面遷移に関する状態。
#[derive(Debug, Clone)]
pub struct AppViewState {
	pub current_view: AppView,
	/// カーソル位置 (SessionSelect / SubAgentSelect で共有)。
	pub cursor_index: usize,
	/// 選択済みセッション ID。
	pub selected_session_id: Option<String>,
	/// 監視中の attach 対象 agent_id 列。
	///
	/// 単数 attach (旧仕様) は要素 1、multi-attach なら 2 件以上、未 attach は空。
	/// `MessagesAppended` のフィルタや WATCHING ヘッダ表示の単数 / 複数判定に
	/// 使う。Vec を採用しているのは、multi-attach でもユーザーが選択した順番
	/// (= SUBAGENT_SELECT のリスト順) を再現したいため。
	pub attached_agent_ids: Vec<String>,
	/// SUBAGENT_SELECT で `Space` トグルされた選択候補 agent_id 集合。
	///
	/// 画面遷移するたびにクリアされる。`Enter` で `[x]` が 1 件以上ならこの
	/// 集合の全件を attach、0 件ならカーソル位置の 1 件を attach する。
	pub selected_agent_ids: HashSet<String>,
	/// 詳細モード (Ctrl+O) の on/off。
	pub is_detailed_mode: bool,
}

impl AppViewState {
	/// `attached_agent_ids` に `agent_id` が含まれるか。`MessagesAppended` の
	/// フィルタで頻繁に呼ばれる。要素数は通常 1〜数個なので線形探索で十分。
	pub fn is_attached(&self, agent_id: &str) -> bool {
		self.attached_agent_ids.iter().any(|id| id == agent_id)
	}

	/// attach 中なら true。`attached_agent_ids` が非空。
	pub fn has_attached_agent(&self) -> bool {
		!self.attached_agent_ids.is_empty()
	}
}

impl Default for AppViewState {
	fn default() -> Self {
		Self {
			current_view: AppView::SessionSelect,
			cursor_index: 0,
			selected_session_id: None,
			attached_agent_ids: Vec::new(),
			selected_agent_ids: HashSet::new(),
			is_detailed_mode: false,
		}
	}
}
