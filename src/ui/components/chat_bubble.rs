//! ChatBubble レンダリング。TS 版 `src/ui/components/ChatBubble.tsx` の移植。
//!
//! M3 設計:
//! - 描画は **pure 関数 `render_bubble_lines`** として実装する
//!   (`Vec<ratatui::text::Line>` を返す)
//! - viewport 層 (`ui::layout::viewport_layout`) がメッセージ列を連結して
//!   グローバル行番号を持たせた上で、行単位クリップを適用する
//! - `Widget` trait は廃止。代わりに `draw_bubble_line(&Line, area, buf, sender)`
//!   で 1 行ずつ Buffer に書き込む (左右寄せ + 左ボーダー色付けはここで適用)
//!
//! 見た目の責務:
//! - Main/Sub で左右寄せ (area の 60%)、左ボーダー色 Main=黄 / Sub=緑
//! - ヘッダー 1 行 (アイコン + 送信者 + timestamp)
//! - 通常モード: テキスト or preview 1-2 行 / 展開モード: 全文
//! - 選択時: ヘッダーを inverse 表示 + `◄ selected` マーカー

use ratatui::{
	buffer::Buffer,
	layout::Rect,
	style::{Color, Modifier, Style},
	text::{Line, Span},
};
use serde_json::Value;

use crate::domain::entities::{FormattedMessage, Sender, ToolResult};
use crate::settings::ChatMode;
use crate::ui::format::{
	format_result_preview, format_time, format_tool_preview, git_bash_preview,
};
use crate::ui::icons::get_agent_icon;
use crate::ui::layout::{
	effective_inner_width, wrap_lines_iter, wrap_lines_iter_with_prefix, LINE_BORDER_COLS,
};

/// LINE モードの 4 辺枠線文字 (ラウンド角)。
///
/// `╭╮╰╯` は LINE アプリ吹き出し風に丸みを出す。`┌┐└┘` (角) や `╔╗╚╝` (二重)
/// も候補だが、メッセンジャーらしい柔らかさを優先して丸角を採用する。
pub(crate) const LINE_TL: char = '╭';
pub(crate) const LINE_TR: char = '╮';
pub(crate) const LINE_BL: char = '╰';
pub(crate) const LINE_BR: char = '╯';
pub(crate) const LINE_H: char = '─';
pub(crate) const LINE_V: char = '│';

/// ツール実行中スピナーのアニメーションフレーム (Braille spinner)。
///
/// Tick (100ms) ごとに `App::spinner_phase` をインクリメントし、描画時に
/// `SPINNER_FRAMES[spinner_phase as usize % SPINNER_FRAMES.len()]` で
/// 該当フレームを取り出す。10 フレームなので 1 周 1 秒。
pub const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// 完了マーカー文字 (緑)。
pub const DONE_MARKER: char = '✓';

/// エラーマーカー文字 (赤)。
pub const ERROR_MARKER: char = '✗';

/// tool_use バブルの実行ステータス。`render_bubble_lines` がメッセージから
/// 算出して `push_tool_use_lines` に渡し、ラベル行末尾の Span 1 つに変換される。
///
/// - `InProgress`: `tool_use=Some && tool_result=None` (実行中)
/// - `Done`: `tool_use=Some && tool_result=Some && !is_error` (完了)
/// - `Errored`: `tool_use=Some && tool_result=Some && is_error` (失敗)
///
/// orphan tool_result (= `tool_use=None`) や text バブルではこの enum は使わない
/// (呼び出し側で `tool_status` が None を返す)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
	InProgress,
	Done,
	Errored,
}

/// メッセージから tool_use のステータスを算出する。
///
/// - `tool_use=None`: tool_use バブルではない → None
/// - `tool_use=Some && tool_result=None`: 実行中 → InProgress
/// - `tool_use=Some && tool_result.is_error=true`: 失敗 → Errored
/// - `tool_use=Some && tool_result.is_error=false`: 完了 → Done
pub fn tool_status(msg: &FormattedMessage) -> Option<ToolStatus> {
	msg.tool_use.as_ref()?;
	match msg.tool_result.as_ref() {
		None => Some(ToolStatus::InProgress),
		Some(tr) if tr.is_error => Some(ToolStatus::Errored),
		Some(_) => Some(ToolStatus::Done),
	}
}

/// 描画対象の行種別。`render_bubble_rows` が返し、`draw_bubble_row` が
/// 境界行 / 本文行 / マージン行を描き分ける。
///
/// - `TopBorder`: bubble 上辺の枠線行 (LINE モードのみ `╭──...──╮` を持つ。
///   他モードでは使わない)
/// - `Body`: ヘッダー + 本文。default / slack では左 1 文字に `▏`、line では
///   左右に `│` が付く
/// - `BottomBorder`: bubble 下辺の枠線行 (LINE モードのみ `╰──...──╯`)
/// - `Margin`: バブル間スペーサー (末尾空行)。**どのモードでも何も描かない**。
///   枠外扱いなので `│` や `▏` は付けない
///
/// default / slack モードは `TopBorder` / `BottomBorder` を使わない (bubble の
/// 末尾は `Body` (本文の最後) + `Margin` の 2 行構成)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BubbleRowKind {
	TopBorder,
	Body,
	BottomBorder,
	Margin,
}

/// `render_bubble_rows` の出力要素。`Line` 本体と種別タグを持つ。
///
/// 種別は描画時の寄せ / 枠描画の分岐にしか使わないので Clone は安い。
/// 既存の `render_bubble_lines` は `Vec<Line>` のままで、既存テストは
/// そのまま使える (Default モード想定の検証)。
#[derive(Debug, Clone)]
pub struct BubbleRow<'a> {
	pub kind: BubbleRowKind,
	pub line: Line<'a>,
}

/// ChatBubble 用の入力。
pub struct ChatBubbleModel<'a> {
	pub message: &'a FormattedMessage,
	pub agent_type: &'a str,
	pub is_detailed_mode: bool,
	pub is_expanded: bool,
	pub is_selected: bool,
}

/// ヘッダー + 本文 + 末尾マージン 1 行を Line 列で返す。
///
/// 本文 (text / 展開時の tool input JSON / 展開時の tool result content) は
/// `content_width` で折り返して **1 Line = 1 物理行** に揃える。`draw_bubble_row`
/// は各 Line を `height=1` の矩形に描画するため、ここで折り返しておかないと
/// 長い行が視覚的に切り捨てられ、`estimate_bubble_height` の見積もり
/// (`wrap_line_count` で折り返し後の行数をカウント) と実描画行数がズレて
/// viewport 末尾に空白が残る (follow_tail 時に最新メッセージが画面下端まで
/// 届かない症状)。
///
/// preview (非展開時の tool_use / tool_result の 2 行目) は 1 Line のまま扱い、
/// `Paragraph` が truncate するので estimate 側も `+1` 固定にしている。
pub fn render_bubble_lines<'a>(
	model: &ChatBubbleModel<'a>,
	content_width: u16,
	spinner_phase: u8,
) -> Vec<Line<'a>> {
	let wrap_width = content_width.max(1) as usize;
	let (border_color, sender_label) = match model.message.sender {
		Sender::Main => (Color::Yellow, "Main".to_string()),
		Sender::Sub => (Color::Green, model.agent_type.to_string()),
	};

	let icon = match model.message.sender {
		Sender::Main => get_agent_icon("main"),
		Sender::Sub => get_agent_icon(model.agent_type),
	};
	let time_str = format_time(&model.message.timestamp);

	// --- ヘッダー行 ---------------------------------------------------------
	let header_style = if model.is_selected {
		Style::default()
			.fg(border_color)
			.add_modifier(Modifier::REVERSED | Modifier::BOLD)
	} else {
		Style::default()
			.fg(border_color)
			.add_modifier(Modifier::BOLD)
	};
	let mut header_spans = vec![
		Span::styled(format!("{icon} {sender_label} "), header_style),
		Span::styled(time_str, Style::default().fg(Color::DarkGray)),
	];
	if model.is_selected {
		header_spans.push(Span::raw("  "));
		header_spans.push(Span::styled(
			"◄ selected",
			Style::default()
				.fg(Color::Cyan)
				.add_modifier(Modifier::BOLD),
		));
	}

	let mut lines: Vec<Line> = vec![Line::from(header_spans)];
	let expanded = model.is_detailed_mode || model.is_expanded;

	// --- 本文 ---------------------------------------------------------------
	// `wrap_lines_iter` で `&str` のサブスライスを yield してもらい、各行ぶんだけ
	// String::from を 1 回発生させる (旧実装は `wrap_string_lines` が中間 Vec<String>
	// を返していて二重 alloc だった)。
	if let Some(text) = model.message.text.as_deref() {
		// mention prefix の決定:
		// - Main → `@{agent_type} ` (agent に向けた発話)
		// - Sub + final_response → `@Main ` (Main 宛て最終応答)
		// - それ以外 (Sub 中間 text) → mention なし + 本文を DarkGray で薄色化
		let mention = mention_prefix(model);
		let prefix_width = mention
			.as_ref()
			.map(|s| s.content.chars().count())
			.unwrap_or(0);

		// 中間 text (Sub + !is_final_response) は独り言扱いで本文全体を暗灰色に。
		// ヘッダーの sender 色は変えない (選択ハイライトが壊れないようにするため)。
		let body_style = if model.message.sender == Sender::Sub && !model.message.is_final_response
		{
			Style::default().fg(Color::DarkGray)
		} else {
			Style::default()
		};

		let mut first_line = true;
		for wrapped in wrap_lines_iter_with_prefix(text, wrap_width, prefix_width) {
			if first_line {
				first_line = false;
				if let Some(mention_span) = mention.clone() {
					// 1 行目: mention Span + 本文 Span を 1 Line にまとめる。
					// 本文側だけ body_style を適用 (mention は常に Cyan + Bold)。
					// ※ 中間 text には mention が付かないので、mention がある =
					//    body_style はデフォルト色になる組み合わせに限定される。
					lines.push(Line::from(vec![
						mention_span,
						Span::styled(wrapped.to_string(), body_style),
					]));
					continue;
				}
			}
			lines.push(Line::from(Span::styled(wrapped.to_string(), body_style)));
		}
	} else if let (Some(tu), Some(tr)) = (
		model.message.tool_use.as_ref(),
		model.message.tool_result.as_ref(),
	) {
		// 統合バブル: 呼び出し + 結果 を 1 バブル。
		// **closed (preview)** では tool_use のみ表示し、区切り / result は出さない
		// (ノイズ削減のためツール表示をミニマム化)。`expanded` のときだけ区切り 1 行
		// + tool_result セクション (indent なし) を続ける。
		// `estimate_bubble_height_full` の同分岐 (`is_expanded` で切り替え) と
		// 行数を揃えること (lessons.md「見積もり == 実描画行数」原則)。
		let status = tool_status(model.message);
		push_tool_use_lines(
			&mut lines,
			&tu.name,
			&tu.input,
			expanded,
			wrap_width,
			status,
			spinner_phase,
		);
		if expanded {
			// 区切り (空行 1 行)
			lines.push(Line::from(""));
			push_tool_result_lines(&mut lines, tr, expanded, wrap_width, 0);
		}
	} else if let Some(tu) = model.message.tool_use.as_ref() {
		let status = tool_status(model.message);
		push_tool_use_lines(
			&mut lines,
			&tu.name,
			&tu.input,
			expanded,
			wrap_width,
			status,
			spinner_phase,
		);
	} else if let Some(tr) = model.message.tool_result.as_ref() {
		// orphan tool_result はインデントなし (従来どおり)
		push_tool_result_lines(&mut lines, tr, expanded, wrap_width, 0);
	}

	// --- 末尾マージン (空行) -----------------------------------------------
	lines.push(Line::from(""));

	lines
}

/// `render_bubble_lines` の chat_mode 対応版。`BubbleRow` のタグ付きで返す。
///
/// - `Default` / `Slack`: 既存の `render_bubble_lines(model, content_width)` を
///   そのまま包んで返す (行数は変わらない)。**末尾の 1 行 (空行マージン)** は
///   `Margin` で tag し、それ以外は `Body`。
/// - `Line`: 本文幅を `content_width - 2` に縮め (左右ボーダー 1 列ずつ食う)、
///   その上で `TopBorder` (`╭──...──╮`) を先頭に、`BottomBorder` (`╰──...──╯`)
///   を末尾マージンの直前に挿入する (合計 +2 行)。末尾マージンは `Margin` tag。
///   結果の `.len()` は `estimate_bubble_height_full` と常に一致する (見積もり
///   == 実描画行数 不変条件)。
///
/// 末尾マージン行はバブル間スペーサーで **枠外扱い**。`draw_bubble_row` は
/// `Margin` のとき何も描かず、row を消費するだけにする (実機で LINE モード
/// の下枠 `╰──╯` の 1 行下に `│ ... │` が残って見えるバグの対策)。
///
/// ## 不変条件
///
/// **どの chat_mode / どのメッセージ形状でも、返される `Vec<BubbleRow>` の
/// 末尾要素は必ず `BubbleRowKind::Margin`** にする。ここはモード毎に tag を
/// 書かず、[`split_trailing_margin`] という単一の helper を経由させることで
/// mode 間ドリフトを **型レベルで** 防ぐ。過去に LINE 分岐だけ末尾マージンを
/// `Body` のまま push して `│ │` が残ったバグ (`lessons.md` の該当節) の
/// 再発防止も兼ねる。
///
/// 境界行は `content_width` 全幅で描かれる (ヘッダーを枠内に収める設計)。
pub fn render_bubble_rows<'a>(
	model: &ChatBubbleModel<'a>,
	content_width: u16,
	chat_mode: ChatMode,
	spinner_phase: u8,
) -> Vec<BubbleRow<'a>> {
	match chat_mode {
		ChatMode::Default | ChatMode::Slack => {
			// `render_bubble_lines` は最低 1 要素 (末尾マージン) を常に含む。
			// 最後の 1 要素を切り出して Margin tag に、残りは Body。
			let lines = render_bubble_lines(model, content_width, spinner_phase);
			let (body_lines, margin_row) = split_trailing_margin(lines);
			let mut rows: Vec<BubbleRow<'a>> = body_lines
				.into_iter()
				.map(|line| BubbleRow {
					kind: BubbleRowKind::Body,
					line,
				})
				.collect();
			rows.push(margin_row);
			rows
		}
		ChatMode::Line => {
			let inner_width = effective_inner_width(content_width, ChatMode::Line);
			let body = render_bubble_lines(model, inner_width, spinner_phase);
			let border_color = match model.message.sender {
				Sender::Main => Color::Yellow,
				Sender::Sub => Color::Green,
			};
			// ボーダー行の水平方向の長さは bubble 全幅 (content_width) ぶん。
			// `draw_bubble_row` で set_line により bubble_width 幅に clamp されるが、
			// ここで `content_width - 2` の水平バー + 左右角を自前で組む。
			let horizontal = LINE_H
				.to_string()
				.repeat(content_width.saturating_sub(LINE_BORDER_COLS) as usize);
			let top_line = Line::from(Span::styled(
				format!("{LINE_TL}{horizontal}{LINE_TR}"),
				Style::default().fg(border_color),
			));
			let bottom_line = Line::from(Span::styled(
				format!("{LINE_BL}{horizontal}{LINE_BR}"),
				Style::default().fg(border_color),
			));

			// `render_bubble_lines` の末尾空行を **ここで必ず切り出して** Margin
			// で tag する。LINE 分岐内では絶対に `BubbleRowKind::Body` で
			// 末尾マージンを push しない (モード間ドリフト防止)。
			let (body_inner, margin_row) = split_trailing_margin(body);

			let mut rows: Vec<BubbleRow<'a>> = Vec::with_capacity(body_inner.len() + 3);
			rows.push(BubbleRow {
				kind: BubbleRowKind::TopBorder,
				line: top_line,
			});
			for line in body_inner {
				rows.push(BubbleRow {
					kind: BubbleRowKind::Body,
					line,
				});
			}
			rows.push(BubbleRow {
				kind: BubbleRowKind::BottomBorder,
				line: bottom_line,
			});
			rows.push(margin_row);
			rows
		}
	}
}

/// `render_bubble_lines` の出力から末尾 1 行 (バブル間スペーサーの空行) を
/// `BubbleRowKind::Margin` として切り出す唯一の helper。
///
/// 返り値は `(body, margin_row)`。
/// - `body`: マージンを除いた Line 列 (呼び出し側で `Body` tag で包む)
/// - `margin_row`: `Margin` で tag 済みの BubbleRow
///
/// `render_bubble_lines` が常に最低 1 要素 (空行マージン) を push する契約に
/// 依存。万一空の Vec が来ても (将来 `render_bubble_lines` の仕様が変わる
/// など) `Line::from("")` で安全側に倒す。
///
/// この関数は [`render_bubble_rows`] の **全モード分岐で使う** こと。
/// モード毎にインラインで tag を書くと、新モード追加 (LINE 追加時と同様)
/// のタイミングで tag 漏れが起きる。単一箇所に集めておけば、property-based
/// テスト (`render_bubble_rows_always_ends_with_margin_across_modes_and_shapes`)
/// が未来の分岐も全部落とす。
fn split_trailing_margin<'a>(mut lines: Vec<Line<'a>>) -> (Vec<Line<'a>>, BubbleRow<'a>) {
	let margin_line = lines.pop().unwrap_or_else(|| Line::from(""));
	(
		lines,
		BubbleRow {
			kind: BubbleRowKind::Margin,
			line: margin_line,
		},
	)
}

/// ChatBubble の text 本文に差し込む mention prefix Span を作る。
///
/// Slack / Discord のメンション風に 1 行目の先頭に `@Main` / `@{agent_type}` を
/// 差し込んで、Main ↔ agent の対話相手を視覚的に明示する。
///
/// - `Sender::Main` → `@{agent_type}` (Main から agent への prompt)
/// - `Sender::Sub` + `is_final_response=true` → `@Main` (agent から Main への最終応答)
/// - `Sender::Sub` + `is_final_response=false` → None (独り言 / 中間 text 扱い、
///   代わりに本文が DarkGray で薄色化される)
///
/// mention は `Color::Cyan + Modifier::BOLD` + 末尾半角スペース 1。
///
/// `render_bubble_lines` と `estimate_bubble_height` の両方で同じ表示幅 (chars
/// カウント) を使うため、表示幅計算は `ui::layout::mention_prefix_width` 側と
/// 必ず一致させる (lessons.md の「見積もり == 実描画行数」原則)。
fn mention_prefix<'a>(model: &ChatBubbleModel<'a>) -> Option<Span<'a>> {
	let style = Style::default()
		.fg(Color::Cyan)
		.add_modifier(Modifier::BOLD);
	match model.message.sender {
		Sender::Main => Some(Span::styled(format!("@{} ", model.agent_type), style)),
		Sender::Sub if model.message.is_final_response => {
			Some(Span::styled("@Main ".to_string(), style))
		}
		_ => None,
	}
}

/// tool_use 用の本文 (ラベル + preview / 全文) を `lines` に push する。
///
/// `estimate_bubble_height` の `tool_use_height` と返り行数を一致させる
/// (preview 1 行 / expanded N 行 + ラベル 1 行)。
///
/// `status` が `Some` のとき、ラベル行末尾に **2 文字ぶん** (`"  ⠋"` のように
/// 半角スペース 2 + マーカー文字 1) のステータスマーカーを追加する。マーカーは
/// ラベル行の末尾 Span として乗るだけで、`tool_use_height` (= ラベル行 1 行 +
/// preview/expanded 行) には影響しない。`spinner_phase` は `InProgress` のとき
/// だけ `SPINNER_FRAMES` の index 計算に使う。
fn push_tool_use_lines<'a>(
	lines: &mut Vec<Line<'a>>,
	name: &str,
	input: &Value,
	expanded: bool,
	wrap_width: usize,
	status: Option<ToolStatus>,
	spinner_phase: u8,
) {
	let display_name = if name == "Bash" {
		input
			.get("command")
			.and_then(Value::as_str)
			.and_then(|cmd| cmd.split('\n').next())
			.and_then(git_bash_preview)
			.map(|label| format!("Bash({label})"))
			.unwrap_or_else(|| name.to_string())
	} else {
		name.to_string()
	};
	let mut label_spans: Vec<Span<'a>> = Vec::with_capacity(if status.is_some() { 3 } else { 1 });
	label_spans.push(Span::styled(
		display_name,
		Style::default()
			.fg(Color::Magenta)
			.add_modifier(Modifier::BOLD),
	));
	if let Some(s) = status {
		let (ch, color) = match s {
			ToolStatus::InProgress => {
				let idx = (spinner_phase as usize) % SPINNER_FRAMES.len();
				(SPINNER_FRAMES[idx], Color::Magenta)
			}
			ToolStatus::Done => (DONE_MARKER, Color::Green),
			ToolStatus::Errored => (ERROR_MARKER, Color::Red),
		};
		label_spans.push(Span::raw("  "));
		label_spans.push(Span::styled(
			ch.to_string(),
			Style::default().fg(color).add_modifier(Modifier::BOLD),
		));
	}
	lines.push(Line::from(label_spans));
	if expanded {
		let pretty = serde_json::to_string_pretty(input).unwrap_or_default();
		let gray = Style::default().fg(Color::Gray);
		for wrapped in wrap_lines_iter(&pretty, wrap_width) {
			lines.push(Line::from(Span::styled(wrapped.to_string(), gray)));
		}
	} else {
		// estimate は preview 行を **常に 1 行** カウントする (`tool_use_height`
		// の `+1` 固定)。preview が空文字でも `Paragraph` が空 1 行を描画する
		// ので 1 Line push しておかないと estimate と乖離する。
		let preview = format_tool_preview(name, input);
		lines.push(Line::from(Span::styled(
			preview,
			Style::default().fg(Color::Gray),
		)));
	}
}

/// tool_result 用の本文 (ラベル + preview / 全文) を `lines` に push する。
///
/// `estimate_bubble_height` の `tool_result_height` と返り行数を一致させる。
///
/// `indent` > 0 のとき、ラベル行 / preview 行 / 展開時の全 wrapped 行の先頭に
/// `indent` 文字ぶんの空白を足す (Slack スレッド返信風の字下げ)。expanded 時は
/// 折り返し幅も `wrap_width - indent` に縮めて、見積もり
/// (`estimate_bubble_height` の統合バブル分岐で `content_width - RESULT_INDENT`)
/// と一致させる。orphan な tool_result は `indent=0` で従来どおり。
fn push_tool_result_lines<'a>(
	lines: &mut Vec<Line<'a>>,
	tr: &ToolResult,
	expanded: bool,
	wrap_width: usize,
	indent: usize,
) {
	let color = if tr.is_error {
		Color::Red
	} else {
		Color::Magenta
	};
	let indent_str: String = " ".repeat(indent);
	let label_style_arrow = Style::default().fg(color).add_modifier(Modifier::BOLD);
	let label_style_text = Style::default().fg(color).add_modifier(Modifier::BOLD);
	let mut label_spans: Vec<Span<'a>> = Vec::with_capacity(if indent > 0 { 3 } else { 2 });
	if indent > 0 {
		label_spans.push(Span::raw(indent_str.clone()));
	}
	label_spans.push(Span::styled("↪ ", label_style_arrow));
	label_spans.push(Span::styled(
		if tr.is_error { "error" } else { "result" },
		label_style_text,
	));
	lines.push(Line::from(label_spans));

	let gray = Style::default().fg(Color::Gray);
	if expanded {
		// 折り返し幅を indent ぶん縮める。wrap_width <= indent の極端ケースでも
		// 1 以上にして panic / 0 行化を避ける (estimate 側と同じクランプ)。
		let effective_width = wrap_width.saturating_sub(indent).max(1);
		for wrapped in wrap_lines_iter(&tr.content, effective_width) {
			let mut spans: Vec<Span<'a>> = Vec::with_capacity(if indent > 0 { 2 } else { 1 });
			if indent > 0 {
				spans.push(Span::raw(indent_str.clone()));
			}
			spans.push(Span::styled(wrapped.to_string(), gray));
			lines.push(Line::from(spans));
		}
	} else {
		let preview = format_result_preview(&tr.content);
		let mut spans: Vec<Span<'a>> = Vec::with_capacity(if indent > 0 { 2 } else { 1 });
		if indent > 0 {
			spans.push(Span::raw(indent_str.clone()));
		}
		spans.push(Span::styled(preview, gray));
		lines.push(Line::from(spans));
	}
}

/// 吹き出しの左右寄せ + 左ボーダー色をかけた上で、1 本の `BubbleRow` を 1 行に
/// 描画する。
///
/// `row_area` は `y` が 1 行分の矩形 (height=1)。
///
/// - `Default` / `Line` モード: Main=右寄せ、Sub=左寄せ
/// - `Slack` モード: sender に関係なくすべて左寄せ
///
/// sender 色: Main=黄 / Sub=緑。色は寄せと独立で、slack モードでも「誰の発言か」
/// を識別できるよう現状色を維持する。`Default` / `Slack` では `Body` の左 1
/// 文字に細い縦線マーカー `▏`、`Line` では `Body` の左右に `│`、`TopBorder` /
/// `BottomBorder` ではその Line 自体が `╭─...─╮` / `╰─...─╯` を持っている
/// ので中身をそのまま書き出す。
///
/// `Margin` kind はバブル間スペーサーの空行で、**どのモードでも何も描かない**
/// (row を消費するだけ)。枠線 `│` / 縦線マーカー `▏` を付けてしまうと、実機
/// で「下枠 `╰──╯` の次の空行に `│ │` が残って見える」症状になるため。
///
/// 本文は `render_bubble_rows` の段階で既に `content_width` (あるいは LINE の
/// `content_width - 2`) で折り返し済みなので、ここでは **`Paragraph::wrap` を
/// 挟まず `buf.set_line` で 1 行直書き** する。Paragraph 経由だと `Line::clone`
/// + `WordWrapper` + `unicode-width` の走査が row 数ぶん走ってホットパスで効いてくる。
pub fn draw_bubble_row(
	row_area: Rect,
	buf: &mut Buffer,
	row: &BubbleRow<'_>,
	sender: Sender,
	bubble_width: u16,
	chat_mode: ChatMode,
) {
	if row_area.width == 0 || row_area.height == 0 {
		return;
	}
	// `Margin` はバブル間スペーサー (枠外の空行)。どのモードでも何も描かない。
	// LINE モードで `╰──╯` の次の空行に `│ │` が残るバグ、
	// default / slack モードで末尾空行の先頭に `▏` が付く見た目の違和感、
	// 両方を同じルール (= 「Margin 行は全部スキップ」) で防ぐ。
	if row.kind == BubbleRowKind::Margin {
		return;
	}
	let width = bubble_width.min(row_area.width).max(2);
	let x_offset = bubble_x_offset(row_area.x, row_area.width, width, sender, chat_mode);
	let border_color = match sender {
		Sender::Main => Color::Yellow,
		Sender::Sub => Color::Green,
	};
	let border_style = Style::default().fg(border_color);

	match (chat_mode, row.kind) {
		(ChatMode::Line, BubbleRowKind::TopBorder)
		| (ChatMode::Line, BubbleRowKind::BottomBorder) => {
			// 境界行は `Line` が丸ごと `╭──...──╮` / `╰──...──╯` を持っている。
			// set_line で bubble_width に clamp しながら書く。
			buf.set_line(x_offset, row_area.y, &row.line, width);
		}
		(ChatMode::Line, BubbleRowKind::Body) => {
			// 左右に `│` を付け、本文は間の width-2 に描く。
			let v = LINE_V.to_string();
			buf.set_stringn(x_offset, row_area.y, v.as_str(), 1, border_style);
			let body_width = width.saturating_sub(2);
			if body_width > 0 {
				buf.set_line(x_offset + 1, row_area.y, &row.line, body_width);
			}
			let right_x = x_offset.saturating_add(width.saturating_sub(1));
			if width >= 2 {
				buf.set_stringn(right_x, row_area.y, v.as_str(), 1, border_style);
			}
		}
		(ChatMode::Default, BubbleRowKind::Body) | (ChatMode::Slack, BubbleRowKind::Body) => {
			// 1 文字目: 左ボーダー "▏" (細い縦線) を sender 色で
			buf.set_stringn(x_offset, row_area.y, "▏", 1, border_style);
			let body_width = width.saturating_sub(1);
			if body_width > 0 {
				buf.set_line(x_offset + 1, row_area.y, &row.line, body_width);
			}
		}
		// default / slack モードで TopBorder / BottomBorder が来るケースは
		// render_bubble_rows 側で生成されないが、防御的に何も描かずスキップ。
		(ChatMode::Default, _) | (ChatMode::Slack, _) => {}
		// LINE + Margin は上で早期リターン済み (到達不能)。
		(ChatMode::Line, BubbleRowKind::Margin) => {}
	}
}

/// bubble の左上 x 座標を算出する。寄せだけを切り出した pure 関数。
///
/// - `Default` / `Line`: Main=右寄せ / Sub=左寄せ
/// - `Slack`: すべて左寄せ (sender 問わず `area_x`)
///
/// テスト性のため公開 (`pub(crate)`)。`chat_mode` は Copy なので clone 不要。
pub(crate) fn bubble_x_offset(
	area_x: u16,
	area_width: u16,
	bubble_width: u16,
	sender: Sender,
	chat_mode: ChatMode,
) -> u16 {
	if chat_mode == ChatMode::Slack {
		return area_x;
	}
	match sender {
		Sender::Main => area_x + area_width.saturating_sub(bubble_width),
		Sender::Sub => area_x,
	}
}

/// 吹き出し幅を `area.width` と `chat_mode` から計算する。
///
/// - `Default` / `Line`: 端末幅の 60% (最低 30 列)。Main=右 / Sub=左 の対向
///   寄せで Slack / Discord 風の会話感を出すため、左右に余白を残す
/// - `Slack`: **端末幅ぜんぶ** (`area_width` そのまま)。Slack は全バブル
///   左寄せなので、右側に余白を残すと間延びして見えるため幅を最大化する
pub fn compute_bubble_width(area_width: u16, chat_mode: ChatMode) -> u16 {
	if chat_mode == ChatMode::Slack {
		return area_width;
	}
	let scaled = (area_width as u32 * 60 / 100) as u16;
	scaled.clamp(30u16.min(area_width), area_width)
}

/// `render_bubble_rows` / `estimate_bubble_height_full` / `HeightsCache` に
/// 渡す **`content_width`** を計算する。これらは全部同じ値を共有しないと
/// 「見積もり == 実描画行数」不変条件や bubble 枠の整列が壊れる。
///
/// ## モード別の意味論
///
/// - **`Default` / `Slack`**: `content_width = bubble_width - 1`。
///   左 1 文字は `▏` マーカー専用で draw_bubble_row が描く。本文領域は
///   `content_width` 列そのもの。
/// - **`Line`**: `content_width = bubble_width`。4 辺枠線は **bubble 全幅**
///   で描くので、TopBorder / BottomBorder の `╭──...──╮` / `╰──...──╯` の
///   長さも `content_width = bubble_width`。本文 (Body) は左右の `│` で 2 列
///   食われるため、折り返し幅は内部で `content_width - 2` (= `effective_inner_width`)
///   になる。**draw_bubble_row の右 `│` 位置 (`x_offset + bubble_width - 1`)
///   と TopBorder の `╮` 位置 (`x_offset + content_width - 1`) を揃える**
///   ため `bubble_width - 1 == content_width - 1` が必要 → `content_width ==
///   bubble_width` と置く設計に寄せる。
///
/// 最低 10 にクランプして、`estimate_bubble_height_full` 内部の `max(10)`
/// ガードと同じ下限を共有する。
///
/// この helper を `watching.rs` / `App::current_content_width` /
/// `HeightsCache::sync` の 3 経路すべてから呼ぶことで、同一フレーム内で
/// `content_width` の食い違いが起きないよう中央集権化する。
pub fn compute_content_width(area_width: u16, chat_mode: ChatMode) -> u16 {
	let bubble_width = compute_bubble_width(area_width, chat_mode);
	match chat_mode {
		ChatMode::Line => bubble_width.max(10),
		ChatMode::Default | ChatMode::Slack => bubble_width.saturating_sub(1).max(10),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::domain::entities::{FormattedMessage, Sender};
	use chrono::Utc;

	fn text_message(text: &str, sender: Sender) -> FormattedMessage {
		FormattedMessage {
			id: "m-0".to_string(),
			sender,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: Some(text.to_string()),
			tool_use: None,
			tool_result: None,
			tool_use_id: None,
			result_timestamp: None,
			is_final_response: false,
		}
	}

	#[test]
	fn lines_include_header_body_and_margin() {
		let msg = text_message("hello", Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 80, 0);
		// header + 1 body + trailing blank = 3
		assert_eq!(lines.len(), 3);
	}

	#[test]
	fn selected_header_contains_marker() {
		let msg = text_message("x", Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Plan",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: true,
		};
		let lines = render_bubble_lines(&model, 80, 0);
		let header = lines.first().unwrap();
		let combined: String = header
			.spans
			.iter()
			.map(|s| s.content.as_ref())
			.collect::<Vec<&str>>()
			.join("");
		assert!(combined.contains("◄ selected"));
	}

	#[test]
	fn multiline_text_produces_multiple_lines() {
		let msg = text_message("a\nb\nc", Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 80, 0);
		// header + 3 body + trailing blank
		assert_eq!(lines.len(), 5);
	}

	#[test]
	fn compute_bubble_width_returns_sixty_percent_for_default_and_line() {
		for mode in [ChatMode::Default, ChatMode::Line] {
			assert_eq!(compute_bubble_width(100, mode), 60, "mode {mode:?}");
			// 60% of 10 = 6 < 30; clamped to area_width since area_width < 30
			assert_eq!(compute_bubble_width(10, mode), 10, "mode {mode:?}");
		}
	}

	#[test]
	fn compute_bubble_width_slack_uses_full_area_width() {
		// Slack モードは全バブル左寄せなので端末幅いっぱいに広げる。
		assert_eq!(compute_bubble_width(100, ChatMode::Slack), 100);
		assert_eq!(compute_bubble_width(10, ChatMode::Slack), 10);
		assert_eq!(compute_bubble_width(200, ChatMode::Slack), 200);
	}

	/// `compute_content_width` のモード別契約:
	/// - Default / Slack: `bubble_width - 1` (左 `▏` 1 セル)
	/// - Line: `bubble_width` そのもの (4 辺枠線が bubble 全幅 = content_width)
	///
	/// この値は watching.rs / App::current_content_width / HeightsCache::sync が
	/// 全部同じ計算を共有するための中央集権化された契約。壊すと `╯` と右 `│` が
	/// 1 セルずれる (実機で報告された off-by-one の根本原因)。
	#[test]
	fn compute_content_width_contract_per_mode() {
		// area=100 → bubble=60 の共通ケース
		// Default / Slack は bubble - 1 = 59、Line は bubble = 60
		assert_eq!(compute_content_width(100, ChatMode::Default), 59);
		assert_eq!(compute_content_width(100, ChatMode::Line), 60);
		// Slack は bubble=100 (全幅) → content=99
		assert_eq!(compute_content_width(100, ChatMode::Slack), 99);

		// area=80 → bubble=48
		assert_eq!(compute_content_width(80, ChatMode::Default), 47);
		assert_eq!(compute_content_width(80, ChatMode::Line), 48);
		assert_eq!(compute_content_width(80, ChatMode::Slack), 79);

		// area=83 (odd) → bubble=49 (odd)
		assert_eq!(compute_content_width(83, ChatMode::Line), 49);
		// area=51 → bubble clamp=30
		assert_eq!(compute_content_width(51, ChatMode::Line), 30);

		// 下限クランプ (max(10))
		assert_eq!(compute_content_width(5, ChatMode::Line), 10);
		assert_eq!(compute_content_width(5, ChatMode::Default), 10);
	}

	/// `compute_content_width` で出した値で `render_bubble_rows` を走らせたとき、
	/// LINE モードの TopBorder `╭──...──╮` の **表示幅が bubble_width と一致**する。
	/// これが成り立つ限り draw_bubble_row で右 `│` の x (`bubble_width-1`) と
	/// `╯` の x (`content_width-1`) がズレない。
	#[test]
	fn line_mode_top_border_length_equals_bubble_width() {
		for area_width in [100u16, 80, 83, 51, 40] {
			let bubble_width = compute_bubble_width(area_width, ChatMode::Line);
			let content_width = compute_content_width(area_width, ChatMode::Line);

			// 契約: LINE では content_width == bubble_width (clamp が効いた場合のみ
			// content_width が bubble_width より大きくなり得るが、その場合も
			// 描画時は bubble_width に set_line で clamp されるので右 `│` とは揃う)
			assert!(
				content_width >= bubble_width,
				"area={area_width}: LINE content_width ({content_width}) must be >= bubble_width ({bubble_width})",
			);

			// TopBorder の文字数 (`╭` + `─`×(content_width-2) + `╮`) == content_width
			let msg = text_message("x", Sender::Sub);
			let model = ChatBubbleModel {
				message: &msg,
				agent_type: "Explore",
				is_detailed_mode: false,
				is_expanded: false,
				is_selected: false,
			};
			let rows = render_bubble_rows(&model, content_width, ChatMode::Line, 0);
			let top_line = &rows[0].line;
			let top_str: String = top_line
				.spans
				.iter()
				.map(|s| s.content.as_ref())
				.collect::<Vec<&str>>()
				.join("");
			assert_eq!(
				top_str.chars().count(),
				content_width as usize,
				"area={area_width}: TopBorder char count must equal content_width",
			);
			// 右端は `╮` (draw 時に bubble_width で clamp されて右 `│` と揃う)
			assert_eq!(
				top_str.chars().last().unwrap(),
				'╮',
				"area={area_width}: TopBorder must end with ╮",
			);
		}
	}

	/// `estimate_bubble_height` と `render_bubble_lines` の行数が一致することを
	/// 検証する (follow_tail 時に viewport 末尾に空白が残るバグの回帰防止)。
	///
	/// 長い行 (content_width を超える) を持つテキスト / 展開時の tool_result で、
	/// 以前は estimate が折り返し後の行数をカウントしていたのに対し
	/// render は `\n` 分割だけで折り返さず、draw_bubble_row が 1 行矩形に truncate
	/// していた。結果 estimate > actual で、max_offset が過大になり follow_tail
	/// 時に最新メッセージが画面下端に届かなくなっていた。
	#[test]
	fn estimate_matches_rendered_line_count_for_long_text() {
		use crate::ui::layout::estimate_bubble_height;

		let long = "a".repeat(200); // 200 文字 / content_width=40 → 5 行に折り返す
		let msg = text_message(&long, Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 40;
		let estimated = estimate_bubble_height(&msg, false, content_width);
		let actual = render_bubble_lines(&model, content_width, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"estimate ({estimated}) must match actual rendered lines ({actual})"
		);
	}

	#[test]
	fn estimate_matches_rendered_line_count_for_expanded_tool_result() {
		use crate::domain::entities::ToolResult;
		use crate::ui::layout::estimate_bubble_height;

		let content = (0..10)
			.map(|i| format!("line{i} ") + &"x".repeat(120))
			.collect::<Vec<_>>()
			.join("\n");
		let msg = FormattedMessage {
			id: "m-1".to_string(),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: None,
			tool_use: None,
			tool_result: Some(ToolResult {
				content,
				is_error: false,
			}),
			tool_use_id: None,
			result_timestamp: None,
			is_final_response: false,
		};
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: true, // 展開モード → 全文が行展開される
			is_expanded: true,
			is_selected: false,
		};
		let content_width: u16 = 50;
		let estimated = estimate_bubble_height(&msg, true, content_width);
		let actual = render_bubble_lines(&model, content_width, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"estimate ({estimated}) must match actual rendered lines ({actual})"
		);
	}

	// ----------------------------------------------------------------------
	// 統合バブル (tool_use + tool_result が pair 済み) の回帰テスト
	// ----------------------------------------------------------------------

	use crate::domain::entities::{ToolResult, ToolUse};

	fn paired_message(
		tool_use_id: &str,
		input: serde_json::Value,
		result_content: &str,
		is_error: bool,
	) -> FormattedMessage {
		FormattedMessage {
			id: format!("m-{tool_use_id}"),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: None,
			tool_use: Some(ToolUse {
				name: "Bash".to_string(),
				input,
			}),
			tool_result: Some(ToolResult {
				content: result_content.to_string(),
				is_error,
			}),
			tool_use_id: Some(tool_use_id.to_string()),
			result_timestamp: Some(Utc::now()),
			is_final_response: false,
		}
	}

	/// 統合バブル (tool_use + tool_result 両方 Some) の preview 描画行数 ==
	/// `estimate_bubble_height` の返り値。lessons.md の「見積もり == 実描画行数」
	/// 原則に従う回帰テスト。
	///
	/// **新仕様**: closed (preview) では tool_result セクションを出さない。
	/// shape は単独 tool_use と同じ (header + tool_use(2) + margin = 4)。
	#[test]
	fn estimate_matches_rendered_for_integrated_bubble_preview() {
		use crate::ui::layout::estimate_bubble_height;

		let msg = paired_message(
			"tu-1",
			serde_json::json!({"command": "echo hello world"}),
			"hello world",
			false,
		);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 60;
		let estimated = estimate_bubble_height(&msg, false, content_width);
		let actual = render_bubble_lines(&model, content_width, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"integrated bubble preview: estimate ({estimated}) must match actual rendered lines ({actual})"
		);
		// closed = header + tool_use(2) + margin = 4 (= 単独 tool_use と同じ)
		assert_eq!(
			actual, 4,
			"closed integrated bubble shape: tool_use のみで 4 行 (result 省略)"
		);
	}

	/// closed 状態の統合バブルでは tool_result の `↪` ラベル / 区切り空行 /
	/// preview のいずれも出さない (ノイズ削減のミニマム表示)。
	#[test]
	fn closed_integrated_bubble_omits_result_section() {
		let msg = paired_message(
			"tu-min",
			serde_json::json!({"command": "ls"}),
			"file1\nfile2",
			false,
		);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 60, 0);
		let combined: String = lines
			.iter()
			.flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
			.collect::<Vec<&str>>()
			.join("");
		assert!(
			!combined.contains("↪"),
			"closed combined bubble must not include result arrow, got {combined:?}"
		);
		assert!(
			!combined.contains("file1") && !combined.contains("file2"),
			"closed combined bubble must not include result preview content, got {combined:?}"
		);
	}

	#[test]
	fn estimate_matches_rendered_for_integrated_bubble_expanded() {
		use crate::ui::layout::estimate_bubble_height;

		let long_input = serde_json::json!({
			"command": (0..5)
				.map(|i| format!("echo line{i} {}", "x".repeat(80)))
				.collect::<Vec<_>>()
				.join("\n"),
		});
		let long_result = (0..8)
			.map(|i| format!("result-line{i} ") + &"y".repeat(100))
			.collect::<Vec<_>>()
			.join("\n");

		let msg = paired_message("tu-2", long_input, &long_result, false);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: true,
			is_expanded: true,
			is_selected: false,
		};
		let content_width: u16 = 50;
		let estimated = estimate_bubble_height(&msg, true, content_width);
		let actual = render_bubble_lines(&model, content_width, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"integrated bubble expanded: estimate ({estimated}) must match actual rendered lines ({actual})"
		);
	}

	/// `is_error=true` の統合バブルでも estimate / render が一致する。
	#[test]
	fn estimate_matches_rendered_for_integrated_bubble_with_error_result() {
		use crate::ui::layout::estimate_bubble_height;

		let msg = paired_message(
			"tu-err",
			serde_json::json!({"command": "false"}),
			"command failed: exit 1",
			true,
		);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 60;
		assert_eq!(
			estimate_bubble_height(&msg, false, content_width),
			render_bubble_lines(&model, content_width, 0).len() as u16
		);
	}

	// ----------------------------------------------------------------------
	// 統合バブル expanded 表示 (インデントなし) の回帰テスト
	// ----------------------------------------------------------------------

	/// **新仕様**: 統合バブルを展開したときの tool_result ラベル / 本文は
	/// インデント無し (`RESULT_INDENT=0`) で描く。orphan tool_result と同じ。
	#[test]
	fn expanded_integrated_bubble_result_has_no_indent() {
		let msg = paired_message(
			"tu-i",
			serde_json::json!({"command": "echo hi"}),
			"hi",
			false,
		);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: true,
			is_expanded: true,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 60, 0);

		// expanded = header + tool_use(label + 全文) + 区切り + tool_result label +
		// 全文 + margin。tool_result label 行を spans 結合で取り出して `↪ ` 始まり
		// (= インデントなし) を確認する。
		let label_line = lines
			.iter()
			.find(|l| {
				l.spans
					.iter()
					.any(|s| s.content.as_ref() == "result" || s.content.as_ref() == "error")
			})
			.expect("expanded integrated bubble must include result label line");
		let combined: String = label_line
			.spans
			.iter()
			.map(|s| s.content.as_ref())
			.collect::<Vec<&str>>()
			.join("");
		assert!(
			combined.starts_with("↪ "),
			"expanded integrated bubble result label must start with '↪ ' (no indent), got {combined:?}"
		);
		assert!(combined.contains("result"));
	}

	/// `is_error=true` でも error ラベル (`↪ error`) はインデントなしで描かれる。
	#[test]
	fn expanded_integrated_bubble_error_label_has_no_indent() {
		let msg = paired_message(
			"tu-ie",
			serde_json::json!({"command": "false"}),
			"failed",
			true,
		);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: true,
			is_expanded: true,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 60, 0);
		let label_line = lines
			.iter()
			.find(|l| {
				l.spans
					.iter()
					.any(|s| s.content.as_ref() == "result" || s.content.as_ref() == "error")
			})
			.expect("expanded integrated bubble must include error label line");
		let combined: String = label_line
			.spans
			.iter()
			.map(|s| s.content.as_ref())
			.collect::<Vec<&str>>()
			.join("");
		assert!(
			combined.starts_with("↪ "),
			"expanded integrated bubble error label must start with '↪ ' (no indent), got {combined:?}"
		);
		assert!(combined.contains("error"));
	}

	/// orphan tool_result (対応する tool_use なし) は従来どおりインデントされない。
	#[test]
	fn orphan_tool_result_has_no_indent() {
		use crate::domain::entities::ToolResult;

		let msg = FormattedMessage {
			id: "m-orphan".to_string(),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: None,
			tool_use: None,
			tool_result: Some(ToolResult {
				content: "some result".to_string(),
				is_error: false,
			}),
			tool_use_id: None,
			result_timestamp: None,
			is_final_response: false,
		};
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 60, 0);
		// header (0) + tool_result label (1) + preview (2) + margin (3)
		let label_line = &lines[1];
		let combined: String = label_line
			.spans
			.iter()
			.map(|s| s.content.as_ref())
			.collect::<Vec<&str>>()
			.join("");
		assert!(
			combined.starts_with("↪ "),
			"orphan tool_result label must start with '↪ ' (no indent), got {combined:?}"
		);
	}

	/// 長い tool_result を含む expanded 統合バブルで estimate == render を担保。
	/// `RESULT_INDENT=0` 化後も折り返し幅 (`inner_width`) が estimate 側と render 側
	/// で揃っていることを保証する回帰テスト。
	#[test]
	fn estimate_matches_rendered_for_integrated_bubble_expanded_long_result() {
		use crate::ui::layout::estimate_bubble_height;

		let long_result = (0..6)
			.map(|i| format!("row-{i} ") + &"z".repeat(140))
			.collect::<Vec<_>>()
			.join("\n");
		let msg = paired_message(
			"tu-exp",
			serde_json::json!({"command": "seq 100"}),
			&long_result,
			false,
		);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: true,
			is_expanded: true,
			is_selected: false,
		};
		let content_width: u16 = 50;
		let estimated = estimate_bubble_height(&msg, true, content_width);
		let actual = render_bubble_lines(&model, content_width, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"expanded integrated bubble (long result): estimate ({estimated}) must match actual rendered lines ({actual})"
		);
	}

	// ----------------------------------------------------------------------
	// mention prefix (@Main / @{agent_type}) + 中間 text の薄色化の回帰テスト
	// ----------------------------------------------------------------------

	/// Main からの prompt は 1 行目の先頭に `@{agent_type} ` Span が入る。
	#[test]
	fn main_text_includes_agent_type_mention_prefix() {
		let msg = text_message("please investigate X", Sender::Main);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 80, 0);
		// header (0) + body 1 行目 (1) + margin (2)
		let body_line = &lines[1];
		let first_span = body_line.spans.first().expect("body has span");
		assert_eq!(first_span.content.as_ref(), "@Explore ");
		assert_eq!(first_span.style.fg, Some(Color::Cyan));
		assert!(first_span.style.add_modifier.contains(Modifier::BOLD));
	}

	/// Sub + is_final_response=true (end_turn 末尾 text) は `@Main ` prefix が入る。
	/// 本文の color は default (薄色化しない = 最終応答は通常色で強調)。
	#[test]
	fn sub_final_response_text_includes_main_mention_prefix() {
		let mut msg = text_message("done. results below.", Sender::Sub);
		msg.is_final_response = true;
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 80, 0);
		let body_line = &lines[1];
		let first_span = body_line.spans.first().expect("body has span");
		assert_eq!(first_span.content.as_ref(), "@Main ");
		assert_eq!(first_span.style.fg, Some(Color::Cyan));
		// 本文 span (2 つ目) は default (DarkGray ではない)
		let body_span = body_line.spans.get(1).expect("body span after mention");
		assert_ne!(
			body_span.style.fg,
			Some(Color::DarkGray),
			"最終応答は独り言扱いではないので薄色化しない"
		);
	}

	/// Sub + is_final_response=false (中間 text / 独り言) は mention なし +
	/// 本文全体が DarkGray で薄色化される。
	#[test]
	fn sub_intermediate_text_is_dimmed_and_has_no_mention() {
		let msg = text_message("thinking out loud...", Sender::Sub);
		assert!(!msg.is_final_response);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 80, 0);
		let body_line = &lines[1];
		// 1 つの span (mention なし)
		assert_eq!(
			body_line.spans.len(),
			1,
			"no mention prefix for intermediate"
		);
		let body_span = &body_line.spans[0];
		assert!(
			!body_span.content.contains('@'),
			"intermediate text must not start with mention, got {:?}",
			body_span.content
		);
		assert_eq!(
			body_span.style.fg,
			Some(Color::DarkGray),
			"intermediate text body must be DarkGray for dim appearance"
		);
	}

	/// tool_use / tool_result バブル (text=None) は mention prefix / 薄色化の
	/// 対象外。Sender::Sub + is_final_response=false でも body は従来通りの
	/// 色 (tool_use は magenta、preview は gray など) を維持する。
	#[test]
	fn tool_use_bubble_does_not_get_mention_or_dim() {
		let msg = paired_message(
			"tu-1",
			serde_json::json!({"command": "echo hi"}),
			"hi",
			false,
		);
		assert!(!msg.is_final_response);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 60, 0);
		// tool_use label 行 (lines[1]) には mention Span (Cyan / "@..") が入らない
		let tool_label = &lines[1];
		for sp in &tool_label.spans {
			assert!(
				!sp.content.starts_with("@"),
				"tool_use label must not carry mention prefix, got {:?}",
				sp.content
			);
		}
	}

	/// mention あり (Main) の長文で、estimate_bubble_height と実描画行数が一致する。
	/// mention ぶん 1 行目の wrap 幅が縮むため、本文が長いと estimate 側が
	/// `wrap_line_count_with_prefix` で正しく計算できていないと破綻する。
	#[test]
	fn estimate_matches_rendered_for_main_text_with_mention_long_body() {
		use crate::ui::layout::estimate_bubble_height_with_prefix;

		// 260 文字。content_width=40 & mention "@general-purpose " (17 文字)
		// → first_width=23、残りは 40 幅で折り返される
		let body = "a".repeat(260);
		let msg = text_message(&body, Sender::Main);
		let agent_type = "general-purpose";
		let model = ChatBubbleModel {
			message: &msg,
			agent_type,
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 40;
		let estimated = estimate_bubble_height_with_prefix(&msg, false, content_width, agent_type);
		let actual = render_bubble_lines(&model, content_width, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"mention-long-body: estimate ({estimated}) must match rendered ({actual})"
		);
	}

	/// mention あり (Sub + final_response) の複数行本文でも estimate/render が一致。
	#[test]
	fn estimate_matches_rendered_for_sub_final_response_with_mention_multiline() {
		use crate::ui::layout::estimate_bubble_height_with_prefix;

		// 複数論理行を持つ本文。1 行目だけ mention ぶん縮む。
		let body = format!("{}\n{}\n{}", "x".repeat(100), "y".repeat(60), "z");
		let mut msg = text_message(&body, Sender::Sub);
		msg.is_final_response = true;
		let agent_type = "Explore";
		let model = ChatBubbleModel {
			message: &msg,
			agent_type,
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 30;
		let estimated = estimate_bubble_height_with_prefix(&msg, false, content_width, agent_type);
		let actual = render_bubble_lines(&model, content_width, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"sub-final-multiline: estimate ({estimated}) must match rendered ({actual})"
		);
	}

	/// mention なし (Sub 中間 text) の長文で estimate/render が一致する
	/// (従来の `wrap_line_count` 経路が維持されている回帰テスト)。
	#[test]
	fn estimate_matches_rendered_for_intermediate_text_without_mention() {
		use crate::ui::layout::estimate_bubble_height;

		let body = "x".repeat(300);
		let msg = text_message(&body, Sender::Sub);
		assert!(!msg.is_final_response);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 40;
		let estimated = estimate_bubble_height(&msg, false, content_width);
		let actual = render_bubble_lines(&model, content_width, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"intermediate-no-mention: estimate ({estimated}) must match rendered ({actual})"
		);
	}

	// ----------------------------------------------------------------------
	// chat_mode: Slack モードの寄せのテスト
	// ----------------------------------------------------------------------

	/// Slack モードでは sender に関係なく bubble の左上 x は `area_x` に揃う。
	#[test]
	fn bubble_x_offset_slack_mode_always_left_aligned() {
		// Main / Sub 共に同じ (= area_x)
		assert_eq!(
			bubble_x_offset(0, 100, 30, Sender::Main, ChatMode::Slack),
			0,
		);
		assert_eq!(bubble_x_offset(0, 100, 30, Sender::Sub, ChatMode::Slack), 0,);
		// area_x が非ゼロでも同様
		assert_eq!(bubble_x_offset(5, 80, 30, Sender::Main, ChatMode::Slack), 5,);
		assert_eq!(bubble_x_offset(5, 80, 30, Sender::Sub, ChatMode::Slack), 5,);
	}

	/// Default モードは Main=右寄せ / Sub=左寄せ (従来挙動を維持)。
	#[test]
	fn bubble_x_offset_default_mode_preserves_sender_alignment() {
		// area_x=0, area_width=100, bubble_width=30
		assert_eq!(
			bubble_x_offset(0, 100, 30, Sender::Main, ChatMode::Default),
			70, // 100 - 30 = 70
		);
		assert_eq!(
			bubble_x_offset(0, 100, 30, Sender::Sub, ChatMode::Default),
			0,
		);
	}

	/// Line モードも寄せは default と同じ (Main=右 / Sub=左)。枠線の見た目だけ違う。
	#[test]
	fn bubble_x_offset_line_mode_preserves_sender_alignment() {
		assert_eq!(
			bubble_x_offset(0, 100, 30, Sender::Main, ChatMode::Line),
			70,
		);
		assert_eq!(bubble_x_offset(0, 100, 30, Sender::Sub, ChatMode::Line), 0,);
	}

	/// Slack モードでも左ボーダー色は sender 別に維持される (Main=黄 / Sub=緑)。
	/// Buffer を実際に描画して、左端セルの fg color を確認する。
	#[test]
	fn slack_mode_keeps_sender_border_color() {
		use ratatui::buffer::Buffer;

		let width: u16 = 60;
		let rect = Rect {
			x: 0,
			y: 0,
			width,
			height: 1,
		};
		let row = BubbleRow {
			kind: BubbleRowKind::Body,
			line: Line::from("hello"),
		};

		// Sender::Main でも Slack モードでは左に描かれ、かつ左端色が Yellow
		let mut buf_main = Buffer::empty(rect);
		draw_bubble_row(rect, &mut buf_main, &row, Sender::Main, 30, ChatMode::Slack);
		let left_cell_main = buf_main.cell((0, 0)).unwrap();
		assert_eq!(left_cell_main.symbol(), "▏");
		assert_eq!(left_cell_main.fg, Color::Yellow);

		// Sender::Sub でも同じく左に描かれ、左端色が Green
		let mut buf_sub = Buffer::empty(rect);
		draw_bubble_row(rect, &mut buf_sub, &row, Sender::Sub, 30, ChatMode::Slack);
		let left_cell_sub = buf_sub.cell((0, 0)).unwrap();
		assert_eq!(left_cell_sub.symbol(), "▏");
		assert_eq!(left_cell_sub.fg, Color::Green);
	}

	/// Default モードで Sender::Main は右寄せされる (従来挙動の回帰確認)。
	/// Buffer を描画して「左端セルは空 (= 描画されていない)」「右側に border 出現」
	/// を確認する。
	#[test]
	fn default_mode_main_sender_right_aligned_in_buffer() {
		use ratatui::buffer::Buffer;

		let area_width: u16 = 100;
		let bubble_width: u16 = 30;
		let rect = Rect {
			x: 0,
			y: 0,
			width: area_width,
			height: 1,
		};
		let row = BubbleRow {
			kind: BubbleRowKind::Body,
			line: Line::from("hi"),
		};
		let mut buf = Buffer::empty(rect);
		draw_bubble_row(
			rect,
			&mut buf,
			&row,
			Sender::Main,
			bubble_width,
			ChatMode::Default,
		);

		// 左端セル (x=0) は描画されていない (空白のまま)
		let left_cell = buf.cell((0, 0)).unwrap();
		assert_ne!(left_cell.symbol(), "▏", "left cell must be untouched");

		// bubble の左上 x = area_width - bubble_width = 70 に border が描かれる
		let border_cell = buf.cell((70, 0)).unwrap();
		assert_eq!(border_cell.symbol(), "▏");
		assert_eq!(border_cell.fg, Color::Yellow);
	}

	// ----------------------------------------------------------------------
	// chat_mode: Line モード (4 辺枠線) の回帰テスト
	// ----------------------------------------------------------------------

	/// LINE モードの render_bubble_rows は top/bottom border を前後に足し、行数が
	/// default より 2 多い。
	#[test]
	fn line_mode_adds_two_border_rows() {
		let msg = text_message("hi", Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let default_rows = render_bubble_rows(&model, 80, ChatMode::Default, 0);
		let line_rows = render_bubble_rows(&model, 80, ChatMode::Line, 0);
		assert_eq!(
			line_rows.len(),
			default_rows.len() + 2,
			"LINE mode should add exactly 2 rows (top + bottom border)"
		);
	}

	/// LINE モードの 1 行目は TopBorder、最後の行は Margin (枠外スペーサー)、
	/// その直前が BottomBorder、他は Body。
	#[test]
	fn line_mode_row_kinds_have_top_and_bottom_borders() {
		let msg = text_message("hello world", Sender::Main);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let rows = render_bubble_rows(&model, 80, ChatMode::Line, 0);
		assert_eq!(rows.first().unwrap().kind, BubbleRowKind::TopBorder);
		let n = rows.len();
		// 末尾は Margin (バブル間スペーサー)、その直前が BottomBorder
		assert_eq!(rows[n - 1].kind, BubbleRowKind::Margin);
		assert_eq!(rows[n - 2].kind, BubbleRowKind::BottomBorder);
		// 中間はすべて Body
		for row in rows.iter().take(n - 2).skip(1) {
			assert_eq!(row.kind, BubbleRowKind::Body);
		}
	}

	/// TopBorder / BottomBorder の Line 文字列は `╭──...──╮` / `╰──...──╯`。
	#[test]
	fn line_mode_border_lines_use_rounded_corners() {
		let msg = text_message("x", Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 20;
		let rows = render_bubble_rows(&model, content_width, ChatMode::Line, 0);
		let top_combined: String = rows[0]
			.line
			.spans
			.iter()
			.map(|s| s.content.as_ref())
			.collect::<Vec<&str>>()
			.join("");
		assert!(
			top_combined.starts_with('╭'),
			"top starts with ╭: {top_combined:?}"
		);
		assert!(
			top_combined.ends_with('╮'),
			"top ends with ╮: {top_combined:?}"
		);
		assert_eq!(top_combined.chars().count(), content_width as usize);

		let n = rows.len();
		let bottom_combined: String = rows[n - 2]
			.line
			.spans
			.iter()
			.map(|s| s.content.as_ref())
			.collect::<Vec<&str>>()
			.join("");
		assert!(bottom_combined.starts_with('╰'));
		assert!(bottom_combined.ends_with('╯'));
	}

	/// LINE モードの border 色は sender 別 (Main=黄 / Sub=緑)。
	#[test]
	fn line_mode_border_color_matches_sender() {
		let msg_main = text_message("a", Sender::Main);
		let model_main = ChatBubbleModel {
			message: &msg_main,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let rows_main = render_bubble_rows(&model_main, 30, ChatMode::Line, 0);
		let top_span_main = rows_main[0].line.spans.first().unwrap();
		assert_eq!(top_span_main.style.fg, Some(Color::Yellow));

		let msg_sub = text_message("a", Sender::Sub);
		let model_sub = ChatBubbleModel {
			message: &msg_sub,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let rows_sub = render_bubble_rows(&model_sub, 30, ChatMode::Line, 0);
		let top_span_sub = rows_sub[0].line.spans.first().unwrap();
		assert_eq!(top_span_sub.style.fg, Some(Color::Green));
	}

	/// LINE モードで Buffer に描画したとき、Body 行の左右に `│` が入る。
	#[test]
	fn line_mode_body_row_draws_vertical_borders_on_both_sides() {
		use ratatui::buffer::Buffer;

		let area_width: u16 = 80;
		let bubble_width: u16 = 30;
		let rect = Rect {
			x: 0,
			y: 0,
			width: area_width,
			height: 1,
		};
		let row = BubbleRow {
			kind: BubbleRowKind::Body,
			line: Line::from("body"),
		};
		let mut buf = Buffer::empty(rect);
		// Sender::Sub → Sub=左寄せ。bubble_width=30 の 0..30 に描かれる。
		draw_bubble_row(
			rect,
			&mut buf,
			&row,
			Sender::Sub,
			bubble_width,
			ChatMode::Line,
		);
		// x=0 と x=bubble_width-1=29 に `│` (Green)
		let left = buf.cell((0, 0)).unwrap();
		assert_eq!(left.symbol(), "│");
		assert_eq!(left.fg, Color::Green);
		let right = buf.cell((bubble_width - 1, 0)).unwrap();
		assert_eq!(right.symbol(), "│");
		assert_eq!(right.fg, Color::Green);
	}

	/// LINE モードで TopBorder 行を描画すると `╭` が左端、`╮` が右端に入る。
	#[test]
	fn line_mode_top_border_draws_rounded_corners_in_buffer() {
		use ratatui::buffer::Buffer;

		let content_width: u16 = 30;
		let rect = Rect {
			x: 0,
			y: 0,
			width: 80,
			height: 1,
		};
		let horizontal = "─".repeat((content_width - 2) as usize);
		let top_line = Line::from(Span::styled(
			format!("╭{horizontal}╮"),
			Style::default().fg(Color::Green),
		));
		let row = BubbleRow {
			kind: BubbleRowKind::TopBorder,
			line: top_line,
		};
		let mut buf = Buffer::empty(rect);
		draw_bubble_row(
			rect,
			&mut buf,
			&row,
			Sender::Sub,
			content_width,
			ChatMode::Line,
		);

		let left = buf.cell((0, 0)).unwrap();
		assert_eq!(left.symbol(), "╭");
		let right = buf.cell((content_width - 1, 0)).unwrap();
		assert_eq!(right.symbol(), "╮");
	}

	/// LINE モードでも Main は右寄せになる (寄せは default と同じ)。
	#[test]
	fn line_mode_preserves_sender_alignment() {
		use ratatui::buffer::Buffer;

		let area_width: u16 = 100;
		let bubble_width: u16 = 30;
		let rect = Rect {
			x: 0,
			y: 0,
			width: area_width,
			height: 1,
		};
		let row = BubbleRow {
			kind: BubbleRowKind::Body,
			line: Line::from("x"),
		};
		let mut buf = Buffer::empty(rect);
		draw_bubble_row(
			rect,
			&mut buf,
			&row,
			Sender::Main,
			bubble_width,
			ChatMode::Line,
		);

		// Main→右寄せ: bubble 左端 = area_width - bubble_width = 70
		let left_edge = buf.cell((70, 0)).unwrap();
		assert_eq!(left_edge.symbol(), "│");
		assert_eq!(left_edge.fg, Color::Yellow);
	}

	/// 回帰テスト: LINE モードの末尾マージン行は `│` を描かない。
	///
	/// 実機で「`╰──╯` の 1 行下のバブル間スペーサーに `│ │` だけ残って見える」
	/// バグ (case A 対策の `BubbleRowKind::Margin` 導入) の回帰防止。
	#[test]
	fn line_mode_margin_row_draws_nothing() {
		use ratatui::buffer::Buffer;

		let area_width: u16 = 80;
		let bubble_width: u16 = 30;
		let rect = Rect {
			x: 0,
			y: 0,
			width: area_width,
			height: 1,
		};
		let row = BubbleRow {
			kind: BubbleRowKind::Margin,
			line: Line::from(""),
		};
		let mut buf = Buffer::empty(rect);
		draw_bubble_row(
			rect,
			&mut buf,
			&row,
			Sender::Sub,
			bubble_width,
			ChatMode::Line,
		);
		// bubble 領域 (x=0..30) のどこにも `│` / `▏` が描かれていないこと
		for x in 0..bubble_width {
			let cell = buf.cell((x, 0)).unwrap();
			assert_ne!(
				cell.symbol(),
				"│",
				"LINE Margin row must not draw │ at x={x}"
			);
			assert_ne!(
				cell.symbol(),
				"▏",
				"LINE Margin row must not draw ▏ at x={x}"
			);
		}
	}

	/// 回帰テスト: Default / Slack モードの末尾マージン行も `▏` を描かない。
	///
	/// 「Margin 行は全モード共通で空」ルールの確認。
	#[test]
	fn default_and_slack_margin_row_draws_nothing() {
		use ratatui::buffer::Buffer;

		for mode in [ChatMode::Default, ChatMode::Slack] {
			let area_width: u16 = 80;
			let bubble_width: u16 = 30;
			let rect = Rect {
				x: 0,
				y: 0,
				width: area_width,
				height: 1,
			};
			let row = BubbleRow {
				kind: BubbleRowKind::Margin,
				line: Line::from(""),
			};
			let mut buf = Buffer::empty(rect);
			draw_bubble_row(rect, &mut buf, &row, Sender::Sub, bubble_width, mode);
			for x in 0..area_width {
				let cell = buf.cell((x, 0)).unwrap();
				assert_ne!(
					cell.symbol(),
					"▏",
					"mode {mode:?}: Margin row must not draw ▏ at x={x}"
				);
				assert_ne!(
					cell.symbol(),
					"│",
					"mode {mode:?}: Margin row must not draw │ at x={x}"
				);
			}
		}
	}

	/// 回帰テスト: LINE モードで実際に `render_bubble_rows` → `draw_bubble_row`
	/// を最後まで流すと、下枠 `╰──╯` の **次の行** (= Margin) に `│` が
	/// 残らない。実機で見えた症状そのものを再現する end-to-end 回帰テスト。
	#[test]
	fn line_mode_rendered_margin_row_is_clean_after_bottom_border() {
		use ratatui::buffer::Buffer;

		let msg = text_message("hello", Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 30;
		let bubble_width: u16 = content_width;
		let rows = render_bubble_rows(&model, content_width, ChatMode::Line, 0);

		let n = rows.len();
		assert!(n >= 3);
		assert_eq!(rows[n - 2].kind, BubbleRowKind::BottomBorder);
		assert_eq!(rows[n - 1].kind, BubbleRowKind::Margin);

		let rect = Rect {
			x: 0,
			y: 0,
			width: 80,
			height: 1,
		};
		let mut buf = Buffer::empty(rect);
		// 最後の Margin 行を描画
		draw_bubble_row(
			rect,
			&mut buf,
			&rows[n - 1],
			Sender::Sub,
			bubble_width,
			ChatMode::Line,
		);

		// bubble 矩形内に `│` が残っていない
		for x in 0..bubble_width {
			let cell = buf.cell((x, 0)).unwrap();
			assert_ne!(
				cell.symbol(),
				"│",
				"rendered Margin row must be visually empty at x={x}"
			);
		}
	}

	/// Default / Slack モードで render_bubble_rows を呼んだときは、末尾 1 行
	/// だけ `Margin` (バブル間スペーサー)、他はすべて `Body`。`TopBorder` /
	/// `BottomBorder` はどこにも出現しない。
	#[test]
	fn default_and_slack_modes_tag_last_row_as_margin() {
		let msg = text_message("hi", Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		for mode in [ChatMode::Default, ChatMode::Slack] {
			let rows = render_bubble_rows(&model, 80, mode, 0);
			let n = rows.len();
			assert!(n >= 2, "mode {:?} must have at least body + margin", mode);
			// 末尾 1 行のみ Margin
			assert_eq!(
				rows[n - 1].kind,
				BubbleRowKind::Margin,
				"mode {:?}: last row must be Margin",
				mode
			);
			// それ以外は Body (TopBorder / BottomBorder は default/slack では使わない)
			for (i, row) in rows.iter().enumerate().take(n - 1) {
				assert_eq!(
					row.kind,
					BubbleRowKind::Body,
					"mode {:?}: row[{}] must be Body",
					mode,
					i
				);
			}
		}
	}

	/// 不変条件: **全 chat_mode × 全メッセージ形状** で、`render_bubble_rows`
	/// の末尾要素は必ず `BubbleRowKind::Margin`。また `Margin` は末尾にしか
	/// 現れない (中間に `Margin` が紛れると行数不一致や `│` ズレの温床になる)。
	///
	/// 過去に LINE 分岐だけ末尾マージンを `Body` tag で push してしまい、
	/// `╰──╯` の下に `│ │` が残るバグが出た (`lessons.md` の「末尾マージン行は
	/// 枠線の責任外として tag する」節)。今後モードを足しても `split_trailing_margin`
	/// helper を経由する限り自動的に正しく tag される。このテストは
	/// **将来の新モード追加時のドリフトも必ず落とす** 網羅テスト。
	#[test]
	fn render_bubble_rows_always_ends_with_margin_across_modes_and_shapes() {
		let text_sub = text_message("hi", Sender::Sub);
		let text_main = text_message("hello", Sender::Main);
		let mut final_response = text_message("done", Sender::Sub);
		final_response.is_final_response = true;
		let long_text = text_message(&"a".repeat(300), Sender::Sub);
		let integrated = paired_message(
			"tu-invariant",
			serde_json::json!({"command": "echo"}),
			"ok",
			false,
		);
		let integrated_error = paired_message(
			"tu-err",
			serde_json::json!({"command": "false"}),
			"boom",
			true,
		);
		let integrated_expanded = paired_message(
			"tu-exp",
			serde_json::json!({"command": "seq 5"}),
			&"line\n".repeat(8),
			false,
		);
		let cases: &[(&FormattedMessage, &str, bool)] = &[
			(&text_sub, "text-sub", false),
			(&text_main, "text-main", false),
			(&final_response, "text-final", false),
			(&long_text, "long-text", false),
			(&integrated, "integrated-preview", false),
			(&integrated_error, "integrated-error", false),
			(&integrated_expanded, "integrated-expanded", true),
		];
		for (msg, label, expand) in cases {
			for mode in [ChatMode::Default, ChatMode::Slack, ChatMode::Line] {
				for content_width in [30u16, 60, 100] {
					let model = ChatBubbleModel {
						message: msg,
						agent_type: "general-purpose",
						is_detailed_mode: *expand,
						is_expanded: *expand,
						is_selected: false,
					};
					let rows = render_bubble_rows(&model, content_width, mode, 0);
					let n = rows.len();
					assert!(
						n >= 1,
						"[{label}] mode={mode:?} width={content_width}: rows must be non-empty",
					);
					// (1) 末尾は必ず Margin
					assert_eq!(
						rows[n - 1].kind,
						BubbleRowKind::Margin,
						"[{label}] mode={mode:?} width={content_width}: last row must be Margin (got {:?})",
						rows[n - 1].kind,
					);
					// (2) Margin は末尾以外に現れない
					for (i, row) in rows.iter().enumerate().take(n - 1) {
						assert_ne!(
							row.kind,
							BubbleRowKind::Margin,
							"[{label}] mode={mode:?} width={content_width}: row[{i}] must not be Margin",
						);
					}
					// (3) LINE モードは先頭 TopBorder / 末尾直前 BottomBorder を確保
					if mode == ChatMode::Line {
						assert_eq!(
							rows[0].kind,
							BubbleRowKind::TopBorder,
							"[{label}] LINE width={content_width}: first row must be TopBorder",
						);
						assert!(n >= 2);
						assert_eq!(
							rows[n - 2].kind,
							BubbleRowKind::BottomBorder,
							"[{label}] LINE width={content_width}: row before Margin must be BottomBorder",
						);
					}
				}
			}
		}
	}

	/// LINE モードで「estimate == render rows count」不変条件が成り立つ。
	/// 複数ケース (text / 統合バブル / mention 付き / 長文) をテーブル駆動で検証。
	#[test]
	fn estimate_matches_rendered_rows_for_line_mode_basic_text() {
		let msg = text_message("hello world", Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 40;
		let estimated = crate::ui::layout::estimate_bubble_height_with_mode(
			&msg,
			false,
			content_width,
			ChatMode::Line,
		);
		let actual = render_bubble_rows(&model, content_width, ChatMode::Line, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"LINE basic text estimate {estimated} must match rendered {actual}"
		);
	}

	#[test]
	fn estimate_matches_rendered_rows_for_line_mode_long_text() {
		// 本文を長くして折り返しが発生するケース。LINE は wrap_width が縮むので
		// estimate 側もそれを考慮した計算でないと必ずズレる。
		let body = "a".repeat(200);
		let msg = text_message(&body, Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 50;
		let estimated = crate::ui::layout::estimate_bubble_height_with_mode(
			&msg,
			false,
			content_width,
			ChatMode::Line,
		);
		let actual = render_bubble_rows(&model, content_width, ChatMode::Line, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"LINE long text estimate {estimated} must match rendered {actual}"
		);
	}

	#[test]
	fn estimate_matches_rendered_rows_for_line_mode_integrated_bubble_expanded() {
		let long_input = serde_json::json!({
			"command": (0..5)
				.map(|i| format!("echo line{i} {}", "x".repeat(80)))
				.collect::<Vec<_>>()
				.join("\n"),
		});
		let long_result = (0..8)
			.map(|i| format!("result-line{i} ") + &"y".repeat(100))
			.collect::<Vec<_>>()
			.join("\n");
		let msg = paired_message("tu-ln", long_input, &long_result, false);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: true,
			is_expanded: true,
			is_selected: false,
		};
		let content_width: u16 = 50;
		let estimated = crate::ui::layout::estimate_bubble_height_with_mode(
			&msg,
			true,
			content_width,
			ChatMode::Line,
		);
		let actual = render_bubble_rows(&model, content_width, ChatMode::Line, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"LINE integrated expanded estimate {estimated} must match rendered {actual}"
		);
	}

	/// mention 付き (Sub + final_response) でも LINE モードで estimate == render rows。
	#[test]
	fn estimate_matches_rendered_rows_for_line_mode_with_mention_prefix() {
		use crate::ui::layout::estimate_bubble_height_full;

		let body = "x".repeat(120);
		let mut msg = text_message(&body, Sender::Sub);
		msg.is_final_response = true;
		let agent_type = "Explore";
		let model = ChatBubbleModel {
			message: &msg,
			agent_type,
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 30;
		let estimated =
			estimate_bubble_height_full(&msg, false, content_width, agent_type, ChatMode::Line);
		let actual = render_bubble_rows(&model, content_width, ChatMode::Line, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"LINE with mention: estimate {estimated} must match rendered {actual}"
		);
	}

	/// Default / Slack モードで render_bubble_rows と estimate_bubble_height_with_mode
	/// が `render_bubble_lines` / `estimate_bubble_height` と一致する (後方互換確認)。
	#[test]
	fn default_and_slack_modes_match_legacy_estimate() {
		let msg = text_message("hello", Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let content_width: u16 = 80;
		let legacy = crate::ui::layout::estimate_bubble_height(&msg, false, content_width);
		for mode in [ChatMode::Default, ChatMode::Slack] {
			let est = crate::ui::layout::estimate_bubble_height_with_mode(
				&msg,
				false,
				content_width,
				mode,
			);
			let rendered = render_bubble_rows(&model, content_width, mode, 0).len() as u16;
			assert_eq!(est, legacy, "mode {:?} estimate must equal legacy", mode);
			assert_eq!(
				rendered, legacy,
				"mode {:?} rendered must equal legacy",
				mode
			);
		}
	}

	/// 極端に狭い content_width (= RESULT_INDENT 以下) でも panic せず、
	/// estimate と render が一致する。
	#[test]
	fn estimate_matches_rendered_for_integrated_bubble_with_tiny_width() {
		use crate::ui::layout::estimate_bubble_height;

		let msg = paired_message(
			"tu-narrow",
			serde_json::json!({"command": "echo"}),
			"result body",
			false,
		);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: true,
			is_expanded: true,
			is_selected: false,
		};
		// 10 は `estimate_bubble_height` の `max(10)` ガードの下限。
		// `wrap_width - RESULT_INDENT` が 1 桁になる境界付近。
		let content_width: u16 = 10;
		let estimated = estimate_bubble_height(&msg, true, content_width);
		let actual = render_bubble_lines(&model, content_width, 0).len() as u16;
		assert_eq!(
			estimated, actual,
			"tiny-width integrated bubble: estimate ({estimated}) must match actual rendered lines ({actual})"
		);
	}

	// ----------------------------------------------------------------------
	// ツール実行ステータスマーカー (スピナー / ✓ / ✗)
	// ----------------------------------------------------------------------

	/// tool_use のみで tool_result が未到着のメッセージは `InProgress`。
	#[test]
	fn tool_status_returns_in_progress_when_result_missing() {
		let mut msg = paired_message("tu-1", serde_json::json!({"command": "echo"}), "hi", false);
		msg.tool_result = None;
		msg.result_timestamp = None;
		assert_eq!(tool_status(&msg), Some(ToolStatus::InProgress));
	}

	/// tool_use + tool_result + !is_error は `Done`。
	#[test]
	fn tool_status_returns_done_when_result_succeeded() {
		let msg = paired_message("tu-2", serde_json::json!({"x":1}), "ok", false);
		assert_eq!(tool_status(&msg), Some(ToolStatus::Done));
	}

	/// tool_use + tool_result + is_error は `Errored`。
	#[test]
	fn tool_status_returns_errored_when_result_failed() {
		let msg = paired_message("tu-3", serde_json::json!({"x":1}), "boom", true);
		assert_eq!(tool_status(&msg), Some(ToolStatus::Errored));
	}

	/// text バブルや orphan tool_result は `None`。
	#[test]
	fn tool_status_returns_none_for_non_tool_use_bubbles() {
		// text バブル
		let text = text_message("hello", Sender::Sub);
		assert_eq!(tool_status(&text), None);
		// orphan tool_result
		let mut orphan = paired_message("tu-4", serde_json::json!({"x":1}), "z", false);
		orphan.tool_use = None;
		assert_eq!(tool_status(&orphan), None);
	}

	/// 実行中バブルのラベル行に Braille スピナー文字が乗る。`spinner_phase` で
	/// 選ばれるフレームが `SPINNER_FRAMES` の対応 index に一致する。
	#[test]
	fn in_progress_bubble_renders_spinner_at_label_tail() {
		let mut msg = paired_message(
			"tu-prog",
			serde_json::json!({"command": "sleep 1"}),
			"",
			false,
		);
		msg.tool_result = None;
		msg.result_timestamp = None;
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		// phase=0 のときスピナー文字は SPINNER_FRAMES[0]
		let lines = render_bubble_lines(&model, 60, 0);
		let label_line = &lines[1];
		let last_span = label_line.spans.last().expect("label has spinner span");
		assert_eq!(
			last_span.content.as_ref(),
			SPINNER_FRAMES[0].to_string(),
			"phase=0 must use SPINNER_FRAMES[0]"
		);
		assert_eq!(last_span.style.fg, Some(Color::Magenta));

		// phase=11 → 11 % 10 = 1 → SPINNER_FRAMES[1]
		let lines2 = render_bubble_lines(&model, 60, 11);
		let label_line2 = &lines2[1];
		let last2 = label_line2.spans.last().unwrap();
		assert_eq!(
			last2.content.as_ref(),
			SPINNER_FRAMES[1].to_string(),
			"phase=11 must wrap to SPINNER_FRAMES[1]"
		);
	}

	/// 完了バブルのラベル行に `✓` が乗る (Green + Bold)。
	#[test]
	fn done_bubble_renders_check_marker_at_label_tail() {
		let msg = paired_message("tu-ok", serde_json::json!({"x":1}), "ok", false);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 60, 0);
		let label_line = &lines[1];
		let last_span = label_line.spans.last().unwrap();
		assert_eq!(last_span.content.as_ref(), DONE_MARKER.to_string());
		assert_eq!(last_span.style.fg, Some(Color::Green));
		assert!(last_span.style.add_modifier.contains(Modifier::BOLD));
	}

	/// エラーバブルのラベル行に `✗` が乗る (Red + Bold)。
	#[test]
	fn errored_bubble_renders_cross_marker_at_label_tail() {
		let msg = paired_message("tu-err", serde_json::json!({"x":1}), "boom", true);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 60, 0);
		let label_line = &lines[1];
		let last_span = label_line.spans.last().unwrap();
		assert_eq!(last_span.content.as_ref(), ERROR_MARKER.to_string());
		assert_eq!(last_span.style.fg, Some(Color::Red));
		assert!(last_span.style.add_modifier.contains(Modifier::BOLD));
	}

	/// orphan tool_result バブルの ↪ ラベルにはステータスマーカーが付かない
	/// (tool_use がないので状態の概念がない)。
	#[test]
	fn orphan_tool_result_has_no_status_marker() {
		use crate::domain::entities::ToolResult;
		let msg = FormattedMessage {
			id: "m-orphan".to_string(),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: None,
			tool_use: None,
			tool_result: Some(ToolResult {
				content: "x".to_string(),
				is_error: false,
			}),
			tool_use_id: None,
			result_timestamp: None,
			is_final_response: false,
		};
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Bash",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 60, 0);
		// マーカー候補文字が body 全体に含まれないことを確認 (`↪` は label の頭文字)
		for line in &lines {
			let combined: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
			assert!(
				!combined.contains(DONE_MARKER) && !combined.contains(ERROR_MARKER),
				"orphan tool_result must not carry ✓/✗, got {combined:?}"
			);
		}
	}

	/// text バブルにはマーカーが付かない (Sub の独り言や Main の prompt も同様)。
	#[test]
	fn text_bubble_has_no_status_marker() {
		let msg = text_message("just text", Sender::Sub);
		let model = ChatBubbleModel {
			message: &msg,
			agent_type: "Explore",
			is_detailed_mode: false,
			is_expanded: false,
			is_selected: false,
		};
		let lines = render_bubble_lines(&model, 60, 5);
		for line in &lines {
			let combined: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
			assert!(
				!SPINNER_FRAMES.iter().any(|c| combined.contains(*c))
					&& !combined.contains(DONE_MARKER)
					&& !combined.contains(ERROR_MARKER),
				"text bubble must not carry status marker, got {combined:?}"
			);
		}
	}

	/// ステータスマーカー追加で **bubble の総行数は増えない** (= ラベル行末尾の
	/// inline Span として乗る)。`estimate_bubble_height` と一致することを担保する。
	#[test]
	fn status_marker_does_not_inflate_bubble_height() {
		use crate::ui::layout::estimate_bubble_height;
		// 3 ケース: in_progress / done / errored
		let mut in_prog = paired_message("a", serde_json::json!({"x":1}), "", false);
		in_prog.tool_result = None;
		in_prog.result_timestamp = None;
		let done = paired_message("b", serde_json::json!({"x":1}), "ok", false);
		let err = paired_message("c", serde_json::json!({"x":1}), "boom", true);

		for (msg, label) in [
			(&in_prog, "in_progress"),
			(&done, "done"),
			(&err, "errored"),
		] {
			let model = ChatBubbleModel {
				message: msg,
				agent_type: "Bash",
				is_detailed_mode: false,
				is_expanded: false,
				is_selected: false,
			};
			let estimated = estimate_bubble_height(msg, false, 60);
			let actual = render_bubble_lines(&model, 60, 0).len() as u16;
			assert_eq!(
				estimated, actual,
				"{label}: estimate ({estimated}) must match render ({actual}) — marker must not add a row",
			);
		}
	}
}
