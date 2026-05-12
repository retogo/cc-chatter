//! 行ベースの viewport 計算 + ChatBubble 高さ見積もり。
//!
//! 設計 (TS 版 `src/ui/utils/viewportLayout.ts` 相当):
//!
//! 1. 各メッセージの推定高さを `estimate_bubble_height` で算出
//! 2. `compute_row_based_viewport` が以下を算出:
//!    - `effective_offset`: clamp + 選択追従 + 末尾追従後の実オフセット
//!    - `follow_tail_reached`: `effective_offset == max_offset` なら true
//!    - `items`: 表示範囲内メッセージの `{message_index, start_row, end_row}`
//!    - `start_index` / `end_index`: 描画対象の message index 範囲
//! 3. `find_message_at_row(items, row)` で行 → message index 逆引き
//!    (マウスクリックのヒット判定に使う)
//!
//! **clamp は描画層の責務** (TS lessons.md の移行節参照)。reducer は
//! `scroll_offset_rows` を素朴に更新し、描画層が算出した値を
//! `set_scroll_offset()` で書き戻す。

use crate::domain::entities::{FormattedMessage, Sender, ToolResult, ToolUse};
use crate::settings::ChatMode;

/// 統合バブル (tool_use + tool_result が pair 済み) で、tool_result 部分の
/// インデント幅 (文字数)。
///
/// 現行仕様ではツール表示をミニマム化するため **0** にしている。Slack スレッド
/// 風の字下げは廃止。orphan tool_result も同じく indent=0。
///
/// 定数として残す理由は、`render_bubble_lines` / `estimate_bubble_height` が
/// 「見積もり == 実描画行数」原則のために同一の値を参照する必要があり、
/// 万一仕様が再び変わったときに 1 箇所で切り替えられるようにするため。
pub(crate) const RESULT_INDENT: usize = 0;

/// LINE chat_mode で追加される行数 (top / bottom border 1 行ずつ)。
pub(crate) const LINE_BORDER_ROWS: u16 = 2;

/// LINE chat_mode で bubble の内側 (本文エリア) が持つ幅を、bubble 全体幅から
/// 引き去る列数。`│ ... │` で左右 1 列ずつ食われる。
pub(crate) const LINE_BORDER_COLS: u16 = 2;

/// chat_mode に応じた本文エリアの実効折り返し幅を算出する。
///
/// - `Default` / `Slack`: `content_width` をそのまま使う
/// - `Line`: 左右ボーダー 2 列分を差し引き、最低 10 で clamp (既存の
///   `estimate_bubble_height` の `max(10)` ガードと整合)
///
/// `estimate_*` と `render_bubble_rows` で同じ幅を使うことで
/// 「見積もり == 実描画行数」を保つ。
pub(crate) fn effective_inner_width(content_width: u16, chat_mode: ChatMode) -> u16 {
	match chat_mode {
		ChatMode::Line => content_width.saturating_sub(LINE_BORDER_COLS).max(10),
		ChatMode::Default | ChatMode::Slack => content_width.max(10),
	}
}

/// mention prefix の表示幅を agent_type 文字列から計算する。
///
/// - Main: `@{agent_type} ` (末尾スペース 1 含む)
/// - Sub + final_response: `@Main `
/// - それ以外: None (mention なし)
///
/// UI 層 (chat_bubble) とこのレイアウト層で **同じ計算** をする必要がある
/// (lessons.md の「見積もり == 実描画行数」原則)。引数は描画側と同じく
/// `agent_type` の生文字列と sender / is_final_response のみに限定する。
pub(crate) fn mention_prefix_width(
	sender: Sender,
	agent_type: &str,
	is_final_response: bool,
) -> Option<usize> {
	match sender {
		Sender::Main => Some(format!("@{agent_type} ").chars().count()),
		Sender::Sub if is_final_response => Some("@Main ".chars().count()),
		_ => None,
	}
}

/// ChatBubble 1 件の推定高さ。
///
/// `render_bubble_lines` が出す Line 数と揃えること:
/// - ヘッダー 1 行
/// - 本文 (text / tool_use / tool_result / 統合バブル いずれか)
/// - 末尾マージン 1 行 (空行)
///
/// ## 統合バブル (tool_use + tool_result が pair 済み)
///
/// `tool_use=Some && tool_result=Some` のケースは「呼び出し + 結果」を 1 バブル
/// にまとめる。**closed (preview) と expanded で構造が異なる**:
///
/// - **closed (preview)**: ツール名 + input preview のみ。区切り行 / result
///   ラベル / result preview は出さない (= 単独 tool_use と同じ shape)。
///   ノイズ削減のためツール表示をミニマム化するという仕様 (spec.md
///   「ツール呼び出し + 結果」の節を参照)。
/// - **expanded**: ツール名 + input 全文 (折り返し) + 区切り (1) +
///   result ラベル + result 全文 (折り返し)。インデントなし (`RESULT_INDENT=0`)。
///
/// 「最新の Bash」は `app::handle_messages_appended` が append 時に
/// `expanded_ids` に挿入することで自動 expanded 状態になる。ユーザーが
/// Enter/Space/クリックで toggle すれば閉じられる。
///
/// ## mention prefix (`@Main` / `@{agent_type}`)
///
/// text bubble では 1 行目の先頭に mention Span が差し込まれるため、最初の
/// 物理行だけ wrap 幅が `content_width - mention_width` に縮む。行数計算も
/// 描画も `wrap_line_count_with_prefix` / `wrap_lines_iter_with_prefix` で
/// 揃える。mention は tool_use / tool_result バブルでは付かない (見た目の
/// 対象外)。`agent_type` / `is_final_response` を渡すオーバーロードは
/// `estimate_bubble_height_with_prefix`。通常版はデフォルトで mention なし
/// (既存テストが agent_type を持たないための互換)。
pub fn estimate_bubble_height(
	msg: &FormattedMessage,
	is_expanded: bool,
	content_width: u16,
) -> u16 {
	estimate_bubble_height_with_prefix(msg, is_expanded, content_width, "unknown")
}

/// `estimate_bubble_height` に `chat_mode` を足した変種。
///
/// LINE モードでは top/bottom border 2 行ぶん高さが増え、本文の折り返し幅は
/// `content_width - 2` に縮む。Default / Slack は従来挙動。
pub fn estimate_bubble_height_with_mode(
	msg: &FormattedMessage,
	is_expanded: bool,
	content_width: u16,
	chat_mode: ChatMode,
) -> u16 {
	estimate_bubble_height_full(msg, is_expanded, content_width, "unknown", chat_mode)
}

/// `estimate_bubble_height` の mention prefix 対応版。
///
/// `agent_type` は `Sender::Main` 経由の mention `@{agent_type} ` の
/// 表示幅計算に使う。`Sender::Sub + is_final_response=true` のときは
/// `@Main ` 固定なので agent_type は使われない。mention 対象でない bubble
/// (tool_use / tool_result / 中間 text) では agent_type を読まず影響なし。
///
/// chat_mode は Default で呼ばれる (互換版)。LINE / Slack を使うときは
/// `estimate_bubble_height_full` を直接呼ぶ。
pub fn estimate_bubble_height_with_prefix(
	msg: &FormattedMessage,
	is_expanded: bool,
	content_width: u16,
	agent_type: &str,
) -> u16 {
	estimate_bubble_height_full(
		msg,
		is_expanded,
		content_width,
		agent_type,
		ChatMode::Default,
	)
}

/// estimate の最終形: mention prefix + chat_mode 両対応。
///
/// - `Default` / `Slack`: 従来の height 計算 (header + body + margin)
/// - `Line`: 上記に加えて top/bottom border 2 行を加算し、本文の折り返し幅を
///   `content_width - 2` に縮める (左右ボーダーが 1 列ずつ食うため)
///
/// `h.max(3)` クランプは Default / Slack 互換のため残す。LINE モードでは
/// border ぶんすでに 3 以上になるので影響しない。
pub fn estimate_bubble_height_full(
	msg: &FormattedMessage,
	is_expanded: bool,
	content_width: u16,
	agent_type: &str,
	chat_mode: ChatMode,
) -> u16 {
	let inner_width = effective_inner_width(content_width, chat_mode) as usize;
	// ヘッダー + 末尾マージン = 2
	let mut h: u16 = 2;

	if let Some(text) = msg.text.as_deref() {
		let prefix_width =
			mention_prefix_width(msg.sender, agent_type, msg.is_final_response).unwrap_or(0);
		h = h.saturating_add(wrap_line_count_with_prefix(text, inner_width, prefix_width));
	} else if let (Some(tu), Some(tr)) = (msg.tool_use.as_ref(), msg.tool_result.as_ref()) {
		// 統合バブル (tool_use + tool_result が pair 済み)
		h = h.saturating_add(tool_use_height(tu, is_expanded, inner_width));
		if is_expanded {
			// 区切り行 (空行) 1 行
			h = h.saturating_add(1);
			// tool_result は indent=0 (字下げなし)。`RESULT_INDENT` を引いた幅は
			// `inner_width` と等価だが、定数経由で 1 箇所に集約する。
			let result_width = inner_width.saturating_sub(RESULT_INDENT).max(1);
			h = h.saturating_add(tool_result_height(tr, is_expanded, result_width));
		}
		// closed (preview) のときは tool_use のみで終わる。区切り / result セクションは
		// emit しない (ノイズ削減のためツール表示をミニマム化する仕様)。
	} else if let Some(tu) = msg.tool_use.as_ref() {
		h = h.saturating_add(tool_use_height(tu, is_expanded, inner_width));
	} else if let Some(tr) = msg.tool_result.as_ref() {
		// orphan tool_result はインデントなし (従来どおり)
		h = h.saturating_add(tool_result_height(tr, is_expanded, inner_width));
	}

	let base = h.max(3);
	match chat_mode {
		ChatMode::Line => base.saturating_add(LINE_BORDER_ROWS),
		ChatMode::Default | ChatMode::Slack => base,
	}
}

fn tool_use_height(tu: &ToolUse, is_expanded: bool, content_width: usize) -> u16 {
	// ラベル行 + preview/pretty
	let mut h: u16 = 1;
	if is_expanded {
		let serialized = serde_json::to_string_pretty(&tu.input).unwrap_or_default();
		h = h.saturating_add(wrap_line_count(&serialized, content_width));
	} else {
		h = h.saturating_add(1);
	}
	h
}

fn tool_result_height(tr: &ToolResult, is_expanded: bool, content_width: usize) -> u16 {
	let mut h: u16 = 1;
	if is_expanded {
		h = h.saturating_add(wrap_line_count(&tr.content, content_width));
	} else {
		h = h.saturating_add(1);
	}
	h
}

/// 指定幅で折り返したときの行数を、**アロケーションなし**で数える。
///
/// `estimate_bubble_height` は全メッセージに対して毎フレーム呼ばれるので、
/// この関数はホットパス。`wrap_string_lines` と違って `Vec<char>` /
/// `Vec<String>` を作らず、char iterator を回すだけに留める。
///
/// `wrap_string_lines(text, width).len()` と**結果が一致する**ことを担保
/// (tests 参照)。ずれると follow_tail 時に viewport 末尾に空白が残る。
fn wrap_line_count(text: &str, width: usize) -> u16 {
	if width == 0 {
		// `wrap_string_lines` は width=0 のとき split 無しで text 全体を 1 行扱い
		return 1;
	}
	let mut total: u32 = 0;
	for line in text.split('\n') {
		let count = line.chars().count();
		if count == 0 {
			total = total.saturating_add(1);
		} else {
			total = total.saturating_add(count.div_ceil(width) as u32);
		}
	}
	// `wrap_string_lines` の「空入力でも 1 行」ガードと合わせる
	total.max(1).min(u16::MAX as u32) as u16
}

/// mention prefix 付きテキストの折り返し行数を数える。
///
/// 「text の 1 行目の冒頭に mention ぶんの prefix (表示幅 `prefix_width`) が
/// 差し込まれる」前提の物理行数を返す。実際のレイアウト:
/// - 最初の論理行の **最初のチャンク幅** は `width - prefix_width` (mention が
///   同じ行に乗るため残り幅が縮む)
/// - それ以降のチャンク / 後続の論理行 は通常の `width`
///
/// `prefix_width == 0` なら `wrap_line_count` と同値。`wrap_lines_iter_with_prefix`
/// の要素数と常に一致する (`wrap_line_count` との一致と同じ不変条件)。
pub fn wrap_line_count_with_prefix(text: &str, width: usize, prefix_width: usize) -> u16 {
	if prefix_width == 0 {
		return wrap_line_count(text, width);
	}
	if width == 0 {
		return 1;
	}
	// mention 幅が content_width 以上という極端なケース (狭ターミナル) では、
	// 1 行目の本文幅が 0 以下になって発散する。render 側も `max(1)` でクランプ
	// するため、ここも揃える。
	let first_width = width.saturating_sub(prefix_width).max(1);

	let mut total: u32 = 0;
	let mut first_logical = true;
	for line in text.split('\n') {
		let count = line.chars().count();
		if count == 0 {
			// 空論理行は 1 行 (mention 行と同じ物理行にはなれない。wrapper 側も
			// 空論理行は独立した空行として emit する)
			total = total.saturating_add(1);
			first_logical = false;
			continue;
		}
		if first_logical {
			first_logical = false;
			if count <= first_width {
				// 1 チャンクで収まる: 1 物理行
				total = total.saturating_add(1);
			} else {
				// 最初のチャンク (1 物理行) + 残りを width で折り返し
				let remaining = count - first_width;
				total = total.saturating_add(1 + remaining.div_ceil(width) as u32);
			}
		} else {
			total = total.saturating_add(count.div_ceil(width) as u32);
		}
	}
	total.max(1).min(u16::MAX as u32) as u16
}

/// `text` を `\n` で分割し、各行を `width` 文字ごとのチャンクに分けた文字列列を
/// 返す。**描画層専用**。毎フレームの見積もり (= `wrap_line_count`) からは呼ばない
/// こと (ホットパスで Vec<String> を作ると重い)。
///
/// `render_bubble_lines` が body を行単位に割った Line 列を作るために使う。
/// viewport slice に入るメッセージだけに適用されるので (10-20 件程度)、
/// 内部で `Vec<char>` を作っても現実的な描画コストには乗らない。
///
/// 結果の要素数は `wrap_line_count(text, width)` と常に一致する (テストで検証)。
pub fn wrap_string_lines(text: &str, width: usize) -> Vec<String> {
	wrap_lines_iter(text, width)
		.map(|s| s.to_string())
		.collect()
}

/// `wrap_string_lines` の iterator 版。借用した `&str` のサブスライスを yield する
/// ので String アロケーションが発生しない (`wrap_string_lines` は内部でこれを
/// `.to_string()` しているだけ)。
///
/// 返り値の要素数 (= iterator の長さ) は `wrap_line_count(text, width)` と一致する
/// (tests 参照)。`width == 0` のときは入力全体を 1 要素だけ返す。
///
/// 描画ホットパスで「画面に映る行だけ Line を作る」ために使う。長い
/// tool_result (数百行) のうち viewport 先頭で skip される行や、末尾で溢れて
/// 描画されない行を `.skip().take()` でスキップすれば char 走査は行われるが
/// `String::from_iter` などの alloc は発生しない。
pub fn wrap_lines_iter(text: &str, width: usize) -> WrapLines<'_> {
	WrapLines {
		original: text,
		logical: text.split('\n'),
		width,
		pending_current: None,
		emitted_any: false,
		done: false,
	}
}

pub struct WrapLines<'a> {
	original: &'a str,
	logical: std::str::Split<'a, char>,
	width: usize,
	/// 現在処理中の論理行 (改行を含まないスライス) の残り。長い論理行を
	/// `width` 文字ごとのチャンクに割るとき、次の next() で使い回す。
	pending_current: Option<&'a str>,
	emitted_any: bool,
	done: bool,
}

/// `wrap_lines_iter` の prefix 付き版。最初の物理行 (= 最初の論理行の最初の
/// チャンク) を `width - prefix_width` で取る以外は `wrap_lines_iter` と同じ。
///
/// 要素数は `wrap_line_count_with_prefix(text, width, prefix_width)` と一致する
/// (lessons.md の「見積もり == 実描画行数」原則を mention 付きでも担保するため)。
///
/// 描画層は 1 要素目に mention span を prepend して 1 Line を作り、2 要素目
/// 以降はそのまま 1 Line = 1 物理行として push する。
pub fn wrap_lines_iter_with_prefix(
	text: &str,
	width: usize,
	prefix_width: usize,
) -> WrapLinesWithPrefix<'_> {
	let first_width = if prefix_width == 0 {
		width
	} else {
		width.saturating_sub(prefix_width).max(1)
	};
	WrapLinesWithPrefix {
		inner: WrapLines {
			original: text,
			logical: text.split('\n'),
			width,
			pending_current: None,
			emitted_any: false,
			done: false,
		},
		first_width,
		first_consumed: false,
		no_prefix: prefix_width == 0,
	}
}

pub struct WrapLinesWithPrefix<'a> {
	inner: WrapLines<'a>,
	first_width: usize,
	first_consumed: bool,
	no_prefix: bool,
}

impl<'a> Iterator for WrapLinesWithPrefix<'a> {
	type Item = &'a str;

	fn next(&mut self) -> Option<Self::Item> {
		// prefix_width=0 のときは `wrap_lines_iter` と同じ挙動で十分。
		if self.no_prefix {
			return self.inner.next();
		}
		if self.first_consumed {
			return self.inner.next();
		}
		// 最初の 1 行だけ first_width で切り出す。`width` を 1 回差し替えて
		// next() を呼ぶ → 元に戻す。WrapLines の内部状態はまだ空なので副作用はない。
		let saved = self.inner.width;
		// width=0 ガードがあるので first_width>=1 を担保している
		self.inner.width = self.first_width;
		let first = self.inner.next();
		self.inner.width = saved;
		self.first_consumed = true;
		first
	}
}

impl<'a> Iterator for WrapLines<'a> {
	type Item = &'a str;

	fn next(&mut self) -> Option<Self::Item> {
		if self.done {
			return None;
		}
		// width==0 は入力全体を 1 要素だけ返す (旧実装互換)
		if self.width == 0 {
			self.done = true;
			self.emitted_any = true;
			return Some(self.original);
		}

		loop {
			if let Some(current) = self.pending_current.take() {
				// 空論理行は "" 1 要素として emit
				if current.is_empty() {
					self.emitted_any = true;
					return Some("");
				}
				// 先頭 width 文字分のバイト境界を探す
				let mut end_byte = current.len();
				for (taken, (byte_idx, _)) in current.char_indices().enumerate() {
					if taken == self.width {
						end_byte = byte_idx;
						break;
					}
				}
				let (head, tail) = current.split_at(end_byte);
				if !tail.is_empty() {
					self.pending_current = Some(tail);
				}
				self.emitted_any = true;
				return Some(head);
			}

			match self.logical.next() {
				Some(next_line) => {
					self.pending_current = Some(next_line);
					continue;
				}
				None => {
					self.done = true;
					if !self.emitted_any {
						self.emitted_any = true;
						return Some("");
					}
					return None;
				}
			}
		}
	}
}

/// viewport slice の 1 要素 (ヒット判定用)。
///
/// message_id は持たない (毎フレーム全件 clone すると O(N) で重くなる)。
/// 呼び出し側が `messages[hit.message_index].id` で on-demand 引く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitItem {
	pub message_index: usize,
	pub start_row: u16,
	pub end_row: u16,
}

/// `compute_row_based_viewport` の戻り値。
#[derive(Debug, Clone)]
pub struct ViewportSlice {
	pub start_index: usize,
	pub end_index: usize,
	pub items: Vec<HitItem>,
	pub total_rows: u32,
	pub effective_offset: u32,
	pub follow_tail_reached: bool,
	/// メッセージ領域の行数 (`view_height` を u32 に正規化したもの)。
	pub view_rows: u32,
	/// 先頭 bubble に対するスキップ行数 (部分行クリップ用)。
	///
	/// `effective_offset` が最初に描画する bubble の開始行より後ろなら、
	/// その差分行だけ先頭 bubble を上から削ぎ落とす。
	pub head_skip_rows: u16,
}

/// メッセージ列から実表示範囲を決める。
///
/// - `heights`: 各メッセージの推定高さ
/// - `view_height`: メッセージ領域に使える行数
/// - `scroll_offset_rows`: 表示先頭からの行オフセット (clamp 前)
/// - `selected_index`: 選択中メッセージのインデックス
/// - `auto_follow_selection`: true のとき「選択が画面外なら寄せる」
/// - `follow_tail`: true のとき末尾吸着 (effective_offset = max_offset)
pub fn compute_row_based_viewport(
	heights: &[u16],
	view_height: u16,
	scroll_offset_rows: u32,
	selected_index: Option<usize>,
	auto_follow_selection: bool,
	follow_tail: bool,
) -> ViewportSlice {
	let total_rows: u32 = heights.iter().copied().map(u32::from).sum();
	let view = u32::from(view_height.max(1));
	let max_offset = total_rows.saturating_sub(view);

	let mut effective_offset = scroll_offset_rows.min(max_offset);

	// 選択追従: 選択が画面外ならその bubble が画面内に収まるように寄せる
	if auto_follow_selection {
		if let Some(sel) = selected_index {
			if sel < heights.len() {
				let (sel_start, sel_end) = cumulative_range(heights, sel);
				if sel_start < effective_offset {
					effective_offset = sel_start;
				} else if sel_end > effective_offset + view {
					effective_offset = sel_end.saturating_sub(view);
				}
			}
		}
	}

	if follow_tail {
		effective_offset = max_offset;
	}

	let follow_tail_reached = effective_offset >= max_offset;
	let screen_bottom = effective_offset + view;

	// 描画範囲の決定
	let mut start_index = heights.len();
	let mut end_index = heights.len();
	let mut cursor: u32 = 0;
	let mut items: Vec<HitItem> = Vec::new();
	let mut head_skip_rows: u16 = 0;

	for (i, &h) in heights.iter().enumerate() {
		let hu = u32::from(h);
		let start = cursor;
		let end = cursor + hu;
		cursor = end;

		// 画面外 (上)
		if end <= effective_offset {
			continue;
		}
		// 画面外 (下)
		if start >= screen_bottom {
			if start_index != heights.len() && end_index == heights.len() {
				end_index = i;
			}
			break;
		}

		// 最初に描画する bubble で、先頭が画面上端より上 → 部分行クリップ
		if start_index == heights.len() {
			start_index = i;
			if start < effective_offset {
				let skip = (effective_offset - start).min(u32::from(u16::MAX));
				head_skip_rows = skip as u16;
			}
		}
		end_index = i + 1;

		let rel_start = start.saturating_sub(effective_offset);
		let rel_end = end.saturating_sub(effective_offset);
		items.push(HitItem {
			message_index: i,
			start_row: rel_start.min(u16::MAX as u32) as u16,
			end_row: rel_end.min(u16::MAX as u32) as u16,
		});
	}

	if start_index > end_index {
		start_index = end_index;
	}

	ViewportSlice {
		start_index,
		end_index,
		items,
		total_rows,
		effective_offset,
		follow_tail_reached,
		view_rows: view,
		head_skip_rows,
	}
}

fn cumulative_range(heights: &[u16], index: usize) -> (u32, u32) {
	let mut start: u32 = 0;
	for &h in heights.iter().take(index) {
		start = start.saturating_add(u32::from(h));
	}
	let end = start.saturating_add(u32::from(heights[index]));
	(start, end)
}

/// 画面先頭 = 0 基準の相対行 `row` から、メッセージ id を引く。
///
/// - マウス Y 座標 (1-based) からの変換は呼び出し側で行う
///   (通常は `y - header_lines - 1`)
/// - どのメッセージにも該当しない (マージン行など) なら `None`
pub fn find_message_at_row(items: &[HitItem], row: u16) -> Option<&HitItem> {
	items
		.iter()
		.find(|item| row >= item.start_row && row < item.end_row)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// mention prefix 相当の幅だけ 1 行目を縮めたときの行数計算が、
	/// `wrap_lines_iter_with_prefix` の iterator 長と一致する (lessons.md の
	/// 「見積もり == 実描画行数」原則を mention 付きでも担保するため)。
	#[test]
	fn wrap_line_count_with_prefix_matches_iter_len() {
		let cases: &[(&str, usize, usize)] = &[
			("hello", 80, 5),          // 1 行に収まる
			("hello world", 8, 0),     // prefix なしと同値
			(&"a".repeat(100), 20, 6), // 1 行目を縮めて残りを 20 幅で折り返す
			(&"a".repeat(100), 20, 0), // 上の prefix なし比較
			("ab\ncd\nef", 10, 5),     // 複数論理行
			("", 10, 5),               // 空入力は 1 行 (空行)
			(&"a".repeat(20), 30, 10), // 1 行目に全部収まる (残りなし)
			(&"a".repeat(20), 30, 25), // first_width が負にクランプ (max(1))
		];
		for (text, width, prefix_width) in cases {
			let count = wrap_line_count_with_prefix(text, *width, *prefix_width);
			let iter_len = wrap_lines_iter_with_prefix(text, *width, *prefix_width).count();
			assert_eq!(
				count as usize, iter_len,
				"wrap_line_count_with_prefix vs iter mismatch for text={text:?} width={width} prefix_width={prefix_width}"
			);
		}
	}

	/// prefix_width=0 は `wrap_line_count` と同値 (既存互換)。
	#[test]
	fn wrap_line_count_with_prefix_zero_equals_plain() {
		let cases: &[(&str, usize)] = &[
			("hello", 80),
			(&"x".repeat(200), 60),
			("a\nb\nc", 5),
			("", 10),
		];
		for (text, width) in cases {
			assert_eq!(
				wrap_line_count_with_prefix(text, *width, 0),
				wrap_line_count(text, *width),
				"prefix=0 must equal plain wrap_line_count for text={text:?} width={width}",
			);
		}
	}

	#[test]
	fn mention_prefix_width_rules() {
		assert_eq!(
			mention_prefix_width(Sender::Main, "Explore", false),
			Some("@Explore ".chars().count())
		);
		assert_eq!(
			mention_prefix_width(Sender::Main, "general-purpose", true),
			Some("@general-purpose ".chars().count())
		);
		assert_eq!(
			mention_prefix_width(Sender::Sub, "Explore", true),
			Some("@Main ".chars().count())
		);
		// Sub + 中間 text (mention なし)
		assert_eq!(mention_prefix_width(Sender::Sub, "Explore", false), None);
	}

	#[test]
	fn wrap_line_count_matches_wrap_string_lines_len() {
		let cases: &[(&str, usize)] = &[
			("", 10),
			("hello", 80),
			("hello", 3),
			("a\nb\nc", 80),
			("a\n\nb", 80),
			("\n", 80),
			(&"x".repeat(200), 60),
			(&"x".repeat(200), 1),
			("こんにちは\nworld", 3),
		];
		for (text, width) in cases {
			let count = wrap_line_count(text, *width);
			let lines_len = wrap_string_lines(text, *width).len() as u16;
			assert_eq!(
				count, lines_len,
				"wrap_line_count vs wrap_string_lines mismatch for text={text:?} width={width}"
			);
		}
	}

	#[test]
	fn wrap_lines_iter_matches_wrap_string_lines() {
		let cases: &[(&str, usize)] = &[
			("", 10),
			("hello", 80),
			("hello", 3),
			("a\nb\nc", 80),
			("a\n\nb", 80),
			("\n", 80),
			(&"x".repeat(50), 8),
			("こんにちは\nworld", 3),
		];
		for (text, width) in cases {
			let iter_out: Vec<String> = wrap_lines_iter(text, *width)
				.map(|s| s.to_string())
				.collect();
			let vec_out = wrap_string_lines(text, *width);
			assert_eq!(
				iter_out, vec_out,
				"wrap_lines_iter vs wrap_string_lines mismatch for text={text:?} width={width}"
			);
		}
	}

	/// `"\n"` の 1 文字入力は「空行 2 つ」として扱う (`"".split('\n') == ["", ""]`
	/// と同じ)。iter 実装の `logical: str::Split<char>` は末尾改行後の空要素も
	/// emit するので、`wrap_string_lines` が 2 要素を返し `wrap_line_count` の 2 と
	/// 一致する。過去に `split_once_lf` ベースで実装した時点では 1 要素しか出て
	/// いなかったので、明示的に regression テストを置く。
	#[test]
	fn wrap_lines_iter_handles_lone_newline() {
		let out: Vec<String> = wrap_lines_iter("\n", 80).map(|s| s.to_string()).collect();
		assert_eq!(out, vec![String::new(), String::new()]);
		assert_eq!(wrap_line_count("\n", 80), 2);
	}

	#[test]
	fn wrap_lines_iter_supports_skip_take() {
		// 100 文字 / width=10 → 10 行。skip(3).take(2) で 4 行目から 2 行 ("xxxxxxxxxx" x2)。
		let text = "x".repeat(100);
		let taken: Vec<String> = wrap_lines_iter(&text, 10)
			.skip(3)
			.take(2)
			.map(|s| s.to_string())
			.collect();
		assert_eq!(taken.len(), 2);
		assert!(taken.iter().all(|s| s.chars().count() == 10));
	}

	#[test]
	fn empty_heights_produces_empty_slice() {
		let slice = compute_row_based_viewport(&[], 20, 0, None, true, false);
		assert_eq!(slice.start_index, 0);
		assert_eq!(slice.end_index, 0);
		assert!(slice.items.is_empty());
	}

	#[test]
	fn slice_fits_everything_when_view_large() {
		let heights = vec![3, 4, 5];
		let slice = compute_row_based_viewport(&heights, 100, 0, None, true, false);
		assert_eq!(slice.start_index, 0);
		assert_eq!(slice.end_index, 3);
		assert_eq!(slice.items.len(), 3);
		assert_eq!(slice.head_skip_rows, 0);
		assert!(slice.follow_tail_reached);
	}

	#[test]
	fn follow_tail_snaps_to_bottom() {
		let heights = vec![5, 5, 5, 5];
		let slice = compute_row_based_viewport(&heights, 5, 0, None, false, true);
		// total=20, view=5, max_offset=15
		assert_eq!(slice.effective_offset, 15);
		// 最後の bubble だけが画面に入る
		assert_eq!(slice.start_index, 3);
		assert_eq!(slice.end_index, 4);
		assert!(slice.follow_tail_reached);
	}

	#[test]
	fn auto_follow_selection_scrolls_down_when_selection_below_view() {
		let heights = vec![5, 5, 5, 5];
		let slice = compute_row_based_viewport(&heights, 5, 0, Some(3), true, false);
		assert_eq!(slice.effective_offset, 15);
		assert_eq!(slice.start_index, 3);
	}

	#[test]
	fn auto_follow_selection_scrolls_up_when_selection_above_view() {
		let heights = vec![5, 5, 5, 5];
		let slice = compute_row_based_viewport(&heights, 5, 15, Some(0), true, false);
		assert_eq!(slice.effective_offset, 0);
		assert_eq!(slice.start_index, 0);
	}

	#[test]
	fn hit_items_are_relative_to_viewport_top() {
		let heights = vec![3, 4, 5];
		let slice = compute_row_based_viewport(&heights, 100, 0, None, true, false);
		assert_eq!(slice.items[0].start_row, 0);
		assert_eq!(slice.items[0].end_row, 3);
		assert_eq!(slice.items[1].start_row, 3);
		assert_eq!(slice.items[1].end_row, 7);
	}

	#[test]
	fn head_skip_rows_reports_partial_top_bubble() {
		let heights = vec![5, 5, 5];
		// view=5, offset=2 → 最初の bubble (rows 0..5) のうち 0,1 を削って 2..5 を出す
		let slice = compute_row_based_viewport(&heights, 5, 2, None, false, false);
		assert_eq!(slice.effective_offset, 2);
		assert_eq!(slice.head_skip_rows, 2);
		assert_eq!(slice.start_index, 0);
		// rel_start は saturating_sub なので 0..3
		assert_eq!(slice.items[0].start_row, 0);
		assert_eq!(slice.items[0].end_row, 3);
		assert!(!slice.follow_tail_reached);
	}

	#[test]
	fn find_message_at_row_hits_inside_range() {
		let items = vec![
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
		];
		assert_eq!(find_message_at_row(&items, 0).unwrap().message_index, 0);
		assert_eq!(find_message_at_row(&items, 2).unwrap().message_index, 0);
		assert_eq!(find_message_at_row(&items, 3).unwrap().message_index, 1);
		assert_eq!(find_message_at_row(&items, 6).unwrap().message_index, 1);
		assert!(find_message_at_row(&items, 7).is_none());
		assert!(find_message_at_row(&items, 100).is_none());
	}

	#[test]
	fn follow_tail_reached_false_when_scrolled_above_bottom() {
		let heights = vec![10, 10, 10];
		let slice = compute_row_based_viewport(&heights, 10, 5, None, false, false);
		// total=30, view=10, max_offset=20. offset=5 < max → follow_tail_reached=false
		assert_eq!(slice.effective_offset, 5);
		assert!(!slice.follow_tail_reached);
	}

	#[test]
	fn follow_tail_reached_true_when_at_bottom_without_flag() {
		let heights = vec![10, 10, 10];
		let slice = compute_row_based_viewport(&heights, 10, 20, None, false, false);
		assert_eq!(slice.effective_offset, 20);
		assert!(slice.follow_tail_reached);
	}
}
