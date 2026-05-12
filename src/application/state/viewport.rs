//! Viewport state (WATCHING 画面の選択・展開・スクロール)。
//!
//! TS 版 `src/application/state/viewportState.ts` 相当。
//!
//! ## 責務分離 (lessons.md の「メッセージ単位スクロールのカクつきと
//! 行ベース viewport への移行」節を参照)
//!
//! - reducer (ここ) は **clamp 前の素朴な値** だけを保持する。`heights` や
//!   `view_height` に依存するロジック (実効オフセット・選択追従・末尾到達
//!   判定) は描画層 (`ui::layout::viewport_layout::compute_row_based_viewport`)
//!   に切り出す
//! - 描画層が算出した `effective_offset` + `follow_tail_reached` を
//!   `set_scroll_offset()` で書き戻す (useEffect 相当)。同値ガードで
//!   再描画ループを防ぐ
//! - 選択 (`selected_index`) とスクロール (`scroll_offset_rows`) は独立。
//!   キー操作は選択を動かすだけ、マウスホイール (`scroll_by_rows`) は
//!   スクロールのみ動かす
//!
//! ## auto_follow_selection の発火条件 (Issue 007 対策)
//!
//! `auto_follow_selection` を**キー操作直後のフレームだけ**有効にするため、
//! `pending_follow_selection` フラグを立てる。描画層が消費したら
//! `consume_pending_follow_selection()` で降ろす。
//!
//! これをしないと、ユーザーがホイールで上スクロールしても「選択が画面外 →
//! 自動追従で offset 引き戻し」のループが起きて、ホイール操作が毎フレーム
//! 打ち消される (= M3 初版で発生したちらつき)。

use std::collections::HashSet;

/// マウスホイール 1 tick あたりの移動行数。
///
/// ターミナルセルがアトミックなので物理ピクセル単位スクロールは不可能。
/// 最小粒度 = 1 行。ゆっくり回せば細かく、勢いよく回せば端末が複数 tick を
/// 連続で送ってくれるので自然に速く動く。Bubble Tea のデフォルトは 3 だが、
/// cc-chatter の WATCHING 画面はログを細かく追いたい用途なので 1 にする。
/// Shift+ホイールで大きく動かす等の拡張は M4 で検討。
pub const WHEEL_DELTA_ROWS: i32 = 1;

/// Viewport の状態。`selected_index = -1` 相当は `None`。
#[derive(Debug, Clone)]
pub struct ViewportState {
	pub selected_index: Option<usize>,
	pub expanded_ids: HashSet<String>,
	pub follow_tail: bool,
	/// 表示先頭からの行オフセット (clamp 前)。描画層が clamp + 選択追従 + 末尾
	/// 到達判定を適用した値を `set_scroll_offset()` で書き戻す。
	pub scroll_offset_rows: u32,
	/// 「次の描画フレームで auto_follow_selection を有効化する」フラグ。
	/// キー操作で selection を動かした直後だけ true になる。描画層が
	/// `consume_pending_follow_selection()` で消費する。
	pub pending_follow_selection: bool,
	/// **ユーザーが個別に開閉トグルした** メッセージ id の集合。
	///
	/// Enter / Space / 個別クリック (`select_or_toggle_by_id` の toggle 経路) で
	/// insert される。一度入った id は **以後その bubble の自動 close 対象から
	/// 外される** (= 「最新の Bash が来ても閉じない」)。
	///
	/// **Ctrl+O の全体トグルはここに入れない** (要件: 「Ctrl+O は明示的操作に
	/// 含まない」)。Ctrl+O は `expanded_ids` を mass-insert / clear するだけ。
	pub user_toggled_ids: HashSet<String>,
}

impl Default for ViewportState {
	fn default() -> Self {
		Self {
			selected_index: None,
			expanded_ids: HashSet::new(),
			follow_tail: true,
			scroll_offset_rows: 0,
			pending_follow_selection: false,
			user_toggled_ids: HashSet::new(),
		}
	}
}

impl ViewportState {
	/// アタッチ切替等で初期化する。
	pub fn reset(&mut self) {
		self.selected_index = None;
		self.expanded_ids.clear();
		self.follow_tail = true;
		self.scroll_offset_rows = 0;
		self.pending_follow_selection = false;
		self.user_toggled_ids.clear();
	}

	/// 1 行上に移動 (最上端を超えない)。
	///
	/// TS 版互換: `selected_index=-1` 相当 (None) は末尾扱いにしてから -1。
	/// `follow_tail` は false にする (上方向移動は末尾追従を解除)。
	pub fn move_up(&mut self, total_messages: usize) {
		if total_messages == 0 {
			self.selected_index = None;
			return;
		}
		let current = self.selected_index.unwrap_or(total_messages - 1);
		let next = current.saturating_sub(1);
		if Some(next) == self.selected_index {
			return;
		}
		self.selected_index = Some(next);
		self.follow_tail = false;
		self.pending_follow_selection = true;
	}

	/// 1 行下に移動 (最下端を超えない)。
	///
	/// TS 版互換: `follow_tail` は触らない (下方向移動は末尾に達しても
	/// 描画層が `SET_SCROLL_OFFSET` で true 復帰させる)。
	pub fn move_down(&mut self, total_messages: usize) {
		if total_messages == 0 {
			self.selected_index = None;
			return;
		}
		let last = total_messages - 1;
		let current = self.selected_index.unwrap_or(0);
		let next = (current + 1).min(last);
		if Some(next) == self.selected_index {
			return;
		}
		self.selected_index = Some(next);
		self.pending_follow_selection = true;
	}

	/// 半ページ単位で下にジャンプする。
	pub fn page_down(&mut self, total_messages: usize, page_rows: usize) {
		if total_messages == 0 {
			return;
		}
		let step = (page_rows / 2).max(1);
		let current = self.selected_index.unwrap_or(0);
		let next = (current + step).min(total_messages - 1);
		if Some(next) == self.selected_index {
			return;
		}
		self.selected_index = Some(next);
		self.pending_follow_selection = true;
	}

	/// 半ページ単位で上にジャンプする。
	pub fn page_up(&mut self, total_messages: usize, page_rows: usize) {
		if total_messages == 0 {
			return;
		}
		let step = (page_rows / 2).max(1);
		let current = self
			.selected_index
			.unwrap_or(total_messages.saturating_sub(1));
		let next = current.saturating_sub(step);
		if Some(next) == self.selected_index {
			return;
		}
		self.selected_index = Some(next);
		self.follow_tail = false;
		self.pending_follow_selection = true;
	}

	pub fn jump_top(&mut self, total_messages: usize) {
		if total_messages == 0 {
			return;
		}
		if self.selected_index == Some(0) && !self.follow_tail {
			return;
		}
		self.selected_index = Some(0);
		self.follow_tail = false;
		self.pending_follow_selection = true;
	}

	pub fn jump_bottom(&mut self, total_messages: usize) {
		if total_messages == 0 {
			self.follow_tail = true;
			return;
		}
		self.selected_index = Some(total_messages - 1);
		self.follow_tail = true;
		self.pending_follow_selection = true;
	}

	/// 選択中のメッセージ id を受け取って expand/collapse をトグル。
	///
	/// **明示的なユーザー操作** として `user_toggled_ids` にも id を記録する。
	/// 一度ここを通った bubble は以後 latest Bash 自動 close の対象から外れる
	/// (= ユーザーが意図的に開閉した状態を尊重する)。Ctrl+O の全体トグルは
	/// この関数を経由しないので、`user_toggled_ids` への記録対象外。
	pub fn toggle_expand(&mut self, message_id: &str) {
		if self.expanded_ids.contains(message_id) {
			self.expanded_ids.remove(message_id);
		} else {
			self.expanded_ids.insert(message_id.to_string());
		}
		self.user_toggled_ids.insert(message_id.to_string());
	}

	/// 行ベースでスクロールする (マウスホイール用)。
	///
	/// - 正の `row_delta` で下へ、負で上へ移動
	/// - `selected_index` は**動かさない**
	/// - **`pending_follow_selection` は触らない** (スクロール起源のフレームでは
	///   選択が画面外でも offset を寄せ直さない)
	/// - `follow_tail` は **上方向スクロール時のみ** false にする。
	///   下方向スクロールは描画層で clamp された結果末尾到達していれば
	///   `set_scroll_offset(max_offset, true)` で書き戻される。毎 event で
	///   false → true の flip が起きると redraw ループの火種になる
	///   (トラックパッドの 1 フリックで 40+ event が連続発火するケース)。
	pub fn scroll_by_rows(&mut self, row_delta: i32) {
		if row_delta == 0 {
			return;
		}
		let new_offset: i64 = self.scroll_offset_rows as i64 + row_delta as i64;
		self.scroll_offset_rows = new_offset.max(0) as u32;
		// 下方向スクロール中は follow_tail を維持。末尾離脱は描画層が
		// effective_offset < max_offset を見て判定し `set_scroll_offset(_, false)`
		// で書き戻す。
		if row_delta < 0 {
			self.follow_tail = false;
		}
	}

	/// 描画層からの同期書き戻し。
	///
	/// 同値なら no-op、差分があるときだけ state を更新する。
	pub fn set_scroll_offset(&mut self, offset: u32, follow_tail: bool) -> bool {
		let mut changed = false;
		if self.scroll_offset_rows != offset {
			self.scroll_offset_rows = offset;
			changed = true;
		}
		if self.follow_tail != follow_tail {
			self.follow_tail = follow_tail;
			changed = true;
		}
		changed
	}

	/// 描画層が `pending_follow_selection` を読み取った後に呼び出す。
	///
	/// - 戻り値: 呼び出し時点のフラグ (true なら auto_follow_selection を効かせる)
	/// - 呼び出し後は必ず false に落とす
	pub fn consume_pending_follow_selection(&mut self) -> bool {
		let was = self.pending_follow_selection;
		self.pending_follow_selection = false;
		was
	}

	/// 指定 id のメッセージを選択する (マウスクリック用)。見つからなければ no-op。
	pub fn select_by_id(&mut self, total_messages: usize, _message_id: &str, index: usize) {
		if index >= total_messages {
			return;
		}
		if self.selected_index == Some(index) {
			return;
		}
		self.selected_index = Some(index);
		// 末尾以外を選択したときだけ follow_tail を明示的に false に
		if index != total_messages - 1 {
			self.follow_tail = false;
		}
		self.pending_follow_selection = true;
	}

	/// 未選択 → 選択、既選択 → トグル (マウスクリック再クリック用)。
	pub fn select_or_toggle_by_id(
		&mut self,
		total_messages: usize,
		message_id: &str,
		index: usize,
	) {
		if index >= total_messages {
			return;
		}
		if self.selected_index == Some(index) {
			self.toggle_expand(message_id);
			return;
		}
		self.selected_index = Some(index);
		if index != total_messages - 1 {
			self.follow_tail = false;
		}
		self.pending_follow_selection = true;
	}

	/// 新着メッセージ到着時の処理。
	///
	/// TS 版互換: **`selected_index=None` + `follow_tail=true`** のときだけ
	/// 末尾を選択する (初期状態からの吸着)。それ以外はユーザーのスクロール /
	/// 選択位置を尊重し、描画層が `follow_tail=true` を見て scroll_offset_rows
	/// を末尾に寄せる。
	pub fn on_messages_appended(&mut self, _previous_count: usize, new_count: usize) {
		if new_count == 0 {
			self.selected_index = None;
			return;
		}
		if self.selected_index.is_none() && self.follow_tail {
			// 初期吸着: None のときだけ selection を末尾へ
			self.selected_index = Some(new_count - 1);
		}
		// それ以外は reducer では何もしない。描画層が follow_tail を見て
		// scroll_offset_rows を max_offset に寄せる
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn move_down_from_empty_selection_goes_to_second() {
		// None (selected_index=-1 相当) は `unwrap_or(0)` で 0 扱い → move_down で
		// 0+1=1 に移動する。テスト名とコメントが「first」だと意味が取れないので
		// 「second」(= index 1) に合わせる。
		let mut v = ViewportState::default();
		v.move_down(5);
		assert_eq!(v.selected_index, Some(1));
	}

	#[test]
	fn move_up_from_empty_selection_goes_to_second_last() {
		// unwrap_or(last) → last - 1 = total - 2
		let mut v = ViewportState::default();
		v.move_up(5);
		assert_eq!(v.selected_index, Some(3));
		assert!(!v.follow_tail, "upward move clears follow_tail");
	}

	#[test]
	fn move_down_does_not_touch_follow_tail() {
		let mut v = ViewportState {
			follow_tail: true,
			..ViewportState::default()
		};
		v.selected_index = Some(3);
		v.move_down(5);
		assert_eq!(v.selected_index, Some(4));
		// TS 互換: MOVE_DOWN は follow_tail を触らない
		assert!(v.follow_tail);
	}

	#[test]
	fn move_up_sets_pending_follow_selection() {
		let mut v = ViewportState::default();
		v.move_up(5);
		assert!(v.pending_follow_selection);
	}

	#[test]
	fn scroll_by_rows_does_not_set_pending_follow_selection() {
		let mut v = ViewportState::default();
		v.scroll_by_rows(-3);
		assert!(
			!v.pending_follow_selection,
			"wheel scroll must not enable auto_follow_selection"
		);
	}

	#[test]
	fn consume_pending_follow_selection_returns_and_clears() {
		let mut v = ViewportState {
			pending_follow_selection: true,
			..ViewportState::default()
		};
		assert!(v.consume_pending_follow_selection());
		assert!(!v.pending_follow_selection);
		// Idempotent
		assert!(!v.consume_pending_follow_selection());
	}

	#[test]
	fn jump_top_clears_follow_tail() {
		let mut v = ViewportState::default();
		v.jump_top(5);
		assert_eq!(v.selected_index, Some(0));
		assert!(!v.follow_tail);
		assert!(v.pending_follow_selection);
	}

	#[test]
	fn jump_bottom_sets_follow_tail() {
		let mut v = ViewportState {
			selected_index: Some(0),
			follow_tail: false,
			..ViewportState::default()
		};
		v.jump_bottom(5);
		assert_eq!(v.selected_index, Some(4));
		assert!(v.follow_tail);
		assert!(v.pending_follow_selection);
	}

	#[test]
	fn toggle_expand_adds_and_removes() {
		let mut v = ViewportState::default();
		v.toggle_expand("m1");
		assert!(v.expanded_ids.contains("m1"));
		v.toggle_expand("m1");
		assert!(!v.expanded_ids.contains("m1"));
	}

	#[test]
	fn messages_appended_only_attaches_when_selection_is_none() {
		// 初期状態 (None + follow_tail=true) → 吸着
		let mut v = ViewportState::default();
		v.on_messages_appended(0, 5);
		assert_eq!(v.selected_index, Some(4));

		// 既に選択があれば末尾に吸着しない (TS 互換)
		let mut v = ViewportState {
			selected_index: Some(2),
			follow_tail: true,
			..ViewportState::default()
		};
		v.on_messages_appended(3, 5);
		assert_eq!(v.selected_index, Some(2), "selection stays");
	}

	#[test]
	fn messages_appended_does_not_attach_when_follow_tail_false() {
		let mut v = ViewportState {
			follow_tail: false,
			..ViewportState::default()
		};
		v.on_messages_appended(0, 5);
		assert_eq!(v.selected_index, None);
	}

	#[test]
	fn scroll_by_rows_moves_offset_without_touching_selection() {
		let mut v = ViewportState {
			selected_index: Some(3),
			follow_tail: true,
			scroll_offset_rows: 10,
			..ViewportState::default()
		};
		v.scroll_by_rows(-3);
		assert_eq!(v.scroll_offset_rows, 7);
		assert_eq!(v.selected_index, Some(3));
		assert!(!v.follow_tail);
	}

	#[test]
	fn scroll_by_rows_downward_preserves_follow_tail() {
		// トラックパッドの 1 フリックで ScrollDown が 40+ event 連発するケースで、
		// 毎 event に follow_tail を false にすると描画層が即 true に戻して
		// flip → dirty → redraw ループになる。下方向スクロールは follow_tail
		// を維持し、末尾離脱は描画層の clamp で判定する。
		let mut v = ViewportState {
			follow_tail: true,
			scroll_offset_rows: 5,
			..ViewportState::default()
		};
		v.scroll_by_rows(3);
		assert_eq!(v.scroll_offset_rows, 8);
		assert!(v.follow_tail, "downward scroll must keep follow_tail");
	}

	#[test]
	fn scroll_by_rows_upward_clears_follow_tail() {
		let mut v = ViewportState {
			follow_tail: true,
			scroll_offset_rows: 10,
			..ViewportState::default()
		};
		v.scroll_by_rows(-3);
		assert_eq!(v.scroll_offset_rows, 7);
		assert!(!v.follow_tail, "upward scroll must clear follow_tail");
	}

	#[test]
	fn scroll_by_rows_does_not_underflow() {
		let mut v = ViewportState {
			scroll_offset_rows: 2,
			..ViewportState::default()
		};
		v.scroll_by_rows(-10);
		assert_eq!(v.scroll_offset_rows, 0);
	}

	#[test]
	fn scroll_by_rows_zero_is_noop() {
		let mut v = ViewportState {
			follow_tail: true,
			scroll_offset_rows: 5,
			..ViewportState::default()
		};
		v.scroll_by_rows(0);
		assert_eq!(v.scroll_offset_rows, 5);
		assert!(v.follow_tail);
	}

	#[test]
	fn set_scroll_offset_reports_changes() {
		let mut v = ViewportState::default();
		assert!(v.set_scroll_offset(10, false));
		assert_eq!(v.scroll_offset_rows, 10);
		assert!(!v.follow_tail);
		// 同じ値で呼び直すと no-op
		assert!(!v.set_scroll_offset(10, false));
	}

	#[test]
	fn set_scroll_offset_restores_follow_tail() {
		let mut v = ViewportState {
			follow_tail: false,
			scroll_offset_rows: 5,
			..ViewportState::default()
		};
		assert!(v.set_scroll_offset(15, true));
		assert!(v.follow_tail);
	}

	#[test]
	fn select_by_id_updates_selection_and_follow_tail() {
		let mut v = ViewportState::default();
		v.select_by_id(5, "id-2", 2);
		assert_eq!(v.selected_index, Some(2));
		assert!(!v.follow_tail);
		assert!(v.pending_follow_selection);

		// 末尾を選択すると follow_tail はそのまま (=true 維持)
		v.follow_tail = true;
		v.pending_follow_selection = false;
		v.select_by_id(5, "id-4", 4);
		assert_eq!(v.selected_index, Some(4));
		assert!(v.follow_tail);
		assert!(v.pending_follow_selection);
	}

	#[test]
	fn select_by_id_out_of_range_is_ignored() {
		let mut v = ViewportState::default();
		v.select_by_id(3, "oops", 10);
		assert_eq!(v.selected_index, None);
		assert!(!v.pending_follow_selection);
	}

	#[test]
	fn select_or_toggle_by_id_behaves_like_click_then_reclick() {
		let mut v = ViewportState::default();
		v.select_or_toggle_by_id(5, "m-2", 2);
		assert_eq!(v.selected_index, Some(2));
		assert!(!v.expanded_ids.contains("m-2"));
		assert!(v.pending_follow_selection);

		v.pending_follow_selection = false;
		v.select_or_toggle_by_id(5, "m-2", 2);
		assert_eq!(v.selected_index, Some(2));
		assert!(v.expanded_ids.contains("m-2"));
		// 同じメッセージへの再クリックは pending_follow_selection を立てない
		assert!(!v.pending_follow_selection);

		v.select_or_toggle_by_id(5, "m-2", 2);
		assert!(!v.expanded_ids.contains("m-2"));

		v.select_or_toggle_by_id(5, "m-3", 3);
		assert_eq!(v.selected_index, Some(3));
	}

	/// Issue 007 再現テスト: ホイール上スクロール連打で offset が max に引き戻されない。
	///
	/// 流れ: 末尾追従状態 → 描画層が `set_scroll_offset(max_offset, true)` で
	/// scroll_offset_rows を max に書き戻す → ユーザーがホイール上 → 次フレームで
	/// auto_follow_selection が false (pending=false) → 引き戻しされない。
	///
	/// `WHEEL_DELTA_ROWS` で 1 tick あたりの移動量を参照するので、定数を調整
	/// しても破綻しない。
	#[test]
	fn scroll_by_rows_repeatedly_does_not_loop_back() {
		let mut v = ViewportState::default();
		// 初期状態: None → 新着 10 件で selection 末尾に吸着
		v.on_messages_appended(0, 10);
		assert_eq!(v.selected_index, Some(9));
		assert!(v.follow_tail);

		// 描画層が「全メッセージ積み上がり 100 行、viewport 20 行、max_offset=80」
		// を検出して set_scroll_offset(80, true) を呼ぶ (useEffect 相当)
		v.set_scroll_offset(80, true);
		assert_eq!(v.scroll_offset_rows, 80);

		let delta = WHEEL_DELTA_ROWS as u32;

		// ホイール上 3 連打 → 1 tick ごとに delta だけ減る
		v.scroll_by_rows(-WHEEL_DELTA_ROWS);
		assert_eq!(v.scroll_offset_rows, 80 - delta);
		assert!(!v.follow_tail);
		assert!(
			!v.pending_follow_selection,
			"wheel must not trigger auto_follow_selection"
		);

		v.scroll_by_rows(-WHEEL_DELTA_ROWS);
		assert_eq!(v.scroll_offset_rows, 80 - 2 * delta);
		v.scroll_by_rows(-WHEEL_DELTA_ROWS);
		assert_eq!(v.scroll_offset_rows, 80 - 3 * delta);

		// selection は末尾のまま動いていない
		assert_eq!(v.selected_index, Some(9));
		// follow_tail は落ちたまま
		assert!(!v.follow_tail);
		assert!(!v.pending_follow_selection);
	}

	/// reducer を経由した「描画 → set_scroll_offset 書き戻し」のループ不在を検証。
	#[test]
	fn set_scroll_offset_is_idempotent_across_frames() {
		let mut v = ViewportState::default();
		v.on_messages_appended(0, 10);
		v.scroll_by_rows(-3);
		let offset_before = v.scroll_offset_rows;

		// 描画層が set_scroll_offset を呼ぶ (clamp 後の値)
		// このケースでは follow_tail=false で scroll_offset_rows=offset_before が clamp 済み
		let changed1 = v.set_scroll_offset(offset_before, false);
		// pending_follow_selection=false なので描画層は offset を触らない → 同値
		assert!(!changed1);

		let changed2 = v.set_scroll_offset(offset_before, false);
		assert!(!changed2, "second write-back must be noop");
	}
}
