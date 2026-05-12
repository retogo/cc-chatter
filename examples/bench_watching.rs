use cc_chatter::application::state::HeightsCache;
use cc_chatter::domain::entities::{FormattedMessage, Sender, ToolResult, ToolUse};
use cc_chatter::settings::ChatMode;
use cc_chatter::ui::layout::{compute_row_based_viewport, estimate_bubble_height};
use chrono::Utc;
use std::time::Instant;

fn make_messages(n: usize) -> Vec<FormattedMessage> {
	// 実際の Claude Code ログに近い分量 (tool_result が大きい general-purpose 想定)。
	let long_file_content = (0..200)
		.map(|line| format!("  {line}: some source code fragment with reasonably long content"))
		.collect::<Vec<_>>()
		.join("\n");

	(0..n)
		.map(|i| {
			let sender = if i % 2 == 0 {
				Sender::Sub
			} else {
				Sender::Main
			};
			if i % 3 == 0 {
				FormattedMessage {
					id: format!("m-{i}"),
					sender,
					agent_id: "a1".into(),
					timestamp: Utc::now(),
					text: None,
					tool_use: Some(ToolUse {
						name: "Bash".into(),
						input: serde_json::json!({"command": "echo long command line ".repeat(5)}),
					}),
					tool_result: None,
					tool_use_id: Some(format!("tu-{i}")),
					result_timestamp: None,
					is_final_response: false,
				}
			} else if i % 3 == 1 {
				FormattedMessage {
					id: format!("m-{i}"),
					sender,
					agent_id: "a1".into(),
					timestamp: Utc::now(),
					text: None,
					tool_use: None,
					tool_result: Some(ToolResult {
						content: long_file_content.clone(),
						is_error: false,
					}),
					tool_use_id: Some(format!("tu-orphan-{i}")),
					result_timestamp: Some(Utc::now()),
					is_final_response: false,
				}
			} else {
				FormattedMessage {
					id: format!("m-{i}"),
					sender,
					agent_id: "a1".into(),
					timestamp: Utc::now(),
					text: Some(format!("text message {} with some body", i).repeat(2)),
					tool_use: None,
					tool_result: None,
					tool_use_id: None,
					result_timestamp: None,
					is_final_response: false,
				}
			}
		})
		.collect()
}

fn bench(n: usize, iters: usize, expanded: bool) -> f64 {
	let msgs = make_messages(n);
	let content_width = 60u16;
	let mut total_us: u128 = 0;
	for _ in 0..iters {
		let t0 = Instant::now();
		let heights: Vec<u16> = msgs
			.iter()
			.map(|m| estimate_bubble_height(m, expanded, content_width))
			.collect();
		let _ = compute_row_based_viewport(&heights, 20, 0, Some(n.saturating_sub(1)), true, true);
		total_us += t0.elapsed().as_micros();
	}
	total_us as f64 / iters as f64
}

fn bench_cached(n: usize, iters: usize, expanded: bool) -> f64 {
	let msgs = make_messages(n);
	let content_width = 60u16;
	let mut cache = HeightsCache::default();
	cache.sync(&msgs, content_width, ChatMode::Default, |_| {
		"unknown".to_string()
	}); // 初回 sync (コスト外)
	let mut total_us: u128 = 0;
	let mut buf: Vec<u16> = Vec::with_capacity(n);
	for _ in 0..iters {
		let t0 = Instant::now();
		// 毎フレーム再同期 (追加無ければ no-op) + buf 埋め
		cache.sync(&msgs, content_width, ChatMode::Default, |_| {
			"unknown".to_string()
		});
		buf.clear();
		for i in 0..n {
			buf.push(cache.get(i, expanded));
		}
		let _ = compute_row_based_viewport(&buf, 20, 0, Some(n.saturating_sub(1)), true, true);
		total_us += t0.elapsed().as_micros();
	}
	total_us as f64 / iters as f64
}

fn main() {
	println!("--- without cache ---");
	for n in [100usize, 500, 1000] {
		let us_preview = bench(n, 200, false);
		let us_expanded = bench(n, 200, true);
		println!("messages={n:4} preview_us={us_preview:6.1} expanded_us={us_expanded:6.1}");
	}
	println!("--- with HeightsCache ---");
	for n in [100usize, 500, 1000] {
		let us_preview = bench_cached(n, 200, false);
		let us_expanded = bench_cached(n, 200, true);
		println!("messages={n:4} preview_us={us_preview:6.1} expanded_us={us_expanded:6.1}");
	}
}
