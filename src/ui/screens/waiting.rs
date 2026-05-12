//! WAITING 画面。

use ratatui::{
	layout::{Alignment, Constraint, Direction, Layout, Rect},
	style::{Color, Modifier, Style},
	text::{Line, Span},
	widgets::{Block, Borders, Paragraph},
	Frame,
};

use crate::application::App;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
	let chunks = Layout::default()
		.direction(Direction::Vertical)
		.constraints([
			Constraint::Length(3),
			Constraint::Min(1),
			Constraint::Length(2),
		])
		.split(area);

	let session_id = app.view.selected_session_id.as_deref().unwrap_or("(none)");
	let header = Paragraph::new(vec![
		Line::from(Span::styled(
			" cc-chatter — Waiting for new subagent ",
			Style::default()
				.fg(Color::Black)
				.bg(Color::Yellow)
				.add_modifier(Modifier::BOLD),
		)),
		Line::from(vec![
			Span::styled("session: ", Style::default().fg(Color::DarkGray)),
			Span::raw(session_id.to_string()),
		]),
	]);
	f.render_widget(header, chunks[0]);

	let body = Paragraph::new(vec![
		Line::from(""),
		Line::from(vec![Span::styled(
			"⏳ Waiting for a new subagent to spawn...",
			Style::default().fg(Color::Yellow),
		)]),
		Line::from(""),
		Line::from(Span::styled(
			"Run Claude Code in another pane and ask it to spawn a subagent.",
			Style::default().fg(Color::Gray),
		)),
	])
	.block(Block::default().borders(Borders::ALL))
	.alignment(Alignment::Center);
	f.render_widget(body, chunks[1]);

	let footer = Paragraph::new(Line::from(Span::styled(
		format!("Esc back  Ctrl+C quit      {}", app.status),
		Style::default().fg(Color::DarkGray),
	)));
	f.render_widget(footer, chunks[2]);
}
