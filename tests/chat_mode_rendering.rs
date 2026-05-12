//! ChatBubble の 3 モードが **実際の WATCHING 画面描画** を
//! どう変えるかを end-to-end で検証する。
//!
//! unit test (`src/ui/components/chat_bubble.rs`) は `render_bubble_rows` /
//! `estimate_bubble_height_*` 単体の行数・色検証をカバーしているが、
//! `watching::render` を通して実 Buffer に出るときに、
//!
//! - `default` / `line` で Main=右寄せ、Sub=左寄せ
//! - `slack` で Main / Sub 問わず左寄せ
//! - `line` で bubble の最上行に `╭`、最下行に `╰` が現れる
//!
//! が維持されることを TestBackend 経由で確認する。

use cc_chatter::application::state::AppView;
use cc_chatter::application::App;
use cc_chatter::cli::CliOptions;
use cc_chatter::domain::entities::{AgentEntity, FormattedMessage, Sender};
use cc_chatter::settings::ChatMode;
use cc_chatter::ui::screens::watching;
use chrono::Utc;
use clap::Parser;
use ratatui::{backend::TestBackend, Terminal};
use std::path::PathBuf;

fn setup_app_with_mode(chat_mode: ChatMode) -> App {
	let options = CliOptions::try_parse_from(["cc-chatter"]).expect("parse");
	let mut app = App::new(options, 30, 100);
	app.set_chat_mode(chat_mode);
	app.view.current_view = AppView::Watching;
	app.view.attached_agent_ids = vec!["a1".to_string()];
	app.domain.agents = vec![AgentEntity {
		agent_id: "a1".to_string(),
		agent_type: "general-purpose".to_string(),
		output_path: PathBuf::from("/tmp/nonexistent"),
		updated_at: Utc::now(),
	}];
	app.agent_type_by_id
		.insert("a1".to_string(), "general-purpose".to_string());
	app.attached_agent_type = Some("general-purpose".to_string());
	app.domain.messages = vec![
		FormattedMessage {
			id: "m-0".to_string(),
			sender: Sender::Main,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: Some("please investigate".to_string()),
			tool_use: None,
			tool_result: None,
			tool_use_id: None,
			result_timestamp: None,
			is_final_response: false,
		},
		FormattedMessage {
			id: "m-1".to_string(),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: Some("working on it".to_string()),
			tool_use: None,
			tool_result: None,
			tool_use_id: None,
			result_timestamp: None,
			is_final_response: false,
		},
	];
	app
}

fn render_once(app: &mut App, cols: u16, rows: u16) -> String {
	let backend = TestBackend::new(cols, rows);
	let mut terminal = Terminal::new(backend).expect("terminal");
	terminal
		.draw(|f| watching::render(f, f.area(), app))
		.expect("draw");
	// バックエンドから各行を取り出して文字列化する
	let buf = terminal.backend().buffer().clone();
	let mut out = String::new();
	for y in 0..buf.area.height {
		for x in 0..buf.area.width {
			out.push_str(buf.cell((x, y)).unwrap().symbol());
		}
		out.push('\n');
	}
	out
}

#[test]
fn default_mode_renders_main_right_and_sub_left() {
	let mut app = setup_app_with_mode(ChatMode::Default);
	let out = render_once(&mut app, 100, 30);
	// Main のヘッダー行は画面右半分にあるはず (ヘッダーの `🤖 Main` / `▏` は x >= 40 領域)
	// Sub のヘッダー行は左端に `▏` が来る。
	let lines: Vec<&str> = out.lines().collect();
	// 少なくとも `▏` マーカーはどこかに出ている
	assert!(out.contains('▏'), "default mode should include ▏ marker");
	// 一部の行に `▏` があり、左端 (x=0) の `▏` = Sub 由来の行、右側 x>=40 の `▏` = Main 由来
	let has_left_marker = lines
		.iter()
		.any(|l| l.chars().next().map(|c| c == '▏').unwrap_or(false));
	let has_right_marker = lines.iter().any(|l| {
		// 左端でない場所に `▏` があるかをチェック。chars().enumerate() で x >= 40 を探す
		l.chars().enumerate().any(|(x, c)| c == '▏' && x >= 40)
	});
	assert!(
		has_left_marker,
		"Sub bubble should be left-aligned (left ▏)"
	);
	assert!(
		has_right_marker,
		"Main bubble should be right-aligned (▏ at x>=40)"
	);
}

#[test]
fn slack_mode_renders_all_bubbles_left_aligned() {
	let mut app = setup_app_with_mode(ChatMode::Slack);
	let out = render_once(&mut app, 100, 30);
	let lines: Vec<&str> = out.lines().collect();
	// slack モードでは Main / Sub 双方とも左寄せ → どの行の `▏` も x=0 にある
	// (メッセージ行以外ではそもそも `▏` が出ない)
	for l in &lines {
		let positions: Vec<usize> = l
			.chars()
			.enumerate()
			.filter_map(|(x, c)| if c == '▏' { Some(x) } else { None })
			.collect();
		for x in positions {
			assert_eq!(
				x, 0,
				"slack mode: ▏ must be at x=0, got x={x} in line {l:?}"
			);
		}
	}
}

#[test]
fn line_mode_renders_rounded_corner_borders() {
	let mut app = setup_app_with_mode(ChatMode::Line);
	let out = render_once(&mut app, 100, 30);
	// 上辺に `╭` と `╮`、下辺に `╰` と `╯` が登場することを確認
	assert!(out.contains('╭'), "line mode should include ╭: {out}");
	assert!(out.contains('╮'), "line mode should include ╮: {out}");
	assert!(out.contains('╰'), "line mode should include ╰");
	assert!(out.contains('╯'), "line mode should include ╯");
	// Body 行の左右には `│`
	assert!(out.contains('│'), "line mode should include │ on body rows");
}

#[test]
fn default_mode_does_not_include_line_borders() {
	let mut app = setup_app_with_mode(ChatMode::Default);
	let out = render_once(&mut app, 100, 30);
	assert!(!out.contains('╭'), "default mode must NOT include ╭");
	assert!(!out.contains('╰'), "default mode must NOT include ╰");
}

#[test]
fn slack_mode_does_not_include_line_borders() {
	let mut app = setup_app_with_mode(ChatMode::Slack);
	let out = render_once(&mut app, 100, 30);
	assert!(!out.contains('╭'), "slack mode must NOT include ╭");
	assert!(!out.contains('╰'), "slack mode must NOT include ╰");
}

/// Slack モードでは bubble が端末幅いっぱいに広がる (default/line の 60% 幅
/// に対して 100%)。十分長い本文を置いたときに右側 (x>=70) まで文字が届くことで
/// 確認する。default モードだと同じ本文は 60 列で折り返されるので x<60 までしか
/// 文字が並ばない。
#[test]
fn slack_mode_uses_full_area_width() {
	let options = CliOptions::try_parse_from(["cc-chatter"]).expect("parse");
	let mut app = App::new(options, 30, 100);
	app.set_chat_mode(ChatMode::Slack);
	app.view.current_view = AppView::Watching;
	app.view.attached_agent_ids = vec!["a1".to_string()];
	app.domain.agents = vec![AgentEntity {
		agent_id: "a1".to_string(),
		agent_type: "general-purpose".to_string(),
		output_path: PathBuf::from("/tmp/nonexistent"),
		updated_at: Utc::now(),
	}];
	app.agent_type_by_id
		.insert("a1".to_string(), "general-purpose".to_string());
	app.attached_agent_type = Some("general-purpose".to_string());
	// 100 文字以上の単一行本文 (Sub 中間 text)
	let long = "x".repeat(120);
	app.domain.messages = vec![FormattedMessage {
		id: "m-0".to_string(),
		sender: Sender::Sub,
		agent_id: "a1".to_string(),
		timestamp: Utc::now(),
		text: Some(long),
		tool_use: None,
		tool_result: None,
		tool_use_id: None,
		result_timestamp: None,
		is_final_response: false,
	}];

	let out = render_once(&mut app, 100, 30);
	// 60 列より右 (x >= 70) に 'x' が描かれている行が少なくとも 1 行ある。
	// default の 60% 幅だと x<60 までしか本文が届かない。
	let has_content_beyond_60 = out
		.lines()
		.any(|line| line.chars().enumerate().any(|(x, c)| x >= 70 && c == 'x'));
	assert!(
		has_content_beyond_60,
		"slack mode must extend bubble beyond x=70 (full area width).\nrendered:\n{out}"
	);
}

/// 回帰テスト: LINE モードで下枠 `╰──╯` の **次の行 (バブル間スペーサー)** に
/// `│` が残っていないこと。実機で報告された「下枠のさらに下に `│ │` が残る」
/// バグの end-to-end 確認。text バブル複数件で検証。
#[test]
fn line_mode_has_no_stray_vertical_bar_below_bottom_border() {
	let mut app = setup_app_with_mode(ChatMode::Line);
	let out = render_once(&mut app, 100, 30);
	assert_no_stray_bar_below_bottom_border(&out, "simple text bubbles");
}

/// 同じ不変条件を **統合バブル (tool_use + tool_result) 混在**のケースでも
/// 確認する。統合バブルは `render_bubble_lines` の本文構造が text とは異なる
/// (ヘッダー / tool_use / 区切り空行 / tool_result / 末尾マージン) ため、
/// 末尾マージンの切り出しロジックが別経路にならないかを別 fixture で検証。
#[test]
fn line_mode_has_no_stray_vertical_bar_below_bottom_border_for_integrated_bubble() {
	use cc_chatter::domain::entities::{ToolResult, ToolUse};

	let options = CliOptions::try_parse_from(["cc-chatter"]).expect("parse");
	let mut app = App::new(options, 30, 100);
	app.set_chat_mode(ChatMode::Line);
	app.view.current_view = AppView::Watching;
	app.view.attached_agent_ids = vec!["a1".to_string()];
	app.domain.agents = vec![AgentEntity {
		agent_id: "a1".to_string(),
		agent_type: "general-purpose".to_string(),
		output_path: PathBuf::from("/tmp/nonexistent"),
		updated_at: Utc::now(),
	}];
	app.agent_type_by_id
		.insert("a1".to_string(), "general-purpose".to_string());
	app.attached_agent_type = Some("general-purpose".to_string());
	app.domain.messages = vec![
		// preview (integrated bubble)
		FormattedMessage {
			id: "m-0".to_string(),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: None,
			tool_use: Some(ToolUse {
				name: "Bash".to_string(),
				input: serde_json::json!({"command": "echo hi"}),
			}),
			tool_result: Some(ToolResult {
				content: "hi".to_string(),
				is_error: false,
			}),
			tool_use_id: Some("tu-1".to_string()),
			result_timestamp: Some(Utc::now()),
			is_final_response: false,
		},
		// error result (integrated bubble)
		FormattedMessage {
			id: "m-1".to_string(),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: None,
			tool_use: Some(ToolUse {
				name: "Bash".to_string(),
				input: serde_json::json!({"command": "false"}),
			}),
			tool_result: Some(ToolResult {
				content: "command failed".to_string(),
				is_error: true,
			}),
			tool_use_id: Some("tu-2".to_string()),
			result_timestamp: Some(Utc::now()),
			is_final_response: false,
		},
	];

	let out = render_once(&mut app, 100, 30);
	assert_no_stray_bar_below_bottom_border(&out, "integrated bubble");
}

/// `│` が **バブルの外**に現れない (TopBorder / BottomBorder 間の行でしか
/// 使われない) ことを端末全幅の視点で確認する。Margin 行や bubble 外側に
/// `│` が描画された場合を広く拾う。
///
/// 実装: 「連続する `│` 行の集合は必ず直前に `╭` を含む行があり、直後に `╰`
/// を含む行があるべし」を走査で確認する。
#[test]
fn line_mode_vertical_bars_are_only_enclosed_within_rounded_borders() {
	let mut app = setup_app_with_mode(ChatMode::Line);
	let out = render_once(&mut app, 100, 30);
	assert_bars_are_enclosed(&out, "line");
}

/// helper: 「`╰──╯` の **すぐ次の行** に `│` が無い」を assert する。
fn assert_no_stray_bar_below_bottom_border(out: &str, label: &str) {
	let lines: Vec<&str> = out.lines().collect();
	for (i, line) in lines.iter().enumerate() {
		if line.contains('╰') && line.contains('╯') {
			if let Some(next) = lines.get(i + 1) {
				assert!(
					!next.contains('│'),
					"[{label}] LINE mode: row {} is ╰──╯, row {} must not contain │ \
					(inter-bubble margin must be clean).\n\
					bottom border: {line:?}\nnext row:     {next:?}",
					i,
					i + 1,
				);
			}
		}
	}
}

/// helper: `│` を含む連続行グループは、直前に `╭` 行、直後に `╰` 行が
/// あることを assert する (= `│` はバブル **内側** でしか使われない)。
fn assert_bars_are_enclosed(out: &str, label: &str) {
	let lines: Vec<&str> = out.lines().collect();
	let mut groups: Vec<(usize, usize)> = Vec::new();
	let mut start: Option<usize> = None;
	for (i, line) in lines.iter().enumerate() {
		if line.contains('│') {
			start.get_or_insert(i);
		} else if let Some(s) = start.take() {
			groups.push((s, i - 1));
		}
	}
	if let Some(s) = start {
		groups.push((s, lines.len() - 1));
	}

	for (begin, end) in groups {
		// group の直前の行が `╭` を含むこと
		let prev = begin
			.checked_sub(1)
			.and_then(|i| lines.get(i))
			.copied()
			.unwrap_or("");
		assert!(
			prev.contains('╭'),
			"[{label}] │-group rows {begin}..={end}: previous row should have ╭ (bubble top).\n\
			prev:      {prev:?}\n\
			first-bar: {:?}",
			lines[begin],
		);
		// group の直後の行が `╰` を含むこと
		let next = lines.get(end + 1).copied().unwrap_or("");
		assert!(
			next.contains('╰'),
			"[{label}] │-group rows {begin}..={end}: next row should have ╰ (bubble bottom).\n\
			last-bar: {:?}\n\
			next:     {next:?}",
			lines[end],
		);
	}
}

// =========================================================================
// 回帰テスト: LINE モードで本文 Body 行の右 `│` の x 座標 ==
// 下枠 `╯` の x 座標 (off-by-one 防止)
// =========================================================================
//
// 以前の実装では watching.rs が全モード共通で `content_width = bubble_width - 1`
// を渡しており、LINE モードの TopBorder / BottomBorder は `content_width` 文字分
// (`╭` + `─`×(content_width-2) + `╮`) で作られる一方、Body の右 `│` は
// `bubble_width - 1` 位置に描かれていた。content_width == bubble_width - 1 の
// ため `╯` は `x+content_width-1 = x+bubble_width-2`、右 `│` は `x+bubble_width-1`
// と 1 セルずれる症状があった。
//
// 本テストは watching::render を通した Buffer から `╭/╮/╰/╯` と `│` の x 座標を
// 抽出して、**同じバブル内で全部同一 x で揃う** ことを assert する。バグ再注入
// (content_width を bubble_width - 1 に戻す) で必ず落ちることを teammate 側で
// 確認済み。

/// LINE モードで 1 bubble を描画したときの枠線 / Body bar の x 座標まとめ。
struct LineModeBubbleGeometry {
	/// 上枠 `╭` / `╮` / 下枠 `╰` / `╯` の x 座標 (左, 右, 左, 右)。
	corners: Option<(u16, u16, u16, u16)>,
	/// Body 行の (左 `│`, 右 `│`) の x 座標。
	bars: Option<(u16, u16)>,
}

/// 単一 bubble を LINE モードで描画し、枠線と Body 行の `│` の x 座標を返す。
fn line_mode_bubble_corners_and_bars(sender: Sender, area_cols: u16) -> LineModeBubbleGeometry {
	let options = CliOptions::try_parse_from(["cc-chatter"]).expect("parse");
	let mut app = App::new(options, 30, area_cols);
	app.set_chat_mode(ChatMode::Line);
	app.view.current_view = AppView::Watching;
	app.view.attached_agent_ids = vec!["a1".to_string()];
	app.domain.agents = vec![AgentEntity {
		agent_id: "a1".to_string(),
		agent_type: "general-purpose".to_string(),
		output_path: PathBuf::from("/tmp/nonexistent"),
		updated_at: Utc::now(),
	}];
	app.agent_type_by_id
		.insert("a1".to_string(), "general-purpose".to_string());
	app.attached_agent_type = Some("general-purpose".to_string());
	app.domain.messages = vec![FormattedMessage {
		id: "m-0".to_string(),
		sender,
		agent_id: "a1".to_string(),
		timestamp: Utc::now(),
		text: Some("hi".to_string()),
		tool_use: None,
		tool_result: None,
		tool_use_id: None,
		result_timestamp: None,
		is_final_response: false,
	}];

	let backend = TestBackend::new(area_cols, 30);
	let mut terminal = Terminal::new(backend).expect("terminal");
	terminal
		.draw(|f| watching::render(f, f.area(), &mut app))
		.expect("draw");
	let buf = terminal.backend().buffer().clone();

	let mut top: Option<(u16, u16)> = None; // (╭ x, ╮ x)
	let mut bot: Option<(u16, u16)> = None; // (╰ x, ╯ x)
	let mut bars: Option<(u16, u16)> = None; // (left │ x, right │ x) first body row

	for y in 0..buf.area.height {
		let mut lx: Option<u16> = None;
		let mut rx: Option<u16> = None;
		let mut bar_lx: Option<u16> = None;
		let mut bar_rx: Option<u16> = None;
		for x in 0..buf.area.width {
			let sym = buf.cell((x, y)).unwrap().symbol();
			match sym {
				"╭" => lx = Some(x),
				"╮" => rx = Some(x),
				"╰" => lx = Some(x),
				"╯" => rx = Some(x),
				"│" => {
					if bar_lx.is_none() {
						bar_lx = Some(x);
					}
					bar_rx = Some(x);
				}
				_ => {}
			}
		}
		// Top border row (`╭` + `╮`)
		if top.is_none() {
			if let (Some(l), Some(r)) = (lx, rx) {
				if buf.cell((l, y)).unwrap().symbol() == "╭" {
					top = Some((l, r));
					continue;
				}
			}
		}
		// Body row inside bubble (captures the FIRST body row that has `│ ... │`)
		if top.is_some() && bot.is_none() && bars.is_none() {
			if let (Some(bl), Some(br)) = (bar_lx, bar_rx) {
				bars = Some((bl, br));
				continue;
			}
		}
		// Bottom border row (`╰` + `╯`)
		if bot.is_none() {
			if let (Some(l), Some(r)) = (lx, rx) {
				if buf.cell((l, y)).unwrap().symbol() == "╰" {
					bot = Some((l, r));
				}
			}
		}
	}

	let corners = match (top, bot) {
		(Some((tl, tr)), Some((bl, br))) => Some((tl, tr, bl, br)),
		_ => None,
	};
	LineModeBubbleGeometry { corners, bars }
}

/// LINE モードで **左右枠線 `│` の x 座標が `╭╮╰╯` の x 座標と揃う** ことを
/// assert する。Main / Sub 両 sender、area_width が偶数 / 奇数、小さめ / 標準
/// の組合せで網羅的に検証する。
///
/// バグ (content_width = bubble_width - 1 をそのまま使う) が再発すると
/// 右 `│` が右枠 `╮` / `╯` より 1 セル右にずれて、以下いずれかの assert が
/// 失敗する:
/// - `top_right_x != bar_right_x` (右枠と Body 右 `│` のミスマッチ)
/// - `top_right_x != bottom_right_x` (上下枠のミスマッチ — 実際には起きないはず)
#[test]
fn line_mode_right_border_aligns_with_bottom_corner() {
	// area_width を色々試す。LINE モードの bubble_width は 60%、
	// 最低 30 (clamp)。content_width (= bubble_width) は以下のとおり:
	//   area=100 → bubble=60
	//   area=80  → bubble=48
	//   area=51  → bubble=30 (clamp; 51*60/100 = 30 だがコード上 min(30,51)=30)
	//   area=40  → bubble=30 (clamp; 40*60/100=24 < 30)
	//   area=41  → bubble=30 (奇数 area)
	//   area=81  → bubble=48 (奇数 area, 48 偶数 bubble)
	//   area=83  → bubble=49 (奇数 area, 奇数 bubble)
	let cases: &[(Sender, u16, &str)] = &[
		(Sender::Sub, 100, "sub-100"),
		(Sender::Main, 100, "main-100"),
		(Sender::Sub, 80, "sub-80-even"),
		(Sender::Main, 80, "main-80-even"),
		(Sender::Sub, 81, "sub-81-odd-area"),
		(Sender::Main, 81, "main-81-odd-area"),
		(Sender::Sub, 83, "sub-83-odd-both"),
		(Sender::Main, 83, "main-83-odd-both"),
		(Sender::Sub, 51, "sub-51-min-clamp"),
		(Sender::Main, 51, "main-51-min-clamp"),
	];
	for (sender, area_cols, label) in cases {
		let geom = line_mode_bubble_corners_and_bars(*sender, *area_cols);
		let (tl, tr, bl, br) = geom.corners.unwrap_or_else(|| {
			panic!("[{label}] LINE mode: ╭/╮/╰/╯ corners must be present in rendered bubble")
		});
		let (bar_l, bar_r) = geom
			.bars
			.unwrap_or_else(|| panic!("[{label}] LINE mode: Body row must have left and right │"));

		// (A) 上枠と下枠の左右 x が一致 (box が矩形であること)
		assert_eq!(
			tl, bl,
			"[{label}] top-left (╭) x={tl} must equal bottom-left (╰) x={bl}"
		);
		assert_eq!(
			tr, br,
			"[{label}] top-right (╮) x={tr} must equal bottom-right (╯) x={br}"
		);

		// (B) Body 行の左 │ が ╭/╰ と同じ x
		assert_eq!(
			bar_l, tl,
			"[{label}] body left │ x={bar_l} must equal ╭ x={tl}",
		);

		// (C) ★ 本丸: Body 行の右 │ が ╮/╯ と同じ x (off-by-one regression)
		assert_eq!(
			bar_r, tr,
			"[{label}] body right │ x={bar_r} must equal ╯ x={tr} \
			(off-by-one: bubble の右枠と Body 右 │ がずれている)",
		);
	}
}

/// 同じ不変条件を **統合バブル (tool_use + tool_result)** でも確認する。
/// render 経路が text bubble と異なる (区切り空行や `↪ result` ラベル) ため、
/// 別 fixture で独立に検証する。
#[test]
fn line_mode_right_border_aligns_for_integrated_bubble() {
	use cc_chatter::domain::entities::{ToolResult, ToolUse};

	for area_cols in [100u16, 81, 51] {
		let options = CliOptions::try_parse_from(["cc-chatter"]).expect("parse");
		let mut app = App::new(options, 30, area_cols);
		app.set_chat_mode(ChatMode::Line);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".to_string()];
		app.domain.agents = vec![AgentEntity {
			agent_id: "a1".to_string(),
			agent_type: "general-purpose".to_string(),
			output_path: PathBuf::from("/tmp/x"),
			updated_at: Utc::now(),
		}];
		app.agent_type_by_id
			.insert("a1".to_string(), "general-purpose".to_string());
		app.attached_agent_type = Some("general-purpose".to_string());
		app.domain.messages = vec![FormattedMessage {
			id: "m-0".to_string(),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: None,
			tool_use: Some(ToolUse {
				name: "Bash".to_string(),
				input: serde_json::json!({"command": "echo hi"}),
			}),
			tool_result: Some(ToolResult {
				content: "hi".to_string(),
				is_error: false,
			}),
			tool_use_id: Some("tu-1".to_string()),
			result_timestamp: Some(Utc::now()),
			is_final_response: false,
		}];

		let backend = TestBackend::new(area_cols, 30);
		let mut terminal = Terminal::new(backend).expect("terminal");
		terminal
			.draw(|f| watching::render(f, f.area(), &mut app))
			.expect("draw");
		let buf = terminal.backend().buffer().clone();

		// Find ╰/╯ and the last preceding │ row, assert same right-x.
		let mut corner_r: Option<u16> = None;
		let mut bar_rs: Vec<u16> = Vec::new();
		for y in 0..buf.area.height {
			for x in 0..buf.area.width {
				let sym = buf.cell((x, y)).unwrap().symbol();
				if sym == "╯" {
					corner_r = Some(x);
				}
				if sym == "│" {
					// Last index in this row will be the right │
					bar_rs.push(x);
				}
			}
		}
		// Rightmost │ in any row
		let max_bar_r = *bar_rs.iter().max().unwrap_or(&0);
		let cr =
			corner_r.unwrap_or_else(|| panic!("[integrated area={area_cols}] ╯ must be present"));
		assert_eq!(
			max_bar_r, cr,
			"[integrated area={area_cols}] rightmost Body │ x={max_bar_r} \
			must equal ╯ x={cr} (integrated-bubble off-by-one regression)",
		);
	}
}

// ----------------------------------------------------------------------
// viewport 内 in-flight 集計 (スピナー dirty 制御の前提条件)
// ----------------------------------------------------------------------

mod inflight_aggregation {
	use super::*;
	use cc_chatter::application::msg::Msg;
	use cc_chatter::domain::entities::{ToolResult, ToolUse};

	fn setup_app() -> App {
		let options = CliOptions::try_parse_from(["cc-chatter"]).expect("parse");
		let mut app = App::new(options, 30, 100);
		app.view.current_view = AppView::Watching;
		app.view.attached_agent_ids = vec!["a1".to_string()];
		app.domain.agents = vec![AgentEntity {
			agent_id: "a1".to_string(),
			agent_type: "general-purpose".to_string(),
			output_path: PathBuf::from("/tmp/nonexistent"),
			updated_at: Utc::now(),
		}];
		app.agent_type_by_id
			.insert("a1".to_string(), "general-purpose".to_string());
		app.attached_agent_type = Some("general-purpose".to_string());
		app
	}

	fn in_progress_msg(id: &str) -> FormattedMessage {
		FormattedMessage {
			id: id.to_string(),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: None,
			tool_use: Some(ToolUse {
				name: "Bash".to_string(),
				input: serde_json::json!({"command":"sleep 1"}),
			}),
			tool_result: None,
			tool_use_id: Some(id.to_string()),
			result_timestamp: None,
			is_final_response: false,
		}
	}

	fn done_msg(id: &str) -> FormattedMessage {
		FormattedMessage {
			id: id.to_string(),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: None,
			tool_use: Some(ToolUse {
				name: "Bash".to_string(),
				input: serde_json::json!({"command":"echo hi"}),
			}),
			tool_result: Some(ToolResult {
				content: "hi".to_string(),
				is_error: false,
			}),
			tool_use_id: Some(id.to_string()),
			result_timestamp: Some(Utc::now()),
			is_final_response: false,
		}
	}

	/// 未完了 tool_use が viewport 内に映っているとき、描画後の
	/// `viewport_cache.has_inflight_in_view` が true になる。
	#[test]
	fn render_sets_has_inflight_in_view_when_pending_visible() {
		let mut app = setup_app();
		app.domain.messages = vec![in_progress_msg("m-0"), done_msg("m-1")];

		let backend = TestBackend::new(100, 30);
		let mut terminal = Terminal::new(backend).expect("terminal");
		terminal
			.draw(|f| watching::render(f, f.area(), &mut app))
			.expect("draw");
		assert!(
			app.viewport_cache.has_inflight_in_view,
			"in-flight bubble in view must set has_inflight_in_view"
		);
	}

	/// 完了済みのみが映っているときは `has_inflight_in_view=false`。
	#[test]
	fn render_keeps_has_inflight_in_view_false_when_only_done() {
		let mut app = setup_app();
		app.domain.messages = vec![done_msg("m-0"), done_msg("m-1")];

		let backend = TestBackend::new(100, 30);
		let mut terminal = Terminal::new(backend).expect("terminal");
		terminal
			.draw(|f| watching::render(f, f.area(), &mut app))
			.expect("draw");
		assert!(
			!app.viewport_cache.has_inflight_in_view,
			"all-done viewport must keep has_inflight_in_view=false"
		);
	}

	/// in-flight 中は Tick で spinner_phase が進み、`render` 経由でラベル末尾の
	/// スピナー文字も切り替わる (E2E: App + Tick + describe フロー)。
	#[test]
	fn tick_then_render_advances_spinner_glyph_on_in_flight_bubble() {
		use cc_chatter::ui::components::chat_bubble::SPINNER_FRAMES;

		let mut app = setup_app();
		app.domain.messages = vec![in_progress_msg("m-0")];

		// 1 度描画して has_inflight_in_view=true をキャッシュに乗せる
		let backend = TestBackend::new(100, 30);
		let mut terminal = Terminal::new(backend).expect("terminal");
		terminal
			.draw(|f| watching::render(f, f.area(), &mut app))
			.expect("draw");
		assert!(app.viewport_cache.has_inflight_in_view);

		// Tick で phase が +1 されて mark_dirty
		let phase_before = app.spinner_phase;
		app.update(Msg::Tick);
		assert_eq!(app.spinner_phase, phase_before.wrapping_add(1));

		// もう 1 回描画 → ラベル行末尾に `SPINNER_FRAMES[1]` が現れる
		terminal
			.draw(|f| watching::render(f, f.area(), &mut app))
			.expect("draw");
		let buf = terminal.backend().buffer().clone();
		let mut frames_seen: Vec<char> = Vec::new();
		for y in 0..buf.area.height {
			for x in 0..buf.area.width {
				let sym = buf.cell((x, y)).unwrap().symbol();
				if let Some(ch) = sym.chars().next() {
					if SPINNER_FRAMES.contains(&ch) {
						frames_seen.push(ch);
					}
				}
			}
		}
		assert!(
			frames_seen.contains(&SPINNER_FRAMES[1]),
			"after one Tick, frame[1] should appear, got {frames_seen:?}",
		);
	}
}
