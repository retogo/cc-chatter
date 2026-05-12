//! SESSION_SELECT 画面。
//!
//! TS 版 `SessionSelectScreen` + `SessionSelect` の最小移植。
//! セッション一覧を上下キーで選択し、Enter で次の画面へ。

use ratatui::{
	layout::{Constraint, Direction, Layout, Rect},
	style::{Color, Modifier, Style},
	text::{Line, Span},
	widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
	Frame,
};

use crate::application::App;
use crate::domain::services::get_display_text;
use crate::ui::format::format_date_time;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
	let chunks = Layout::default()
		.direction(Direction::Vertical)
		.constraints([
			Constraint::Length(3), // header
			Constraint::Min(1),    // list
			Constraint::Length(2), // footer
		])
		.split(area);

	render_header(f, chunks[0], app);
	render_list(f, chunks[1], app);
	render_footer(f, chunks[2], app);
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
	let workspace_text = app
		.options
		.workspace
		.as_deref()
		.unwrap_or("(current directory)");
	let header = Paragraph::new(vec![
		Line::from(vec![Span::styled(
			" cc-chatter — Session Select ",
			Style::default()
				.fg(Color::Black)
				.bg(Color::Cyan)
				.add_modifier(Modifier::BOLD),
		)]),
		Line::from(vec![
			Span::styled("workspace: ", Style::default().fg(Color::DarkGray)),
			Span::raw(workspace_text),
		]),
	])
	.block(Block::default().borders(Borders::NONE));
	f.render_widget(header, area);
}

fn render_list(f: &mut Frame, area: Rect, app: &App) {
	// 行ヘッダの幅 (updated 16 + 2 spaces + "[ N ]" 7 + 2 spaces = 27 列) を
	// 概算で除き、本文の最大長を決める。N が 1000 以上のときは "[ 9999 ]" の
	// 8 列まで膨らむが希少なので bubble させる。
	let header_cols = 27usize;
	let body_max = (area.width as usize).saturating_sub(header_cols + 2).max(1);
	let items: Vec<ListItem> = app
		.domain
		.sessions
		.iter()
		.map(|s| {
			let text = get_display_text(&s.metadata);
			let truncated: String = text.chars().take(body_max).collect();
			let updated = format_date_time(&s.updated_at, "%Y-%m-%d %H:%M");
			let count_str = format!("{:>3}", s.subagent_count);
			ListItem::new(Line::from(vec![
				Span::styled(format!("{updated}  "), Style::default().fg(Color::DarkGray)),
				Span::styled("[ ", Style::default().fg(Color::DarkGray)),
				Span::styled(
					count_str,
					Style::default()
						.fg(Color::Yellow)
						.add_modifier(Modifier::BOLD),
				),
				Span::styled(" ]  ", Style::default().fg(Color::DarkGray)),
				Span::raw(truncated),
			]))
		})
		.collect();

	let mut state = ListState::default();
	state.select(Some(
		app.view.cursor_index.min(items.len().saturating_sub(1)),
	));

	let list = List::new(items)
		.block(Block::default().borders(Borders::ALL).title(" Sessions "))
		.highlight_style(
			Style::default()
				.bg(Color::DarkGray)
				.add_modifier(Modifier::BOLD),
		)
		.highlight_symbol("▶ ");

	f.render_stateful_widget(list, area, &mut state);
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
	let text = format!(
		"↑↓ select  Enter attach  Ctrl+R refresh  Ctrl+C quit      {}",
		app.status
	);
	let footer = Paragraph::new(Line::from(Span::styled(
		text,
		Style::default().fg(Color::DarkGray),
	)))
	.block(Block::default().borders(Borders::NONE));
	f.render_widget(footer, area);
}
