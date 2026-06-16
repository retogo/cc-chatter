//! SUBAGENT_SELECT 画面。

use ratatui::{
	layout::{Constraint, Direction, Layout, Rect},
	style::{Color, Modifier, Style},
	text::{Line, Span},
	widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
	Frame,
};

use crate::application::App;
use crate::ui::format::format_date_time;
use crate::ui::icons::get_agent_icon;

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
	let chunks = Layout::default()
		.direction(Direction::Vertical)
		.constraints([
			Constraint::Length(3),
			Constraint::Min(1),
			Constraint::Length(2),
		])
		.split(area);

	render_header(f, chunks[0], app);
	render_list(f, chunks[1], app);
	render_footer(f, chunks[2], app);
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
	let session_id = app.view.selected_session_id.as_deref().unwrap_or("(none)");
	let header = Paragraph::new(vec![
		Line::from(Span::styled(
			" cc-chatter — Subagent Select ",
			Style::default()
				.fg(Color::Black)
				.bg(Color::Cyan)
				.add_modifier(Modifier::BOLD),
		)),
		Line::from(vec![
			Span::styled("session: ", Style::default().fg(Color::DarkGray)),
			Span::raw(session_id.to_string()),
		]),
	]);
	f.render_widget(header, area);
}

fn render_list(f: &mut Frame, area: Rect, app: &App) {
	let items: Vec<ListItem> = app
		.domain
		.agents
		.iter()
		.map(|a| {
			// Workflow ツール経由の agent は agentType に関わらず 🧩 で区別する
			let icon = if a.workflow_run.is_some() {
				"🧩"
			} else {
				get_agent_icon(&a.agent_type)
			};
			let updated = format_date_time(&a.updated_at, "%H:%M:%S");
			let checked = app.view.selected_agent_ids.contains(&a.agent_id);
			let checkbox_str = if checked { "[x] " } else { "[ ] " };
			let checkbox_style = if checked {
				Style::default()
					.fg(Color::Yellow)
					.add_modifier(Modifier::BOLD)
			} else {
				Style::default().fg(Color::DarkGray)
			};
			// 型カラム: generic な workflow-subagent は導出ロール label を優先表示する
			let label = a.workflow_label.as_deref().unwrap_or(&a.agent_type);

			let mut spans = vec![
				Span::styled(checkbox_str, checkbox_style),
				Span::raw(format!("{icon}  ")),
			];
			// Workflow ツール経由なら行頭に run マーカーを付ける
			if let Some(run) = &a.workflow_run {
				let run_short: String = run.chars().take(8).collect();
				spans.push(Span::styled(
					format!("wf:{run_short} "),
					Style::default().fg(Color::DarkGray),
				));
			}
			spans.push(Span::styled(
				label.to_string(),
				Style::default().fg(Color::Cyan),
			));
			spans.push(Span::raw("  "));
			spans.push(Span::styled(
				a.agent_id.clone(),
				Style::default().fg(Color::Gray),
			));
			spans.push(Span::raw("  "));
			spans.push(Span::styled(updated, Style::default().fg(Color::DarkGray)));

			ListItem::new(Line::from(spans))
		})
		.collect();

	let mut state = ListState::default();
	state.select(Some(
		app.view.cursor_index.min(items.len().saturating_sub(1)),
	));

	let list = List::new(items)
		.block(Block::default().borders(Borders::ALL).title(" Subagents "))
		.highlight_style(
			Style::default()
				.bg(Color::DarkGray)
				.add_modifier(Modifier::BOLD),
		)
		.highlight_symbol("▶ ");

	f.render_stateful_widget(list, area, &mut state);
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
	// 選択件数を可視化。0 件のときは Enter で「カーソル位置の 1 件 attach」を
	// 期待するユーザーがいるので、わざわざ「0」とは出さず Space ヒントだけ出す。
	let selected = app.view.selected_agent_ids.len();
	let selection_hint = if selected == 0 {
		"Space toggle".to_string()
	} else {
		format!("Space toggle ({selected} selected)")
	};
	let text = format!(
		"↑↓ select  {selection_hint}  Enter attach  Ctrl+R refresh  Esc back  Ctrl+C quit      {}",
		app.status
	);
	let footer = Paragraph::new(Line::from(Span::styled(
		text,
		Style::default().fg(Color::DarkGray),
	)));
	f.render_widget(footer, area);
}
