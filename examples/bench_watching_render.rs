//! WATCHING 画面の描画 hot path を `TestBackend` で 1000 フレーム回して時間を計測する。
//!
//! Task #118 の expanded bubble (13KB tool_result × 多数) のケースを近似する
//! メッセージセットで、`render_bubble_lines` の alloc と `draw_bubble_row` の
//! 直描画の効果を測る。`cargo run --release --example bench_watching_render`。

use cc_chatter::application::state::AppView;
use cc_chatter::application::App;
use cc_chatter::cli::CliOptions;
use cc_chatter::domain::entities::{AgentEntity, FormattedMessage, Sender, ToolResult, ToolUse};
use cc_chatter::ui::screens::watching;
use chrono::Utc;
use clap::Parser;
use ratatui::{backend::TestBackend, Terminal};
use std::path::PathBuf;
use std::time::Instant;

fn make_messages(n: usize, expanded_len: usize) -> Vec<FormattedMessage> {
	// 13KB 相当の tool_result (general-purpose agent のコード探索結果を想定)
	let long_result = (0..expanded_len)
		.map(|line| {
			format!("  {line:4}: fn foo() {{ /* source line with moderately long body */ }}")
		})
		.collect::<Vec<_>>()
		.join("\n");

	(0..n)
		.map(|i| {
			let sender = if i % 2 == 0 {
				Sender::Sub
			} else {
				Sender::Main
			};
			match i % 3 {
				0 => FormattedMessage {
					id: format!("m-{i}"),
					sender,
					agent_id: "a1".into(),
					timestamp: Utc::now(),
					text: None,
					tool_use: Some(ToolUse {
						name: "Bash".into(),
						input: serde_json::json!({
							"command": "rg -n 'foo' src/ | head -40".repeat(2),
						}),
					}),
					tool_result: None,
					tool_use_id: Some(format!("tu-{i}")),
					result_timestamp: None,
					is_final_response: false,
				},
				1 => FormattedMessage {
					id: format!("m-{i}"),
					sender,
					agent_id: "a1".into(),
					timestamp: Utc::now(),
					text: None,
					tool_use: None,
					tool_result: Some(ToolResult {
						content: long_result.clone(),
						is_error: false,
					}),
					tool_use_id: Some(format!("tu-orphan-{i}")),
					result_timestamp: Some(Utc::now()),
					is_final_response: false,
				},
				_ => FormattedMessage {
					id: format!("m-{i}"),
					sender,
					agent_id: "a1".into(),
					timestamp: Utc::now(),
					text: Some(format!("text message {i} with some body").repeat(3)),
					tool_use: None,
					tool_result: None,
					tool_use_id: None,
					result_timestamp: None,
					is_final_response: false,
				},
			}
		})
		.collect()
}

fn setup_app(n: usize, expanded_len: usize, detail_mode: bool) -> App {
	let options = CliOptions::try_parse_from(["cc-chatter"]).expect("parse");
	let mut app = App::new(options, 40, 120);
	app.view.current_view = AppView::Watching;
	app.view.attached_agent_ids = vec!["a1".to_string()];
	app.view.is_detailed_mode = detail_mode;
	app.domain.agents = vec![AgentEntity {
		agent_id: "a1".to_string(),
		agent_type: "general-purpose".to_string(),
		output_path: PathBuf::from("/tmp/nonexistent-bench"),
		updated_at: Utc::now(),
	}];
	app.agent_type_by_id
		.insert("a1".to_string(), "general-purpose".to_string());
	app.domain.messages = make_messages(n, expanded_len);
	app
}

fn bench_render(n: usize, expanded_len: usize, detail_mode: bool, frames: usize) -> f64 {
	let mut app = setup_app(n, expanded_len, detail_mode);
	let backend = TestBackend::new(120, 40);
	let mut terminal = Terminal::new(backend).expect("terminal");

	// warm-up (1 回だけ)
	terminal
		.draw(|f| watching::render(f, f.area(), &mut app))
		.ok();

	let t0 = Instant::now();
	for _ in 0..frames {
		terminal
			.draw(|f| watching::render(f, f.area(), &mut app))
			.ok();
	}
	let elapsed_us = t0.elapsed().as_micros() as f64;
	elapsed_us / frames as f64
}

fn main() {
	println!("--- WATCHING render bench (TestBackend 120x40) ---");
	for n in [100usize, 500, 1000] {
		for expanded_len in [50usize, 200] {
			let us_preview = bench_render(n, expanded_len, false, 200);
			let us_detail = bench_render(n, expanded_len, true, 200);
			println!(
				"n={n:4}  expanded_len={expanded_len:3}  preview_us={us_preview:7.1}  \
				 detail_us={us_detail:7.1}"
			);
		}
	}
}
