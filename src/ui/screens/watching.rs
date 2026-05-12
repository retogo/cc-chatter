//! WATCHING 画面。
//!
//! ## 行単位クリップ描画フロー
//!
//! 1. 各メッセージを `render_bubble_lines` で `Vec<Line>` に展開
//! 2. 全 Line を連結して「グローバル行番号 → (Line, Sender)」のフラットな列を作る
//! 3. `compute_row_based_viewport` で `effective_offset` と `head_skip_rows`,
//!    `view_rows` を算出
//! 4. フラット列を `effective_offset` から `view_rows` 行だけ取り出して
//!    `draw_bubble_row` で 1 行ずつ Buffer に描画
//! 5. 描画のたびに App の `viewport_cache` (hit items + message_area_y) を
//!    更新して、マウスクリックから参照できるようにする
//!
//! ## State 書き戻し (useEffect 相当)
//!
//! `compute_row_based_viewport` の結果 `effective_offset` +
//! `follow_tail_reached` と ViewportState の `scroll_offset_rows` /
//! `follow_tail` が乖離していたら、描画と同じタイミングで
//! `viewport.set_scroll_offset()` を呼んで書き戻す。reducer は同値ガード
//! しているのでループしない。

use ratatui::{
	layout::{Constraint, Direction, Layout, Rect},
	style::{Color, Modifier, Style},
	text::{Line, Span},
	widgets::{Block, Borders, Paragraph},
	Frame,
};

use crate::application::App;
use crate::ui::components::chat_bubble::{
	compute_bubble_width, compute_content_width, draw_bubble_row, render_bubble_rows,
	ChatBubbleModel,
};
use crate::ui::icons::get_agent_icon;
use crate::ui::layout::{compute_row_based_viewport, HitItem};

/// ヘッダー行数 (render_header が使う行数 + 空行)。マウス座標変換で使う。
pub const HEADER_LINES: u16 = 2;
/// フッター行数。
pub const FOOTER_LINES: u16 = 2;

/// WATCHING 画面の描画結果 (マウスヒット判定 + Tick dirty 判定のため App に保存する分)。
#[derive(Debug, Clone, Default)]
pub struct ViewportCache {
	/// メッセージ領域のターミナル Y 座標 (1-based ベース / 左上の y)。
	pub message_area_y: u16,
	/// メッセージ領域の高さ (行数)。
	pub message_area_h: u16,
	/// メッセージ領域の幅 (行数)。
	pub message_area_w: u16,
	/// メッセージ領域内 (相対行) の描画アイテム。
	pub items: Vec<HitItem>,
	/// 直前のフレームで描画範囲内に **未完了 tool_use** が 1 件以上あったか。
	///
	/// `App::handle_tick` がこのフラグを見て `spinner_phase` を進めるかどうか
	/// 判定する。viewport 外に流れた未完了バブルや、すべて完了済みのケースでは
	/// false になり、Tick で再描画されない (静止画コスト = 0)。
	pub has_inflight_in_view: bool,
}

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
	let chunks = Layout::default()
		.direction(Direction::Vertical)
		.constraints([
			Constraint::Length(HEADER_LINES),
			Constraint::Min(1),
			Constraint::Length(FOOTER_LINES),
		])
		.split(area);

	render_header(f, chunks[0], app);
	render_messages(f, chunks[1], app);
	render_footer(f, chunks[2], app);
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
	// 単数 attach は従来どおり `<icon> <agent_type>  agent: <id>` 表示、
	// multi-attach は `<icon> Multi  agents: <N>` 表示で「複数監視中」と
	// 即座に分かるようにする。icon は単数なら attached_agent_type ベース、
	// multi なら Main アイコンを流用 (色分けは別タスク)。
	let multi = app.view.attached_agent_ids.len() > 1;
	let (agent_label, agent_id_label, icon) = if multi {
		(
			"Multi".to_string(),
			format!("{} agents", app.view.attached_agent_ids.len()),
			get_agent_icon("Main Agent"),
		)
	} else {
		let agent_id = app
			.view
			.attached_agent_ids
			.first()
			.map(String::as_str)
			.unwrap_or("(none)")
			.to_string();
		let agent_type = app
			.attached_agent_type
			.as_deref()
			.unwrap_or("unknown")
			.to_string();
		let icon = get_agent_icon(&agent_type);
		(agent_type, agent_id, icon)
	};
	let agent_type = agent_label.as_str();
	let agent_id = agent_id_label.as_str();

	let total = app.domain.messages.len();
	let current = app.viewport.selected_index.map(|i| i + 1).unwrap_or(0);
	let pos = format!("[{current}/{total}]");

	let all_expanded = !app.domain.messages.is_empty()
		&& app.domain.messages.len() == app.viewport.expanded_ids.len()
		&& app
			.domain
			.messages
			.iter()
			.all(|msg| app.viewport.expanded_ids.contains(&msg.id));
	let detail_indicator = if all_expanded {
		"Ctrl+O: close all"
	} else {
		"Ctrl+O: open all"
	};

	let follow_indicator = if app.viewport.follow_tail {
		"FOLLOW"
	} else {
		"SCROLL"
	};

	let header = Paragraph::new(vec![
		Line::from(vec![
			Span::styled(
				format!(" {icon} {agent_type} "),
				Style::default()
					.fg(Color::Black)
					.bg(Color::Green)
					.add_modifier(Modifier::BOLD),
			),
			Span::raw("  "),
			Span::styled(agent_id, Style::default().fg(Color::Gray)),
			Span::raw("  "),
			Span::styled(pos, Style::default().fg(Color::Cyan)),
			Span::raw("  "),
			Span::styled(detail_indicator, Style::default().fg(Color::Yellow)),
			Span::raw("  "),
			Span::styled(follow_indicator, Style::default().fg(Color::Magenta)),
		]),
		Line::from(""),
	]);
	f.render_widget(header, area);
}

fn render_messages(f: &mut Frame, area: Rect, app: &mut App) {
	if app.domain.messages.is_empty() {
		let empty = Paragraph::new(Line::from(Span::styled(
			"(no messages yet — waiting for subagent output)",
			Style::default().fg(Color::DarkGray),
		)))
		.block(Block::default().borders(Borders::ALL));
		f.render_widget(empty, area);
		app.viewport_cache = ViewportCache {
			message_area_y: area.y,
			message_area_h: area.height,
			message_area_w: area.width,
			items: Vec::new(),
			has_inflight_in_view: false,
		};
		return;
	}

	let all_expanded = app.domain.messages.len() == app.viewport.expanded_ids.len()
		&& app
			.domain
			.messages
			.iter()
			.all(|msg| app.viewport.expanded_ids.contains(&msg.id));

	// chat_mode が Slack のときは bubble 幅 = area_width (全幅)、
	// default / line は area_width の 60% (`compute_bubble_width` 内で分岐)。
	// `content_width` は `compute_content_width` 経由でモード依存に出す:
	// default / slack は `bubble_width - 1` (左 `▏` 1 セル分)、
	// LINE は `bubble_width` そのもの (本文の左右 `│` は内側で処理)。
	// App::current_content_width / HeightsCache::sync もこの同じヘルパーを
	// 参照することで、`╯` と右 `│` がズレない (off-by-one 防止)。
	let bubble_width = compute_bubble_width(area.width, app.chat_mode);
	let content_width = compute_content_width(area.width, app.chat_mode);

	// メッセージ高さは immutable なのでキャッシュ。content_width が変わったとき
	// と MAX_MESSAGES で先頭 drain が起きたときだけ全再計算する。
	//
	// mention prefix (`@Main` / `@{agent_type}`) の表示幅を考慮するため、
	// agent_type_by_id の lookup を closure で渡す。描画側 (ChatBubble) と
	// 同じ agent_type を使うこと (lessons.md の「見積もり == 実描画行数」原則)。
	{
		let agent_type_by_id = &app.agent_type_by_id;
		app.heights_cache.sync(
			&app.domain.messages,
			content_width,
			app.chat_mode,
			|agent_id| {
				agent_type_by_id
					.get(agent_id)
					.cloned()
					.unwrap_or_else(|| "unknown".to_string())
			},
		);
	}

	// 毎フレーム Vec<u16> を alloc しないよう、App 保有のバッファを使い回す。
	let heights = &mut app.heights_buffer;
	heights.clear();
	heights.reserve(app.domain.messages.len());
	// expanded_ids が空のときも hash 計算 (m.id のハッシュ化) を避けるため is_empty() ガードする。
	let expanded_empty = app.viewport.expanded_ids.is_empty();
	for (i, m) in app.domain.messages.iter().enumerate() {
		let is_expanded = !expanded_empty && app.viewport.expanded_ids.contains(&m.id);
		heights.push(app.heights_cache.get(i, is_expanded));
	}

	// auto_follow_selection はキー操作直後のフレームだけ有効 (Issue 007)。
	let auto_follow = app.viewport.consume_pending_follow_selection();

	let slice = compute_row_based_viewport(
		heights,
		area.height,
		app.viewport.scroll_offset_rows,
		app.viewport.selected_index,
		auto_follow,
		app.viewport.follow_tail,
	);

	// --- state 書き戻し (TS の SET_SCROLL_OFFSET useEffect 相当) -----------
	// reducer の同値ガードで差分があるときだけ state を更新する。
	// ストリーミング中は max_offset が毎フレーム増えるため offset 差分だけで
	// dirty を立てると無限再描画ループになる (FINDING-001)。FOLLOW/SCROLL
	// インジケータの真偽反転だけを dirty 対象にする。
	let prev_follow = app.viewport.follow_tail;
	let _ = app
		.viewport
		.set_scroll_offset(slice.effective_offset, slice.follow_tail_reached);
	if app.viewport.follow_tail != prev_follow {
		app.needs_redraw = true;
	}

	// --- 描画対象 bubble を Line 列に展開しつつ直描画 (slice 内のみ) ---------
	// 従来は slice 範囲内の全メッセージを `Vec<(Line, Sender)>` にまとめてから
	// `skip(head_skip_rows).take(view_rows)` で切り出していた。expanded な
	// tool_result (数百行) が末尾に居ると、skip/take で捨てられる行まで
	// 全部 Line として alloc されていた。
	//
	// ここでは message ごとに `render_bubble_lines` を呼んで、その出力を
	// 1 行ずつ skip/draw しながら消費し、view_rows を超えたら残りのメッセージ
	// に対する `render_bubble_lines` 呼び出しをスキップ (break) する。
	// slice 末尾メッセージ 1 件ぶんの Vec alloc は残るが、それ以降の
	// 潜在的な大 alloc は完全に消える。
	//
	// agent_type は App.agent_type_by_id に事前キャッシュ済み (Task #119)。
	let head_skip = slice.head_skip_rows as usize;
	let take_limit = slice.view_rows as usize;
	let area_bottom = area.y.saturating_add(area.height);
	let buf = f.buffer_mut();

	let mut skipped = 0usize;
	let mut drawn = 0usize;
	// viewport 内に未完了 tool_use が見えたか。`App::handle_tick` がこのフラグを
	// 見て spinner_phase を進めるかどうか判定する (画面外の未完了は無視)。
	let mut has_inflight_in_view = false;
	let spinner_phase = app.spinner_phase;
	'outer: for i in slice.start_index..slice.end_index {
		if drawn >= take_limit {
			break;
		}
		let msg = &app.domain.messages[i];
		let is_expanded = !expanded_empty && app.viewport.expanded_ids.contains(&msg.id);
		let is_selected = app.viewport.selected_index == Some(i);
		let agent_type = app
			.agent_type_by_id
			.get(msg.agent_id.as_str())
			.map(String::as_str)
			.unwrap_or("unknown");
		// 未完了 tool_use 判定 (この bubble が viewport 内に少なくとも一部映る場合のみ)。
		if !has_inflight_in_view && msg.tool_use.is_some() && msg.tool_result.is_none() {
			has_inflight_in_view = true;
		}
		let model = ChatBubbleModel {
			message: msg,
			agent_type,
			is_detailed_mode: all_expanded,
			is_expanded,
			is_selected,
		};
		let sender = msg.sender;
		for row in render_bubble_rows(&model, content_width, app.chat_mode, spinner_phase) {
			if skipped < head_skip {
				skipped += 1;
				continue;
			}
			if drawn >= take_limit {
				break 'outer;
			}
			let y = area.y.saturating_add(drawn as u16);
			if y >= area_bottom {
				break 'outer;
			}
			let row_area = Rect {
				x: area.x,
				y,
				width: area.width,
				height: 1,
			};
			draw_bubble_row(row_area, buf, &row, sender, bubble_width, app.chat_mode);
			drawn += 1;
		}
	}

	// --- ヒット判定用キャッシュを更新 ---------------------------------------
	// HitItem は Copy になったので clone 不要 (items 自体は slice が Drop するので
	// move する必要があるが、それは Vec<HitItem> の move で O(1))。
	app.viewport_cache = ViewportCache {
		message_area_y: area.y,
		message_area_h: area.height,
		message_area_w: area.width,
		items: slice.items,
		has_inflight_in_view,
	};
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
	let text = format!(
		"↑↓/jk select  Enter/Space expand  g/G top/bot  Ctrl+D/U half-page  Ctrl+O detail  wheel scroll  click select  Esc back      {}",
		app.status
	);
	let footer = Paragraph::new(Line::from(Span::styled(
		text,
		Style::default().fg(Color::DarkGray),
	)));
	f.render_widget(footer, area);
}
