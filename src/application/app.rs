//! App 本体。3 つの reducer を統合し、`Msg` → 状態更新 + `Vec<Cmd>` を返す。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::application::mappers::MapperOutcome;
use crate::application::msg::{Cmd, Msg};
use crate::application::state::viewport::WHEEL_DELTA_ROWS;
use crate::application::state::{AppView, AppViewState, DomainState, HeightsCache, ViewportState};
use crate::cli::CliOptions;
use crate::domain::entities::{FormattedMessage, Sender, SessionEntity, ToolResult};
use crate::settings::ChatMode;
use crate::ui::screens::watching::ViewportCache;

/// App のトップレベル状態。
pub struct App {
	pub view: AppViewState,
	pub domain: DomainState,
	pub viewport: ViewportState,
	pub options: CliOptions,
	pub status: String,
	/// 直近でターミナルから通知された rows (resize 用)。
	pub terminal_rows: u16,
	pub terminal_cols: u16,
	/// 終了フラグ (event_loop が拾ってループを抜ける)。
	pub should_quit: bool,
	/// 最新描画フレームの WATCHING hit test キャッシュ。
	/// 描画層が毎フレーム更新、マウスイベントハンドラが参照する。
	pub viewport_cache: ViewportCache,
	/// 再描画要求フラグ。`update()` が state を変える可能性のある Msg を
	/// 処理したときだけ true にし、event_loop は `take_needs_redraw()` で
	/// 消費してから `terminal.draw` を呼ぶ。`Msg::Tick` 等の state を変えない
	/// Msg では再描画をスキップする。
	pub needs_redraw: bool,
	/// メッセージ高さキャッシュ (preview / expanded)。`estimate_bubble_height`
	/// は tool_result の content が長いと O(len) で重いので、メッセージが
	/// 追加された瞬間に 1 度計算してキャッシュする。
	pub heights_cache: HeightsCache,
	/// 毎フレームの `compute_row_based_viewport` に渡す Vec<u16> を使い回す。
	/// N 件 × 2 バイトの容量を毎フレーム再アロケーションしないために保持する。
	pub heights_buffer: Vec<u16>,
	/// `agent_id → agent_type` の逆引きキャッシュ。`domain.agents` が更新される
	/// タイミング (`handle_agents_loaded` / `handle_new_agent_appeared`) で
	/// 一緒に同期する。描画層が毎フレーム HashMap を再構築する無駄を避ける。
	pub agent_type_by_id: HashMap<String, String>,
	/// 現在 WATCHING 中エージェントの type。`start_watching` で設定、
	/// `back_to_*` で clear。`render_header` が毎フレーム線形探索する無駄を避ける。
	pub attached_agent_type: Option<String>,
	/// ChatBubble の表示モード。起動時は default、WATCHING 画面で循環切替できる。
	pub chat_mode: ChatMode,
	/// ツール実行中スピナーの現在のフレーム番号 (0..=255 で wrapping)。
	/// `Tick` ハンドラが viewport 内に未完了 tool_use があるときだけ +1 する。
	/// 描画時は `SPINNER_FRAMES[spinner_phase as usize % SPINNER_FRAMES.len()]` で
	/// 該当 Braille 文字を取り出す。
	pub spinner_phase: u8,
}

impl App {
	pub fn new(options: CliOptions, initial_rows: u16, initial_cols: u16) -> Self {
		Self {
			view: AppViewState::default(),
			domain: DomainState::default(),
			viewport: ViewportState::default(),
			options,
			status: String::from("Loading sessions..."),
			terminal_rows: initial_rows.max(1),
			terminal_cols: initial_cols.max(1),
			should_quit: false,
			viewport_cache: ViewportCache::default(),
			// 起動直後は必ず 1 回描画する
			needs_redraw: true,
			heights_cache: HeightsCache::default(),
			heights_buffer: Vec::new(),
			agent_type_by_id: HashMap::new(),
			attached_agent_type: None,
			chat_mode: ChatMode::Default,
			spinner_phase: 0,
		}
	}

	pub fn set_chat_mode(&mut self, chat_mode: ChatMode) {
		if self.chat_mode == chat_mode {
			return;
		}
		self.chat_mode = chat_mode;
		self.heights_cache.clear();
		self.mark_dirty();
	}

	/// event_loop が再描画前に呼ぶ。true を返したら draw、false なら skip。
	pub fn take_needs_redraw(&mut self) -> bool {
		let v = self.needs_redraw;
		self.needs_redraw = false;
		v
	}

	/// 描画層が state を書き換えたときに呼ぶ (e.g. `set_scroll_offset` の
	/// 同値ガードを経た後に差分があったケース)。
	pub fn mark_dirty(&mut self) {
		self.needs_redraw = true;
	}

	/// Tick (100ms 周期) ハンドラ。
	///
	/// viewport 内に未完了 tool_use (= `tool_use=Some && tool_result=None`) が
	/// あるとき **だけ** spinner_phase を +1 + `mark_dirty()` する。`has_inflight_in_view`
	/// は直前の WATCHING 描画で `viewport_cache` に書き込まれる。`has_inflight_in_view=false`
	/// のフレームは spinner_phase を進めない (描画前と一致したまま) ので
	/// `take_needs_redraw()` が false を返し、draw が skip される。
	///
	/// WATCHING 以外の画面では `viewport_cache.items` が空のまま、
	/// `has_inflight_in_view` も false なので副作用なし。
	fn handle_tick(&mut self) {
		if self.viewport_cache.has_inflight_in_view {
			self.spinner_phase = self.spinner_phase.wrapping_add(1);
			self.mark_dirty();
		}
	}

	/// Msg を処理して副作用を返す。
	///
	/// `Msg::Tick` と WATCHING 中の `Msg::AgentsLoaded` は state が変化しない
	/// 経路があるため draw を skip する。他の Msg は handler 到達時点で state が
	/// 変化する可能性ありと見なして `needs_redraw` を立てる。AgentsLoaded は
	/// `handle_agents_loaded` 側で必要なときだけ `mark_dirty()` を呼ぶ
	/// (FINDING-002: MainWatcher の 200ms polling で毎 tick 走るため)。
	pub fn update(&mut self, msg: Msg) -> Vec<Cmd> {
		// 以下の Msg は state が変化しない前提で draw を skip する:
		// - `Tick`: 純粋な時間通知。
		// - `AgentsLoaded`: MainWatcher の 200ms polling で毎周期届くが、
		//   WATCHING では表示に反映しない (FINDING-002)。必要時のみ
		//   `handle_agents_loaded` が `mark_dirty()` を呼ぶ。
		// - `Mouse`: SGR button-event tracking は Moved / Drag / ButtonUp も
		//   イベントとして届く。cc-chatter が処理するのは ScrollUp / ScrollDown /
		//   LeftDown だけなので、ここでは全 Mouse を一旦 non-dirty にして、
		//   `handle_mouse` で処理したケースだけ `mark_dirty()` する
		//   (= 実測で毎秒数十回のマウス移動で full redraw する問題の対策)。
		let dirty = !matches!(msg, Msg::Tick | Msg::AgentsLoaded { .. } | Msg::Mouse(_));
		if dirty {
			self.needs_redraw = true;
		}
		match msg {
			Msg::Key(key) => self.handle_key(key),
			Msg::Mouse(m) => self.handle_mouse(m),
			Msg::Resize { cols, rows } => {
				self.terminal_cols = cols.max(1);
				self.terminal_rows = rows.max(1);
				Vec::new()
			}
			Msg::Tick => {
				self.handle_tick();
				Vec::new()
			}
			Msg::SessionsLoaded(sessions) => self.handle_sessions_loaded(sessions),
			Msg::AgentsLoaded { session_id, agents } => {
				self.handle_agents_loaded(session_id, agents)
			}
			Msg::MessagesAppended { agent_id, outcomes } => {
				self.handle_messages_appended(agent_id, outcomes)
			}
			Msg::NewAgentAppeared { agent } => self.handle_new_agent_appeared(agent),
			Msg::Error(text) => {
				self.status = format!("Error: {text}");
				Vec::new()
			}
		}
	}

	// -----------------------------------------------------------------------
	// Mouse handlers
	// -----------------------------------------------------------------------

	fn handle_mouse(&mut self, event: MouseEvent) -> Vec<Cmd> {
		// マウスは WATCHING 画面でのみ反応する (それ以外ではノイズを無視)
		if !matches!(self.view.current_view, AppView::Watching) {
			return Vec::new();
		}

		// SGR mouse reporting は button-event tracking でも Moved / Drag /
		// ButtonUp のイベントが届く。`update()` 側で全 Mouse を non-dirty に
		// しているので、state を実際に動かしたケースだけここで `mark_dirty()`
		// する (移動だけで full redraw するとカクつく)。
		match event.kind {
			MouseEventKind::ScrollUp => {
				self.viewport.scroll_by_rows(-WHEEL_DELTA_ROWS);
				self.mark_dirty();
			}
			MouseEventKind::ScrollDown => {
				self.viewport.scroll_by_rows(WHEEL_DELTA_ROWS);
				self.mark_dirty();
			}
			MouseEventKind::Down(MouseButton::Left) => {
				self.handle_left_click(event.column, event.row);
				self.mark_dirty();
			}
			// 右クリック / 中クリック / ドラッグ / Up / Moved は描画にも
			// state にも影響しないので dirty を立てない
			_ => {}
		}
		Vec::new()
	}

	fn handle_left_click(&mut self, _column: u16, row: u16) {
		// ターミナル Y (0-based) → メッセージ領域内相対行に変換
		let cache = &self.viewport_cache;
		if cache.message_area_h == 0 {
			return;
		}
		let area_top = cache.message_area_y;
		let area_bottom = area_top.saturating_add(cache.message_area_h);
		if row < area_top || row >= area_bottom {
			return;
		}
		let relative_row = row - area_top;
		let Some(hit) = crate::ui::layout::find_message_at_row(&cache.items, relative_row) else {
			return;
		};
		// hit は Copy なので値を取り出してから借用競合を回避
		let message_index = hit.message_index;
		let total = self.domain.messages.len();
		// message_id は毎フレーム clone せず on-demand で引く (全件 clone 撤廃)
		let Some(message_id) = self
			.domain
			.messages
			.get(message_index)
			.map(|m| m.id.clone())
		else {
			return;
		};
		self.viewport
			.select_or_toggle_by_id(total, &message_id, message_index);
	}

	// -----------------------------------------------------------------------
	// Key handlers
	// -----------------------------------------------------------------------

	fn handle_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
		// Ctrl+C はどこでも抜ける
		if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
			self.should_quit = true;
			return vec![Cmd::Quit];
		}

		match self.view.current_view {
			AppView::SessionSelect => self.handle_key_session_select(key),
			AppView::SubAgentSelect => self.handle_key_subagent_select(key),
			AppView::Waiting => self.handle_key_waiting(key),
			AppView::Watching => self.handle_key_watching(key),
		}
	}

	fn handle_key_session_select(&mut self, key: KeyEvent) -> Vec<Cmd> {
		match key.code {
			KeyCode::Up | KeyCode::Char('k') => {
				if self.view.cursor_index > 0 {
					self.view.cursor_index -= 1;
				}
				Vec::new()
			}
			KeyCode::Down | KeyCode::Char('j') => {
				let max = self.domain.sessions.len().saturating_sub(1);
				if self.view.cursor_index < max {
					self.view.cursor_index += 1;
				}
				Vec::new()
			}
			KeyCode::Enter => self.commit_session_selection(),
			KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
				self.status = "Refreshing sessions...".to_string();
				vec![Cmd::RefreshSessions]
			}
			KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
				let next = !self.view.is_detailed_mode;
				self.view.is_detailed_mode = next;
				if next {
					self.viewport
						.expanded_ids
						.extend(self.domain.messages.iter().map(|msg| msg.id.clone()));
				} else {
					self.viewport.expanded_ids.clear();
				}
				Vec::new()
			}
			_ => Vec::new(),
		}
	}

	fn handle_key_subagent_select(&mut self, key: KeyEvent) -> Vec<Cmd> {
		match key.code {
			KeyCode::Up | KeyCode::Char('k') => {
				if self.view.cursor_index > 0 {
					self.view.cursor_index -= 1;
				}
				Vec::new()
			}
			KeyCode::Down | KeyCode::Char('j') => {
				let max = self.domain.agents.len().saturating_sub(1);
				if self.view.cursor_index < max {
					self.view.cursor_index += 1;
				}
				Vec::new()
			}
			KeyCode::Char(' ') => {
				self.toggle_cursor_agent_selection();
				Vec::new()
			}
			KeyCode::Enter => self.commit_agent_selection(),
			KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
				if let Some(session_id) = self.view.selected_session_id.clone() {
					self.status = "Refreshing agents...".to_string();
					return vec![Cmd::RefreshAgents { session_id }];
				}
				Vec::new()
			}
			KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
				let next = !self.view.is_detailed_mode;
				self.view.is_detailed_mode = next;
				if next {
					self.viewport
						.expanded_ids
						.extend(self.domain.messages.iter().map(|msg| msg.id.clone()));
				} else {
					self.viewport.expanded_ids.clear();
				}
				Vec::new()
			}
			KeyCode::Esc => self.back_to_session_select(),
			_ => Vec::new(),
		}
	}

	fn handle_key_waiting(&mut self, key: KeyEvent) -> Vec<Cmd> {
		match key.code {
			KeyCode::Esc => self.back_to_session_select(),
			KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
				let all_expanded = !self.domain.messages.is_empty()
					&& self.domain.messages.len() == self.viewport.expanded_ids.len()
					&& self
						.domain
						.messages
						.iter()
						.all(|msg| self.viewport.expanded_ids.contains(&msg.id));
				self.view.is_detailed_mode = !all_expanded;
				if all_expanded {
					self.viewport.expanded_ids.clear();
				} else {
					self.viewport.expanded_ids.clear();
					self.viewport
						.expanded_ids
						.extend(self.domain.messages.iter().map(|msg| msg.id.clone()));
				}
				Vec::new()
			}
			_ => Vec::new(),
		}
	}

	fn handle_key_watching(&mut self, key: KeyEvent) -> Vec<Cmd> {
		let total = self.domain.messages.len();
		let page_rows = self.page_rows() as usize;
		match key.code {
			KeyCode::Up | KeyCode::Char('k') => {
				self.viewport.move_up(total);
				Vec::new()
			}
			KeyCode::Down | KeyCode::Char('j') => {
				self.viewport.move_down(total);
				Vec::new()
			}
			KeyCode::Char('g') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
				self.viewport.jump_top(total);
				Vec::new()
			}
			KeyCode::Char('G') => {
				self.viewport.jump_bottom(total);
				Vec::new()
			}
			KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
				self.viewport.page_down(total, page_rows);
				Vec::new()
			}
			KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
				self.viewport.page_up(total, page_rows);
				Vec::new()
			}
			KeyCode::Enter | KeyCode::Char(' ') => {
				if let Some(idx) = self.viewport.selected_index {
					if let Some(msg) = self.domain.messages.get(idx) {
						let id = msg.id.clone();
						self.viewport.toggle_expand(&id);
					}
				}
				Vec::new()
			}
			KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
				let all_expanded = !self.domain.messages.is_empty()
					&& self.domain.messages.len() == self.viewport.expanded_ids.len()
					&& self
						.domain
						.messages
						.iter()
						.all(|msg| self.viewport.expanded_ids.contains(&msg.id));
				self.view.is_detailed_mode = !all_expanded;
				if all_expanded {
					self.viewport.expanded_ids.clear();
				} else {
					self.viewport.expanded_ids.clear();
					self.viewport
						.expanded_ids
						.extend(self.domain.messages.iter().map(|msg| msg.id.clone()));
				}
				Vec::new()
			}
			KeyCode::Char('m') if key.modifiers.is_empty() => {
				self.set_chat_mode(self.chat_mode.cycle());
				self.status = format!("Chat mode: {:?}", self.chat_mode);
				Vec::new()
			}
			KeyCode::Esc => self.back_to_subagent_select(),
			_ => Vec::new(),
		}
	}

	// -----------------------------------------------------------------------
	// Transition helpers
	// -----------------------------------------------------------------------

	fn commit_session_selection(&mut self) -> Vec<Cmd> {
		let Some(session) = self.domain.sessions.get(self.view.cursor_index).cloned() else {
			return Vec::new();
		};
		self.enter_subagent_select(&session)
	}

	fn enter_subagent_select(&mut self, session: &SessionEntity) -> Vec<Cmd> {
		self.view.selected_session_id = Some(session.session_id.clone());
		self.view.cursor_index = 0;
		self.view.selected_agent_ids.clear();
		self.status = format!("Loading agents for session {}...", session.session_id);
		self.view.current_view = AppView::SubAgentSelect;
		vec![Cmd::RefreshAgents {
			session_id: session.session_id.clone(),
		}]
	}

	/// SUBAGENT_SELECT で `Space` が押されたとき、カーソル位置のエージェントの
	/// `selected_agent_ids` 参加状態をトグルする。
	fn toggle_cursor_agent_selection(&mut self) {
		let Some(agent) = self.domain.agents.get(self.view.cursor_index) else {
			return;
		};
		let agent_id = agent.agent_id.clone();
		if !self.view.selected_agent_ids.remove(&agent_id) {
			self.view.selected_agent_ids.insert(agent_id);
		}
	}

	fn commit_agent_selection(&mut self) -> Vec<Cmd> {
		let Some(session_id) = self.view.selected_session_id.clone() else {
			return Vec::new();
		};
		// `[x]` が 1 件以上ならその全件を attach。0 件はカーソル位置 1 件で
		// 既存 UX を維持。並び順は `domain.agents` の順 (一覧画面で見た順)
		// に揃えて WATCHING ヘッダの読みやすさを保つ。
		let agent_ids: Vec<String> = if self.view.selected_agent_ids.is_empty() {
			match self.domain.agents.get(self.view.cursor_index) {
				Some(agent) => vec![agent.agent_id.clone()],
				None => return Vec::new(),
			}
		} else {
			self.domain
				.agents
				.iter()
				.filter(|a| self.view.selected_agent_ids.contains(&a.agent_id))
				.map(|a| a.agent_id.clone())
				.collect()
		};
		self.start_watching(session_id, agent_ids)
	}

	fn start_watching(&mut self, session_id: String, agent_ids: Vec<String>) -> Vec<Cmd> {
		if agent_ids.is_empty() {
			return Vec::new();
		}
		self.view.attached_agent_ids = agent_ids.clone();
		self.view.current_view = AppView::Watching;
		// 単数 attach のみ attached_agent_type をキャッシュ。multi のときは None
		// にしてヘッダ描画側で "Multi (N agents)" 表示に切り替える。
		self.attached_agent_type = if agent_ids.len() == 1 {
			self.agent_type_by_id.get(&agent_ids[0]).cloned()
		} else {
			None
		};
		self.view.selected_agent_ids.clear();
		self.domain.clear_messages();
		self.heights_cache.clear();
		self.viewport.reset();
		self.status = if agent_ids.len() == 1 {
			format!("Attached to agent {}", agent_ids[0])
		} else {
			format!("Attached to {} agents", agent_ids.len())
		};
		vec![Cmd::AttachToAgents {
			session_id,
			agent_ids,
		}]
	}

	fn back_to_session_select(&mut self) -> Vec<Cmd> {
		self.view.current_view = AppView::SessionSelect;
		self.view.selected_session_id = None;
		self.view.attached_agent_ids.clear();
		self.view.selected_agent_ids.clear();
		self.attached_agent_type = None;
		self.view.cursor_index = 0;
		self.domain.agents.clear();
		self.agent_type_by_id.clear();
		self.domain.clear_messages();
		self.heights_cache.clear();
		self.viewport.reset();
		self.status = "Session select".to_string();
		vec![Cmd::Detach]
	}

	fn back_to_subagent_select(&mut self) -> Vec<Cmd> {
		self.view.current_view = AppView::SubAgentSelect;
		self.view.attached_agent_ids.clear();
		self.view.selected_agent_ids.clear();
		self.attached_agent_type = None;
		self.view.cursor_index = 0;
		self.domain.clear_messages();
		self.heights_cache.clear();
		self.viewport.reset();
		self.status = "Subagent select".to_string();
		let mut cmds = vec![Cmd::Detach];
		if let Some(session_id) = self.view.selected_session_id.clone() {
			cmds.push(Cmd::RefreshAgents { session_id });
		}
		cmds
	}

	// -----------------------------------------------------------------------
	// Domain-event handlers
	// -----------------------------------------------------------------------

	fn handle_sessions_loaded(&mut self, sessions: Vec<SessionEntity>) -> Vec<Cmd> {
		let len = sessions.len();
		self.domain.sessions = sessions;
		self.view.cursor_index = self.view.cursor_index.min(len.saturating_sub(1));
		self.status = if len == 0 {
			"No sessions found in workspace.".to_string()
		} else {
			format!("{len} session(s) loaded.")
		};
		Vec::new()
	}

	fn handle_agents_loaded(
		&mut self,
		session_id: String,
		agents: Vec<crate::domain::entities::AgentEntity>,
	) -> Vec<Cmd> {
		// 現在選択中のセッション宛てでなければ無視
		if self.view.selected_session_id.as_deref() != Some(session_id.as_str()) {
			return Vec::new();
		}
		let len = agents.len();
		self.domain.agents = agents;
		// `agent_id → agent_type` のキャッシュを再構築 (Task #119)。
		self.agent_type_by_id = self
			.domain
			.agents
			.iter()
			.map(|a| (a.agent_id.clone(), a.agent_type.clone()))
			.collect();

		// `--latest --agent <id>` 経由で AgentsLoaded 到達前に `start_watching`
		// してしまっていた場合、`attached_agent_type` が None のままヘッダーが
		// "unknown" 表示になる。単数 attach のときだけ補完する (multi-attach は
		// "Multi (N agents)" 表示なので type 不要)。
		if self.attached_agent_type.is_none() && self.view.attached_agent_ids.len() == 1 {
			let attached_id = self.view.attached_agent_ids[0].clone();
			if let Some(t) = self.agent_type_by_id.get(&attached_id).cloned() {
				self.attached_agent_type = Some(t);
				self.mark_dirty();
			}
		}

		// SUBAGENT_SELECT 以外の状態 (WATCHING の再取得など) では画面遷移しない。
		// MainWatcher の 200ms polling 由来のイベントはここに到達するので、
		// ここで redraw を立てるとバックグラウンドで毎 tick 再描画する
		// ことになる (FINDING-002)。mark_dirty は SubAgentSelect 経路のみ。
		if !matches!(self.view.current_view, AppView::SubAgentSelect) {
			return Vec::new();
		}

		self.mark_dirty();

		if len == 0 {
			// 既存エージェント無し → WAITING へ遷移
			self.view.current_view = AppView::Waiting;
			self.status = format!("Waiting for a new subagent in session {session_id}...");
			return vec![Cmd::WaitForNewAgent { session_id }];
		}

		self.view.cursor_index = self.view.cursor_index.min(len - 1);
		self.status = format!("{len} agent(s) available.");
		Vec::new()
	}

	fn handle_messages_appended(
		&mut self,
		agent_id: String,
		outcomes: Vec<MapperOutcome>,
	) -> Vec<Cmd> {
		if !self.view.is_attached(&agent_id) {
			return Vec::new();
		}
		let previous = self.domain.messages.len();
		let content_width = self.current_content_width();

		let mut appended: Vec<FormattedMessage> = Vec::new();
		for outcome in outcomes {
			match outcome {
				MapperOutcome::Append(msg) => {
					// 「最新の Bash はデフォルトで Open」: append タイミングだけ
					// `expanded_ids` に挿入し、以降のトグル操作 (Enter/Space/クリック/
					// Ctrl+O) はユーザーの意思を尊重する。閉じた状態は `expanded_ids`
					// から消えるだけで、新しい Bash が来てもそれ以前の Bash には影響
					// しない (= 過去 latest が手動 open のままなら open 維持)。
					//
					// Bash 限定なのは「実行中の出力を即座に見たい」という UX 仕様。
					// 他のツール (Read/Grep/Web*) はノイズ削減のため closed 既定のまま。
					if msg
						.tool_use
						.as_ref()
						.map(|tu| tu.name == "Bash")
						.unwrap_or(false)
					{
						self.viewport.expanded_ids.insert(msg.id.clone());
					}
					appended.push(msg);
				}
				MapperOutcome::AttachResult {
					tool_use_id,
					content,
					is_error,
					timestamp,
				} => {
					// まず直前 batch の append 中で pair 可能か探す (同一 entry 内で
					// tool_use → tool_result が連続することがあるため)。
					let attach_result = ToolResult {
						content: content.clone(),
						is_error,
					};
					if let Some(msg) = appended
						.iter_mut()
						.rev()
						.find(|m| m.tool_use_id.as_deref() == Some(&tool_use_id))
					{
						// batch 内の append 済みメッセージに直接 merge。
						// heights_cache への sync は後でまとめてやるので invalidate 不要。
						msg.tool_result = Some(attach_result);
						msg.result_timestamp = Some(timestamp);
						continue;
					}

					// batch 内では見つからなかった → 既存 messages を逆順検索 (直近の
					// tool_use が対応する確率が高い)。見つかったら mutate + heights_cache
					// を部分無効化 + redraw 要求。
					let hit_idx = self
						.domain
						.messages
						.iter_mut()
						.enumerate()
						.rev()
						.find(|(_, m)| m.tool_use_id.as_deref() == Some(&tool_use_id))
						.map(|(i, m)| {
							m.tool_result = Some(attach_result);
							m.result_timestamp = Some(timestamp);
							i
						});

					if let Some(idx) = hit_idx {
						if content_width > 0 {
							let msg_ref = &self.domain.messages[idx];
							let at = self
								.agent_type_by_id
								.get(msg_ref.agent_id.as_str())
								.map(String::as_str)
								.unwrap_or("unknown");
							self.heights_cache.invalidate(
								idx,
								msg_ref,
								content_width,
								self.chat_mode,
								at,
							);
						}
						self.mark_dirty();
					} else {
						// 対応 tool_use が無い orphan result。独立メッセージとして
						// fallback push する (先頭 drain 済み / 欠損対策)。
						appended.push(orphan_tool_result_message(
							&tool_use_id,
							content,
							is_error,
							timestamp,
							&agent_id,
						));
					}
				}
			}
		}

		if !appended.is_empty() {
			self.domain.append_messages(appended);
		}
		let current = self.domain.messages.len();
		if current != previous {
			self.viewport.on_messages_appended(previous, current);
		}
		Vec::new()
	}

	/// WATCHING 描画層と同じ `content_width` 計算。heights_cache の部分無効化で
	/// `estimate_bubble_height` を呼ぶときに使う。画面高さ/幅が未取得の場合は 0 を
	/// 返し、invalidate を no-op にする (次フレームの `sync` で全件再計算される)。
	fn current_content_width(&self) -> u16 {
		if self.terminal_cols == 0 {
			return 0;
		}
		// watching.rs が描画時に使う `content_width` と **同じヘルパー** を経由する。
		// モード別の差分 (LINE は bubble_width、default/slack は bubble_width-1) は
		// `compute_content_width` が吸収する。AttachResult の invalidate 経路も
		// これを通すことで、描画側と見積もり側の content_width が常に一致する。
		crate::ui::components::chat_bubble::compute_content_width(
			self.terminal_cols,
			self.chat_mode,
		)
	}

	fn handle_new_agent_appeared(
		&mut self,
		agent: crate::domain::entities::AgentEntity,
	) -> Vec<Cmd> {
		if !matches!(self.view.current_view, AppView::Waiting) {
			return Vec::new();
		}
		let Some(session_id) = self.view.selected_session_id.clone() else {
			return Vec::new();
		};
		let agent_id = agent.agent_id.clone();
		self.agent_type_by_id
			.insert(agent.agent_id.clone(), agent.agent_type.clone());
		self.domain.agents.push(agent);
		self.start_watching(session_id, vec![agent_id])
	}

	// -----------------------------------------------------------------------
	// Startup skip (CLI `--latest` / `--session` / `--agent` の自動遷移用)
	// -----------------------------------------------------------------------

	/// `resolve_startup_state` の結果を初期状態に反映する。
	pub fn apply_startup_skip(
		&mut self,
		session_id: Option<String>,
		agent_id: Option<String>,
	) -> Vec<Cmd> {
		let Some(session_id) = session_id else {
			return Vec::new();
		};
		self.view.selected_session_id = Some(session_id.clone());
		self.view.cursor_index = 0;

		if let Some(agent_id) = agent_id {
			self.start_watching(session_id, vec![agent_id])
		} else {
			self.view.current_view = AppView::Waiting;
			self.status = format!("Waiting for a new subagent in session {session_id}...");
			vec![Cmd::WaitForNewAgent { session_id }]
		}
	}

	/// Ctrl+D/U のページサイズ = メッセージ領域の行数 / 2 (最低 1)。
	fn page_rows(&self) -> u16 {
		// header + footer 合わせて ~4 行想定。実画面と 1-2 行ズレても体感に差が出ない
		self.terminal_rows.saturating_sub(4).max(4)
	}
}

/// orphan tool_result (pair する tool_use が見つからなかった結果) を独立メッセージ
/// として fallback 表示するためのヘルパー。
///
/// id は `orphan-{tool_use_id}-{timestamp_ms}` で採番し、`sender=Sub`、
/// `tool_use` は None のままにする (チャット上は「結果だけ単体で残骸」として
/// 表示される)。先頭 drain 済み / 欠損 tool_use ケースの保険なので、頻度は低い。
fn orphan_tool_result_message(
	tool_use_id: &str,
	content: String,
	is_error: bool,
	timestamp: DateTime<Utc>,
	agent_id: &str,
) -> FormattedMessage {
	FormattedMessage {
		id: format!("orphan-{tool_use_id}-{}", timestamp.timestamp_millis()),
		sender: Sender::Sub,
		agent_id: agent_id.to_string(),
		timestamp,
		text: None,
		tool_use: None,
		tool_result: Some(ToolResult { content, is_error }),
		tool_use_id: Some(tool_use_id.to_string()),
		result_timestamp: Some(timestamp),
		is_final_response: false,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::domain::entities::{Sender, SessionMetadata};
	use chrono::Utc;
	use std::path::PathBuf;

	fn session(id: &str) -> SessionEntity {
		SessionEntity {
			session_id: id.to_string(),
			workspace: "-ws".to_string(),
			file_path: PathBuf::from("/tmp/x.jsonl"),
			updated_at: Utc::now(),
			subagents_dir: PathBuf::from("/tmp/x/subagents"),
			subagent_count: 0,
			metadata: SessionMetadata::default(),
		}
	}

	fn formatted(id: &str) -> crate::domain::entities::FormattedMessage {
		crate::domain::entities::FormattedMessage {
			id: id.to_string(),
			sender: Sender::Sub,
			agent_id: "agent-1".to_string(),
			timestamp: Utc::now(),
			text: Some("hi".to_string()),
			tool_use: None,
			tool_result: None,
			tool_use_id: None,
			result_timestamp: None,
			is_final_response: false,
		}
	}

	/// テストで `Msg::MessagesAppended` を組むためのヘルパ。
	fn append_outcomes<I: IntoIterator<Item = FormattedMessage>>(msgs: I) -> Vec<MapperOutcome> {
		msgs.into_iter().map(MapperOutcome::Append).collect()
	}

	fn base_opts() -> CliOptions {
		CliOptions {
			workspace: None,
			latest: false,
			session: None,
			agent: None,
			since: chrono::Duration::days(7),
			limit: 50,
			show_all: false,
			no_mouse: false,
		}
	}

	#[test]
	fn session_select_arrow_moves_cursor() {
		let mut app = App::new(base_opts(), 30, 80);
		app.domain.sessions = vec![session("a"), session("b")];
		app.update(Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
		assert_eq!(app.view.cursor_index, 1);
	}

	#[test]
	fn session_enter_triggers_refresh_agents_cmd() {
		let mut app = App::new(base_opts(), 30, 80);
		app.domain.sessions = vec![session("sess-1")];
		let cmds = app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
		assert_eq!(app.view.current_view, AppView::SubAgentSelect);
		assert!(matches!(cmds[0], Cmd::RefreshAgents { .. }));
	}

	#[test]
	fn agents_loaded_empty_redirects_to_waiting() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::SubAgentSelect;
		app.view.selected_session_id = Some("s1".to_string());
		let cmds = app.update(Msg::AgentsLoaded {
			session_id: "s1".to_string(),
			agents: vec![],
		});
		assert_eq!(app.view.current_view, AppView::Waiting);
		assert!(matches!(cmds[0], Cmd::WaitForNewAgent { .. }));
	}

	#[test]
	fn watching_enter_toggles_expand_for_selected_message() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".to_string()];
		app.domain.messages = vec![formatted("m-0"), formatted("m-1")];
		app.viewport.selected_index = Some(0);
		app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
		assert!(app.viewport.expanded_ids.contains("m-0"));
	}

	#[test]
	fn watching_ctrl_o_syncs_with_clicked_expansion_state() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".to_string()];
		app.domain.messages = vec![formatted("m-0"), formatted("m-1")];
		app.viewport.selected_index = Some(1);

		app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
		assert!(app.viewport.expanded_ids.contains("m-1"));
		assert!(!app.view.is_detailed_mode);

		app.update(Msg::Key(KeyEvent::new(
			KeyCode::Char('o'),
			KeyModifiers::CONTROL,
		)));
		assert!(app.view.is_detailed_mode);
		assert_eq!(app.viewport.expanded_ids.len(), 2);
		assert!(app.viewport.expanded_ids.contains("m-0"));
		assert!(app.viewport.expanded_ids.contains("m-1"));

		app.update(Msg::Key(KeyEvent::new(
			KeyCode::Char('o'),
			KeyModifiers::CONTROL,
		)));
		assert!(!app.view.is_detailed_mode);
		assert!(app.viewport.expanded_ids.is_empty());
	}

	fn agent_entity(id: &str) -> crate::domain::entities::AgentEntity {
		crate::domain::entities::AgentEntity {
			agent_id: id.to_string(),
			agent_type: "general-purpose".to_string(),
			output_path: PathBuf::from(format!("/tmp/agent-{id}.jsonl")),
			updated_at: Utc::now(),
		}
	}

	#[test]
	fn subagent_select_space_toggles_selection() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::SubAgentSelect;
		app.view.selected_session_id = Some("s1".to_string());
		app.domain.agents = vec![agent_entity("a"), agent_entity("b")];
		app.view.cursor_index = 1;
		// Space で b をトグル ON
		app.update(Msg::Key(KeyEvent::new(
			KeyCode::Char(' '),
			KeyModifiers::NONE,
		)));
		assert!(app.view.selected_agent_ids.contains("b"));
		// もう一度押すと OFF
		app.update(Msg::Key(KeyEvent::new(
			KeyCode::Char(' '),
			KeyModifiers::NONE,
		)));
		assert!(!app.view.selected_agent_ids.contains("b"));
	}

	#[test]
	fn subagent_select_enter_without_selection_attaches_cursor_only() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::SubAgentSelect;
		app.view.selected_session_id = Some("s1".to_string());
		app.domain.agents = vec![agent_entity("a"), agent_entity("b"), agent_entity("c")];
		app.view.cursor_index = 1;
		let cmds = app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
		assert_eq!(app.view.attached_agent_ids, vec!["b".to_string()]);
		match &cmds[0] {
			Cmd::AttachToAgents {
				session_id,
				agent_ids,
			} => {
				assert_eq!(session_id, "s1");
				assert_eq!(agent_ids, &vec!["b".to_string()]);
			}
			other => panic!("expected AttachToAgents, got {other:?}"),
		}
	}

	#[test]
	fn subagent_select_enter_with_multi_selection_attaches_all_in_list_order() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::SubAgentSelect;
		app.view.selected_session_id = Some("s1".to_string());
		app.domain.agents = vec![agent_entity("a"), agent_entity("b"), agent_entity("c")];
		// b と c を Space で選ぶ (順序: c → b で押しても出力は domain.agents 順)
		app.view.cursor_index = 2;
		app.update(Msg::Key(KeyEvent::new(
			KeyCode::Char(' '),
			KeyModifiers::NONE,
		)));
		app.view.cursor_index = 1;
		app.update(Msg::Key(KeyEvent::new(
			KeyCode::Char(' '),
			KeyModifiers::NONE,
		)));
		// Enter で multi-attach
		let cmds = app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
		assert_eq!(
			app.view.attached_agent_ids,
			vec!["b".to_string(), "c".to_string()],
			"並び順は domain.agents の順 (cursor 操作順ではない)"
		);
		assert!(
			app.view.selected_agent_ids.is_empty(),
			"start_watching で selected はクリアされる"
		);
		assert!(
			app.attached_agent_type.is_none(),
			"multi-attach では attached_agent_type は None (ヘッダで Multi 表示する)"
		);
		match &cmds[0] {
			Cmd::AttachToAgents { agent_ids, .. } => {
				assert_eq!(agent_ids, &vec!["b".to_string(), "c".to_string()]);
			}
			other => panic!("expected AttachToAgents, got {other:?}"),
		}
	}

	#[test]
	fn enter_subagent_select_resets_selected_agent_ids() {
		// 別セッションに入り直したときに前回の選択候補が残らないことを確認する
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::SessionSelect;
		app.domain.sessions = vec![session("s1")];
		app.view.selected_agent_ids.insert("ghost".to_string());
		app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
		assert!(app.view.selected_agent_ids.is_empty());
	}

	#[test]
	fn ctrl_c_sets_quit_and_emits_cmd_quit() {
		let mut app = App::new(base_opts(), 30, 80);
		let cmds = app.update(Msg::Key(KeyEvent::new(
			KeyCode::Char('c'),
			KeyModifiers::CONTROL,
		)));
		assert!(app.should_quit);
		assert!(matches!(cmds[0], Cmd::Quit));
	}

	#[test]
	fn esc_from_watching_goes_back_to_subagent_select() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.selected_session_id = Some("s1".to_string());
		let cmds = app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
		assert_eq!(app.view.current_view, AppView::SubAgentSelect);
		assert!(matches!(cmds[0], Cmd::Detach));
	}

	#[test]
	fn apply_startup_skip_to_watching() {
		let mut app = App::new(base_opts(), 30, 80);
		let cmds = app.apply_startup_skip(Some("s1".into()), Some("a1".into()));
		assert_eq!(app.view.current_view, AppView::Watching);
		assert!(matches!(cmds[0], Cmd::AttachToAgents { .. }));
	}

	#[test]
	fn apply_startup_skip_to_waiting() {
		let mut app = App::new(base_opts(), 30, 80);
		let cmds = app.apply_startup_skip(Some("s1".into()), None);
		assert_eq!(app.view.current_view, AppView::Waiting);
		assert!(matches!(cmds[0], Cmd::WaitForNewAgent { .. }));
	}

	/// 回帰テスト: `--latest --agent <id>` で `apply_startup_skip` が走ると、
	/// まだ `AgentsLoaded` が届いていないので `attached_agent_type` は None。
	/// 直後に `AgentsLoaded` が到着したら、WATCHING 画面のヘッダーが正しい
	/// agent_type を表示できるよう `attached_agent_type` を埋め直す。
	#[test]
	fn agents_loaded_fills_attached_agent_type_for_startup_skip_watching() {
		let mut app = App::new(base_opts(), 30, 80);
		// --latest --agent a-1 相当
		app.apply_startup_skip(Some("s1".into()), Some("a-1".into()));
		assert_eq!(app.view.current_view, AppView::Watching);
		assert!(
			app.attached_agent_type.is_none(),
			"cache is empty at startup_skip time"
		);

		// その後 MainWatcher から AgentsLoaded が到着
		let agent = crate::domain::entities::AgentEntity {
			agent_id: "a-1".into(),
			agent_type: "general-purpose".into(),
			output_path: PathBuf::from("/tmp/a.jsonl"),
			updated_at: Utc::now(),
		};
		app.update(Msg::AgentsLoaded {
			session_id: "s1".into(),
			agents: vec![agent],
		});

		assert_eq!(app.attached_agent_type.as_deref(), Some("general-purpose"));
	}

	#[test]
	fn agents_loaded_populates_agent_type_cache() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::SubAgentSelect;
		app.view.selected_session_id = Some("s1".into());
		let agent = crate::domain::entities::AgentEntity {
			agent_id: "a-1".into(),
			agent_type: "Explore".into(),
			output_path: PathBuf::from("/tmp/a.jsonl"),
			updated_at: Utc::now(),
		};
		app.update(Msg::AgentsLoaded {
			session_id: "s1".into(),
			agents: vec![agent],
		});
		assert_eq!(
			app.agent_type_by_id.get("a-1").map(String::as_str),
			Some("Explore")
		);
	}

	#[test]
	fn new_agent_appeared_inserts_into_agent_type_cache() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Waiting;
		app.view.selected_session_id = Some("s1".into());
		let agent = crate::domain::entities::AgentEntity {
			agent_id: "a-new".into(),
			agent_type: "general-purpose".into(),
			output_path: PathBuf::from("/tmp/agent-a-new.jsonl"),
			updated_at: Utc::now(),
		};
		app.update(Msg::NewAgentAppeared { agent });
		assert_eq!(
			app.agent_type_by_id.get("a-new").map(String::as_str),
			Some("general-purpose")
		);
		assert_eq!(app.attached_agent_type.as_deref(), Some("general-purpose"));
	}

	#[test]
	fn back_to_session_select_clears_agent_type_cache() {
		let mut app = App::new(base_opts(), 30, 80);
		app.agent_type_by_id.insert("a-1".into(), "Explore".into());
		app.attached_agent_type = Some("Explore".into());
		app.view.current_view = AppView::Watching;
		app.view.selected_session_id = Some("s1".into());
		app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
		// Watching から Esc は SubAgentSelect 戻りなので agent cache は残る
		// (cache clear は back_to_session_select の責任)
		assert!(app.attached_agent_type.is_none());
		assert_eq!(
			app.agent_type_by_id.get("a-1").map(String::as_str),
			Some("Explore")
		);
		// さらに Esc で SessionSelect に戻ると cache も消える
		app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
		assert!(app.agent_type_by_id.is_empty());
	}

	#[test]
	fn new_agent_appeared_from_waiting_attaches_automatically() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Waiting;
		app.view.selected_session_id = Some("s1".into());
		let agent = crate::domain::entities::AgentEntity {
			agent_id: "a-new".into(),
			agent_type: "Explore".into(),
			output_path: PathBuf::from("/tmp/agent-a-new.jsonl"),
			updated_at: Utc::now(),
		};
		let cmds = app.update(Msg::NewAgentAppeared { agent });
		assert_eq!(app.view.current_view, AppView::Watching);
		assert_eq!(app.view.attached_agent_ids, vec!["a-new".to_string()]);
		assert!(matches!(cmds[0], Cmd::AttachToAgents { .. }));
	}

	// -----------------------------------------------------------------------
	// Mouse handler tests (M3)
	// -----------------------------------------------------------------------

	use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

	fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
		MouseEvent {
			kind,
			column,
			row,
			modifiers: KeyModifiers::NONE,
		}
	}

	#[test]
	fn mouse_wheel_on_watching_scrolls_viewport_without_moving_selection() {
		use crate::application::state::viewport::WHEEL_DELTA_ROWS;

		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".to_string()];
		app.viewport.selected_index = Some(3);
		app.viewport.follow_tail = true;
		app.viewport.scroll_offset_rows = 10;

		app.update(Msg::Mouse(mouse(MouseEventKind::ScrollUp, 5, 5)));
		assert_eq!(
			app.viewport.scroll_offset_rows,
			10 - WHEEL_DELTA_ROWS as u32
		);
		assert_eq!(app.viewport.selected_index, Some(3), "selection must stay");
		assert!(
			!app.viewport.follow_tail,
			"upward scroll clears follow_tail"
		);
	}

	#[test]
	fn mouse_wheel_in_non_watching_views_is_ignored() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::SessionSelect;
		let before = app.viewport.scroll_offset_rows;
		app.update(Msg::Mouse(mouse(MouseEventKind::ScrollDown, 0, 0)));
		assert_eq!(app.viewport.scroll_offset_rows, before);
	}

	#[test]
	fn left_click_selects_message_via_hit_items() {
		use crate::ui::layout::HitItem;
		use crate::ui::screens::watching::ViewportCache;

		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".to_string()];
		app.domain.messages = vec![formatted("m-0"), formatted("m-1"), formatted("m-2")];
		// メッセージ領域は y=2..22 にあるとする。相対行 0..3 = m-0, 3..7 = m-1, 7..12 = m-2
		app.viewport_cache = ViewportCache {
			message_area_y: 2,
			message_area_h: 20,
			message_area_w: 80,
			items: vec![
				HitItem {
					message_index: 0,
					start_row: 0,
					end_row: 3,
				},
				HitItem {
					message_index: 1,
					start_row: 3,
					end_row: 7,
				},
				HitItem {
					message_index: 2,
					start_row: 7,
					end_row: 12,
				},
			],
			has_inflight_in_view: false,
		};

		// クリック位置 row=5 → area 内 relative 3 = m-1
		app.update(Msg::Mouse(mouse(
			MouseEventKind::Down(MouseButton::Left),
			10,
			5,
		)));
		assert_eq!(app.viewport.selected_index, Some(1));

		// 同じ位置 (= 選択中の m-1) を再クリック → 開閉トグル
		app.update(Msg::Mouse(mouse(
			MouseEventKind::Down(MouseButton::Left),
			10,
			5,
		)));
		assert!(app.viewport.expanded_ids.contains("m-1"));
	}

	#[test]
	fn left_click_outside_message_area_is_ignored() {
		use crate::ui::screens::watching::ViewportCache;

		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".to_string()];
		app.domain.messages = vec![formatted("m-0")];
		app.viewport_cache = ViewportCache {
			message_area_y: 2,
			message_area_h: 20,
			message_area_w: 80,
			items: vec![],
			has_inflight_in_view: false,
		};
		// 領域外
		app.update(Msg::Mouse(mouse(
			MouseEventKind::Down(MouseButton::Left),
			10,
			0,
		)));
		assert_eq!(app.viewport.selected_index, None);
	}

	#[test]
	fn right_click_and_drag_are_ignored() {
		use crate::ui::layout::HitItem;
		use crate::ui::screens::watching::ViewportCache;

		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".to_string()];
		app.domain.messages = vec![formatted("m-0")];
		app.viewport_cache = ViewportCache {
			message_area_y: 0,
			message_area_h: 10,
			message_area_w: 80,
			items: vec![HitItem {
				message_index: 0,
				start_row: 0,
				end_row: 3,
			}],
			has_inflight_in_view: false,
		};

		app.update(Msg::Mouse(mouse(
			MouseEventKind::Down(MouseButton::Right),
			10,
			1,
		)));
		assert_eq!(app.viewport.selected_index, None);

		app.update(Msg::Mouse(mouse(
			MouseEventKind::Drag(MouseButton::Left),
			10,
			1,
		)));
		assert_eq!(app.viewport.selected_index, None);
	}

	/// Issue 007 再現テスト: ストリーミング中のホイール上スクロールが毎フレーム
	/// 打ち消されない (pending_follow_selection が立っていないので auto-follow は
	/// 走らず、reducer が固まる)。
	#[test]
	fn streaming_messages_do_not_pull_selection_while_wheel_scrolling() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".to_string()];

		// 新着到着で selection 末尾吸着
		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: append_outcomes((0..5).map(|i| formatted(&format!("m-{i}")))),
		});
		assert_eq!(app.viewport.selected_index, Some(4));
		assert!(app.viewport.follow_tail);

		// 描画層が offset を max に書き戻した体で state を準備
		app.viewport.set_scroll_offset(50, true);

		// ホイール上 → scroll_offset_rows が 1 tick ぶん減る
		use crate::application::state::viewport::WHEEL_DELTA_ROWS;
		app.update(Msg::Mouse(mouse(MouseEventKind::ScrollUp, 5, 5)));
		assert_eq!(
			app.viewport.scroll_offset_rows,
			50 - WHEEL_DELTA_ROWS as u32
		);
		assert!(!app.viewport.follow_tail);
		assert!(!app.viewport.pending_follow_selection);

		// 新着が来ても reducer では selection が動かない (既に Some(4) のため)
		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: append_outcomes([formatted("m-5")]),
		});
		assert_eq!(app.viewport.selected_index, Some(4));
		// 描画層が auto_follow_selection=false で compute するので、offset も
		// 引き戻されない
		assert!(!app.viewport.pending_follow_selection);
	}

	// -----------------------------------------------------------------------
	// needs_redraw dirty flag tests (Issue 008)
	// -----------------------------------------------------------------------

	#[test]
	fn initial_app_starts_dirty() {
		let mut app = App::new(base_opts(), 30, 80);
		assert!(app.take_needs_redraw(), "first frame must draw");
		assert!(!app.take_needs_redraw(), "second take returns false");
	}

	#[test]
	fn tick_message_does_not_mark_dirty() {
		let mut app = App::new(base_opts(), 30, 80);
		let _ = app.take_needs_redraw(); // drain initial
		app.update(Msg::Tick);
		assert!(
			!app.take_needs_redraw(),
			"tick without state change must not request redraw"
		);
	}

	#[test]
	fn key_message_marks_dirty() {
		let mut app = App::new(base_opts(), 30, 80);
		let _ = app.take_needs_redraw();
		app.update(Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
		assert!(app.take_needs_redraw(), "key input must request redraw");
	}

	#[test]
	fn messages_appended_marks_dirty() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".into()];
		let _ = app.take_needs_redraw();
		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: append_outcomes([formatted("m-0")]),
		});
		assert!(app.take_needs_redraw());
	}

	#[test]
	fn agents_loaded_during_watching_does_not_mark_dirty() {
		// FINDING-002: MainWatcher の 200ms polling で届く AgentsLoaded は
		// WATCHING 中は画面に影響しないので再描画をスキップする。
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.selected_session_id = Some("s1".into());
		let _ = app.take_needs_redraw();
		app.update(Msg::AgentsLoaded {
			session_id: "s1".into(),
			agents: vec![],
		});
		assert!(
			!app.take_needs_redraw(),
			"AgentsLoaded while watching must not request redraw"
		);
	}

	#[test]
	fn mouse_moved_on_watching_does_not_mark_dirty() {
		// マウスの Moved / Drag / Up / 右クリック等は handle_mouse で処理
		// しないので、update() でも dirty を立てない (Issue: マウス移動だけで
		// full redraw するとカクつく)。
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".into()];
		let _ = app.take_needs_redraw();

		for kind in [
			MouseEventKind::Moved,
			MouseEventKind::Up(MouseButton::Left),
			MouseEventKind::Drag(MouseButton::Left),
			MouseEventKind::Down(MouseButton::Right),
			MouseEventKind::Down(MouseButton::Middle),
		] {
			app.update(Msg::Mouse(mouse(kind, 5, 5)));
			assert!(
				!app.take_needs_redraw(),
				"mouse kind {kind:?} must not request redraw"
			);
		}
	}

	#[test]
	fn mouse_wheel_on_watching_marks_dirty() {
		// ScrollUp / ScrollDown / LeftDown は handle_mouse で state を動かす
		// ので dirty を立てる必要がある。
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".into()];
		let _ = app.take_needs_redraw();

		app.update(Msg::Mouse(mouse(MouseEventKind::ScrollUp, 5, 5)));
		assert!(app.take_needs_redraw(), "ScrollUp must request redraw");

		app.update(Msg::Mouse(mouse(MouseEventKind::ScrollDown, 5, 5)));
		assert!(app.take_needs_redraw(), "ScrollDown must request redraw");
	}

	#[test]
	fn agents_loaded_on_subagent_select_marks_dirty() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::SubAgentSelect;
		app.view.selected_session_id = Some("s1".into());
		let _ = app.take_needs_redraw();
		app.update(Msg::AgentsLoaded {
			session_id: "s1".into(),
			agents: vec![],
		});
		assert!(
			app.take_needs_redraw(),
			"AgentsLoaded on SubAgentSelect must request redraw"
		);
	}

	#[test]
	fn mark_dirty_sets_flag() {
		let mut app = App::new(base_opts(), 30, 80);
		let _ = app.take_needs_redraw();
		assert!(!app.take_needs_redraw());
		app.mark_dirty();
		assert!(app.take_needs_redraw());
	}

	/// キーで選択を動かした直後のフレームだけ auto-follow が有効 (pending フラグ)。
	#[test]
	fn key_nav_sets_pending_follow_which_drains_after_consume() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".into()];
		app.domain.messages = (0..10).map(|i| formatted(&format!("m-{i}"))).collect();
		app.viewport.selected_index = Some(9);
		app.viewport.follow_tail = true;

		// j で下移動 (selection は末尾で止まる) → pending はそのまま false (変化なし)
		app.update(Msg::Key(KeyEvent::new(
			KeyCode::Char('j'),
			KeyModifiers::NONE,
		)));
		assert_eq!(app.viewport.selected_index, Some(9));

		// k で上移動 → pending=true
		app.update(Msg::Key(KeyEvent::new(
			KeyCode::Char('k'),
			KeyModifiers::NONE,
		)));
		assert_eq!(app.viewport.selected_index, Some(8));
		assert!(app.viewport.pending_follow_selection);

		// 描画層が consume → false に落ちる
		let was = app.viewport.consume_pending_follow_selection();
		assert!(was);
		assert!(!app.viewport.pending_follow_selection);
	}

	// -----------------------------------------------------------------------
	// MapperOutcome pairing tests (tool_use ↔ tool_result aggregation)
	// -----------------------------------------------------------------------

	use crate::domain::entities::ToolUse;

	fn tool_use_msg(id: &str, tool_use_id: &str, name: &str) -> FormattedMessage {
		FormattedMessage {
			id: id.to_string(),
			sender: Sender::Sub,
			agent_id: "agent-1".to_string(),
			timestamp: Utc::now(),
			text: None,
			tool_use: Some(ToolUse {
				name: name.to_string(),
				input: serde_json::json!({"command": "echo hi"}),
			}),
			tool_result: None,
			tool_use_id: Some(tool_use_id.to_string()),
			result_timestamp: None,
			is_final_response: false,
		}
	}

	fn attach(tool_use_id: &str, content: &str, is_error: bool) -> MapperOutcome {
		MapperOutcome::AttachResult {
			tool_use_id: tool_use_id.to_string(),
			content: content.to_string(),
			is_error,
			timestamp: Utc::now(),
		}
	}

	/// 同一 batch 内で `Append(tool_use)` 直後に `AttachResult` が来たら、
	/// messages は 1 件 (統合バブル) として残る。
	#[test]
	fn pairing_within_same_batch_collapses_into_one_message() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".into()];

		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: vec![
				MapperOutcome::Append(tool_use_msg("m-0", "tu-1", "Bash")),
				attach("tu-1", "ok", false),
			],
		});

		assert_eq!(app.domain.messages.len(), 1, "tool_use+result merged");
		let m = &app.domain.messages[0];
		assert!(m.tool_use.is_some());
		let tr = m.tool_result.as_ref().expect("result attached");
		assert_eq!(tr.content, "ok");
		assert!(!tr.is_error);
		assert!(m.result_timestamp.is_some());
	}

	/// tool_use を先に push したあと別 batch で AttachResult が来ると、
	/// 既存 message が mutate される + heights_cache が部分無効化される + dirty が立つ。
	#[test]
	fn attach_result_in_later_batch_mutates_existing_message_and_invalidates_cache() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".into()];
		app.terminal_cols = 80;

		// batch 1: tool_use 単独
		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: vec![MapperOutcome::Append(tool_use_msg("m-0", "tu-x", "Read"))],
		});
		assert_eq!(app.domain.messages.len(), 1);
		assert!(app.domain.messages[0].tool_result.is_none());

		// 描画層を経由して heights_cache を初期化させる
		app.heights_cache.sync(
			&app.domain.messages,
			app.current_content_width(),
			app.chat_mode,
			|_| "unknown".to_string(),
		);
		assert_eq!(app.heights_cache.len(), 1);
		let h_before = app.heights_cache.get(0, true);

		let _ = app.take_needs_redraw(); // initial dirty を吸収

		// batch 2: AttachResult のみ。content を意図的に長くして高さが変わるよう
		// にする (invalidate されたかの間接観測になる)。
		let long = "x".repeat(800);
		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: vec![attach("tu-x", &long, false)],
		});

		assert_eq!(app.domain.messages.len(), 1, "no extra message appended");
		let m = &app.domain.messages[0];
		assert!(
			m.tool_use.is_some() && m.tool_result.is_some(),
			"merged into single bubble"
		);
		assert!(app.take_needs_redraw(), "AttachResult must request redraw");

		// 部分無効化が走ったので expanded 高さが大きくなっているはず
		// (統合バブルは tool_use + tool_result 全文ぶん expanded で行数が伸びる)
		let h_after = app.heights_cache.get(0, true);
		assert!(
			h_after > h_before,
			"heights_cache must be invalidated and recomputed: {h_before} -> {h_after}"
		);
	}

	/// 並列 tool_use → 結果が tool_use_id 一致でそれぞれの bubble に attach される。
	#[test]
	fn parallel_tool_uses_each_get_their_own_result() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".into()];

		// 2 つの tool_use を立てる
		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: vec![
				MapperOutcome::Append(tool_use_msg("m-0", "tu-A", "Read")),
				MapperOutcome::Append(tool_use_msg("m-1", "tu-B", "Read")),
			],
		});
		assert_eq!(app.domain.messages.len(), 2);

		// 順不同で結果が到着
		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: vec![
				attach("tu-B", "result-B", false),
				attach("tu-A", "result-A", false),
			],
		});

		assert_eq!(app.domain.messages.len(), 2, "no fallback push happened");
		let m_a = app
			.domain
			.messages
			.iter()
			.find(|m| m.tool_use_id.as_deref() == Some("tu-A"))
			.unwrap();
		assert_eq!(m_a.tool_result.as_ref().unwrap().content, "result-A");
		let m_b = app
			.domain
			.messages
			.iter()
			.find(|m| m.tool_use_id.as_deref() == Some("tu-B"))
			.unwrap();
		assert_eq!(m_b.tool_result.as_ref().unwrap().content, "result-B");
	}

	/// 対応 tool_use が無い orphan tool_result は独立メッセージとして fallback push。
	#[test]
	fn orphan_attach_result_falls_back_to_standalone_message() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".into()];

		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: vec![attach("tu-missing", "lonely", true)],
		});

		assert_eq!(app.domain.messages.len(), 1);
		let m = &app.domain.messages[0];
		assert!(m.tool_use.is_none());
		let tr = m.tool_result.as_ref().expect("orphan result kept");
		assert_eq!(tr.content, "lonely");
		assert!(tr.is_error);
		assert!(m.id.starts_with("orphan-tu-missing-"));
		assert_eq!(m.tool_use_id.as_deref(), Some("tu-missing"));
	}

	/// AttachResult は他エージェント宛て messages にしか到達しない (alien
	/// agent_id の Msg は handle_messages_appended が早期 return)。
	#[test]
	fn messages_appended_for_other_agent_is_ignored() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".into()];
		app.update(Msg::MessagesAppended {
			agent_id: "wrong-agent".into(),
			outcomes: vec![MapperOutcome::Append(tool_use_msg("m-0", "tu-z", "Bash"))],
		});
		assert!(app.domain.messages.is_empty());
	}

	/// 既存 tool_use と batch 内 tool_use の両方が同じ tool_use_id の場合、
	/// batch 内 (より新しい) を優先する。
	#[test]
	fn pairing_prefers_in_batch_message_over_existing_when_id_collides() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".into()];

		// 既存
		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: vec![MapperOutcome::Append(tool_use_msg("m-0", "tu-dup", "Bash"))],
		});

		// 同じ tool_use_id が batch 内で再登場 + result。
		app.update(Msg::MessagesAppended {
			agent_id: "a1".into(),
			outcomes: vec![
				MapperOutcome::Append(tool_use_msg("m-1", "tu-dup", "Bash")),
				attach("tu-dup", "to-newer", false),
			],
		});

		assert_eq!(app.domain.messages.len(), 2);
		// 既存 (idx=0) は結果が attach されない
		assert!(app.domain.messages[0].tool_result.is_none());
		// batch 内 (idx=1) に attach
		assert_eq!(
			app.domain.messages[1].tool_result.as_ref().unwrap().content,
			"to-newer"
		);
	}

	// -----------------------------------------------------------------------
	// Tick / spinner 制御
	// -----------------------------------------------------------------------

	/// `viewport_cache.has_inflight_in_view=false` のとき Tick で何も起きない
	/// (spinner_phase 変化なし、dirty も立たない)。スピナー dirty 制御の根幹。
	#[test]
	fn tick_does_not_advance_spinner_when_no_inflight_in_view() {
		let mut app = App::new(base_opts(), 30, 80);
		app.viewport_cache.has_inflight_in_view = false;
		// 初期描画フラグを 1 度消費する
		app.take_needs_redraw();
		let before = app.spinner_phase;
		app.update(Msg::Tick);
		assert_eq!(
			app.spinner_phase, before,
			"phase must stay when nothing in-flight in view"
		);
		assert!(
			!app.take_needs_redraw(),
			"Tick without in-flight must not mark dirty"
		);
	}

	/// `has_inflight_in_view=true` のとき Tick で spinner_phase が +1 + dirty。
	#[test]
	fn tick_advances_spinner_and_marks_dirty_when_inflight_in_view() {
		let mut app = App::new(base_opts(), 30, 80);
		app.viewport_cache.has_inflight_in_view = true;
		app.take_needs_redraw();
		let before = app.spinner_phase;
		app.update(Msg::Tick);
		assert_eq!(
			app.spinner_phase,
			before.wrapping_add(1),
			"phase must advance by 1 on Tick when in-flight visible"
		);
		assert!(
			app.take_needs_redraw(),
			"Tick with in-flight must mark dirty so the next draw redraws spinner"
		);
	}

	/// spinner_phase は u8 wrapping。255 → 0 で panic しない。
	#[test]
	fn tick_spinner_phase_wraps_around_u8() {
		let mut app = App::new(base_opts(), 30, 80);
		app.viewport_cache.has_inflight_in_view = true;
		app.spinner_phase = 255;
		app.take_needs_redraw();
		app.update(Msg::Tick);
		assert_eq!(app.spinner_phase, 0, "u8 wrapping_add must wrap to 0");
	}

	// -----------------------------------------------------------------------
	// 「最新の Bash はデフォルトで Open」自動展開
	// -----------------------------------------------------------------------

	/// Bash の tool_use bubble が append されると、その id が `expanded_ids` に
	/// 自動挿入される。これにより closed では省略される result セクションが
	/// 即座に展開され、結果が到着した瞬間に view に乗る (= リアルタイム表示)。
	#[test]
	fn appended_bash_tool_use_is_auto_expanded() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["agent-1".into()];

		app.update(Msg::MessagesAppended {
			agent_id: "agent-1".into(),
			outcomes: vec![MapperOutcome::Append(tool_use_msg(
				"m-bash", "tu-1", "Bash",
			))],
		});

		assert!(
			app.viewport.expanded_ids.contains("m-bash"),
			"Bash tool_use must be auto-expanded on append"
		);
	}

	/// Bash 以外の tool_use (Read / Grep / WebFetch 等) は自動展開されない
	/// (closed のままで result が省略される)。
	#[test]
	fn appended_non_bash_tool_use_stays_collapsed() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["agent-1".into()];

		app.update(Msg::MessagesAppended {
			agent_id: "agent-1".into(),
			outcomes: vec![
				MapperOutcome::Append(tool_use_msg("m-read", "tu-r", "Read")),
				MapperOutcome::Append(tool_use_msg("m-grep", "tu-g", "Grep")),
			],
		});

		assert!(!app.viewport.expanded_ids.contains("m-read"));
		assert!(!app.viewport.expanded_ids.contains("m-grep"));
	}

	/// 新しい Bash bubble が append されても、過去の Bash の expanded 状態は
	/// 変更しない (ユーザーが手動で閉じた / 開けた状態を尊重する)。
	#[test]
	fn new_bash_does_not_alter_past_bash_expansion_state() {
		let mut app = App::new(base_opts(), 30, 80);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["agent-1".into()];

		// 1 つ目の Bash → 自動 expanded
		app.update(Msg::MessagesAppended {
			agent_id: "agent-1".into(),
			outcomes: vec![MapperOutcome::Append(tool_use_msg("m-b1", "tu-1", "Bash"))],
		});
		assert!(app.viewport.expanded_ids.contains("m-b1"));

		// ユーザー操作相当: 1 つ目を手動 collapse
		app.viewport.toggle_expand("m-b1");
		assert!(!app.viewport.expanded_ids.contains("m-b1"));

		// 2 つ目の Bash → 2 つ目だけ expanded、1 つ目は collapse のまま
		app.update(Msg::MessagesAppended {
			agent_id: "agent-1".into(),
			outcomes: vec![MapperOutcome::Append(tool_use_msg("m-b2", "tu-2", "Bash"))],
		});
		assert!(app.viewport.expanded_ids.contains("m-b2"));
		assert!(
			!app.viewport.expanded_ids.contains("m-b1"),
			"past Bash collapsed by user must stay collapsed",
		);
	}
}
