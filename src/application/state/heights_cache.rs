//! メッセージ高さのキャッシュ。
//!
//! `estimate_bubble_height` は `wrap_line_count` 経由で `chars().count()` を
//! 呼ぶため、tool_result の content が数 KB 以上のときに O(len) のコストが
//! 発生する。WATCHING 画面は毎フレーム全メッセージの heights を必要とするので、
//! N 件 × frame_rate に対して O(total_content_chars) / sec が乗ってしまう
//! (実測: general-purpose 系の長い tool_result を 1000 件で 1085 μs/frame)。
//!
//! メッセージは **immutable** (id で識別され、一度追加されたら content は変わらない)
//! なので、追加時に 1 度だけ `(preview_h, expanded_h)` を計算してキャッシュに
//! 積む戦略が効く。`is_detailed_mode` や `expanded_ids` のトグルはキャッシュを
//! 無効化しない (両方のパターンを事前に計算してあるため)。
//!
//! ## 無効化条件
//!
//! - `content_width` が変わったとき (ターミナル幅変更)
//! - メッセージ配列が前から drain されたとき (`MAX_MESSAGES` 超過時)
//!
//! どちらも全件再計算する。content_width の変更は稀 (resize 時のみ)、drain は
//! MAX_MESSAGES 件ごとに 1 回のみなので、償却コストは十分低い。

use crate::domain::entities::FormattedMessage;
use crate::settings::ChatMode;
use crate::ui::layout::estimate_bubble_height_full;

/// メッセージ 1 件あたりの (preview_height, expanded_height)。
#[derive(Debug, Clone, Copy, Default)]
pub struct MessageHeights {
	pub preview: u16,
	pub expanded: u16,
}

#[derive(Debug, Default)]
pub struct HeightsCache {
	/// 最後に計算に使った `content_width`。0 は未初期化。
	width: u16,
	/// 最後に計算に使った `chat_mode`。既定は `Default` (`ChatMode::default()`)。
	///
	/// LINE モードでは top/bottom border 2 行ぶん高さが増え、本文折り返し幅も
	/// 縮むため、モード切替は `content_width` 変更と同様に全件再計算が必要。
	/// `m` キーによる runtime 切替で `sync` 経路から検出 → 全件再計算が走る。
	chat_mode: ChatMode,
	/// `messages` と同じ順序・長さ。
	entries: Vec<MessageHeights>,
}

impl HeightsCache {
	/// キャッシュを `messages` + `content_width` + `chat_mode` に同期する。
	///
	/// - `content_width` または `chat_mode` が変わったとき → 全再計算
	/// - `messages` 長が cache より **短い** → 全再計算 (先頭 drain と見なす)
	/// - `messages` 長が cache より **長い** → 末尾追加分だけ計算
	/// - 同一長 → no-op
	///
	/// `resolve_agent_type` は `message.agent_id` → mention 計算用 `agent_type`
	/// (見つからなければ `"unknown"` 相当) を解決する closure。mention prefix
	/// (`@{agent_type}` / `@Main`) の表示幅計算を render 側と揃えるために必要。
	///
	/// 戻り値の lifetime は closure 呼び出し境界内でのみ有効で良いため、
	/// `for<'a> Fn(&str) -> &'a str` ではなく同一 scope 内 borrow で組み立てる。
	/// シグネチャは簡単のため所有文字列を返す設計にする (agent_type は
	/// せいぜい十数文字なので clone コストは無視できる)。
	pub fn sync(
		&mut self,
		messages: &[FormattedMessage],
		content_width: u16,
		chat_mode: ChatMode,
		mut resolve_agent_type: impl FnMut(&str) -> String,
	) {
		if content_width == 0 {
			return;
		}
		if self.width != content_width
			|| self.chat_mode != chat_mode
			|| self.entries.len() > messages.len()
		{
			self.width = content_width;
			self.chat_mode = chat_mode;
			self.entries.clear();
			self.entries.reserve(messages.len());
			for m in messages {
				let at = resolve_agent_type(m.agent_id.as_str());
				self.entries.push(MessageHeights {
					preview: estimate_bubble_height_full(m, false, content_width, &at, chat_mode),
					expanded: estimate_bubble_height_full(m, true, content_width, &at, chat_mode),
				});
			}
			return;
		}
		// 末尾追加分のみ計算
		for m in &messages[self.entries.len()..] {
			let at = resolve_agent_type(m.agent_id.as_str());
			self.entries.push(MessageHeights {
				preview: estimate_bubble_height_full(m, false, content_width, &at, chat_mode),
				expanded: estimate_bubble_height_full(m, true, content_width, &at, chat_mode),
			});
		}
	}

	/// 指定 index のメッセージ高さを取り出す。範囲外なら 1 (最低高さ相当)。
	pub fn get(&self, idx: usize, expanded: bool) -> u16 {
		self.entries
			.get(idx)
			.map(|e| if expanded { e.expanded } else { e.preview })
			.unwrap_or(1)
	}

	/// 指定 index のキャッシュを部分的に再計算する。
	///
	/// メッセージが mutate された (例: tool_result が後から attach された) ときに
	/// 呼ぶ。`content_width=0` のとき、または `width` が未初期化 / 不一致のとき、
	/// または index 範囲外のときは何もしない (次の `sync` で全件再計算される)。
	///
	/// immutable 前提のキャッシュを敢えて更新するのは、ツール呼び出しと結果を
	/// 1 メッセージに集約するドメイン設計の都合 (Slack スレッド風 pairing)。
	/// 呼び出しは per-tool_result なので頻度は極端には上がらない。
	pub fn invalidate(
		&mut self,
		idx: usize,
		msg: &FormattedMessage,
		content_width: u16,
		chat_mode: ChatMode,
		agent_type: &str,
	) {
		if content_width == 0 {
			return;
		}
		if self.width != content_width || self.chat_mode != chat_mode {
			// 幅 or モードが食い違うなら個別更新は信頼できない。次の sync で全件再計算。
			return;
		}
		if idx >= self.entries.len() {
			return;
		}
		self.entries[idx] = MessageHeights {
			preview: estimate_bubble_height_full(msg, false, content_width, agent_type, chat_mode),
			expanded: estimate_bubble_height_full(msg, true, content_width, agent_type, chat_mode),
		};
	}

	/// アタッチ切替等でキャッシュを初期化する。
	pub fn clear(&mut self) {
		self.entries.clear();
		self.width = 0;
		self.chat_mode = ChatMode::default();
	}

	/// キャッシュされているメッセージ数 (= 最後の `sync` 時点の messages.len)。
	pub fn len(&self) -> usize {
		self.entries.len()
	}

	/// キャッシュが空かどうか。
	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}

	#[cfg(test)]
	pub fn width(&self) -> u16 {
		self.width
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::domain::entities::Sender;
	use chrono::Utc;

	fn msg(idx: usize, text: &str) -> FormattedMessage {
		FormattedMessage {
			id: format!("m-{idx}"),
			sender: Sender::Sub,
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
	fn sync_populates_cache_from_scratch() {
		let mut cache = HeightsCache::default();
		let messages = vec![msg(0, "hello"), msg(1, "world")];
		cache.sync(&messages, 80, ChatMode::Default, |_| "unknown".to_string());
		assert_eq!(cache.len(), 2);
		assert_eq!(cache.width(), 80);
	}

	#[test]
	fn sync_with_width_change_recomputes_all() {
		let mut cache = HeightsCache::default();
		let messages = vec![msg(0, "hello"), msg(1, "world")];
		cache.sync(&messages, 80, ChatMode::Default, |_| "unknown".to_string());
		let h_width80 = cache.get(0, false);
		cache.sync(&messages, 5, ChatMode::Default, |_| "unknown".to_string()); // 新しい狭い幅
		let h_width5 = cache.get(0, false);
		// 幅が狭くなれば折り返し行数が増えるはず
		assert!(h_width5 >= h_width80);
		assert_eq!(cache.width(), 5);
	}

	#[test]
	fn sync_extends_on_append_without_recompute() {
		let mut cache = HeightsCache::default();
		let mut messages = vec![msg(0, "hello")];
		cache.sync(&messages, 80, ChatMode::Default, |_| "unknown".to_string());
		assert_eq!(cache.len(), 1);
		messages.push(msg(1, "more"));
		cache.sync(&messages, 80, ChatMode::Default, |_| "unknown".to_string());
		assert_eq!(cache.len(), 2);
		assert_eq!(cache.width(), 80);
	}

	#[test]
	fn sync_with_shorter_messages_triggers_full_recompute() {
		let mut cache = HeightsCache::default();
		let messages = vec![msg(0, "a"), msg(1, "b"), msg(2, "c")];
		cache.sync(&messages, 80, ChatMode::Default, |_| "unknown".to_string());
		assert_eq!(cache.len(), 3);
		// drain_front が起きたケース: 先頭 1 件が消えて 2 件残る
		let shorter = vec![msg(1, "b"), msg(2, "c")];
		cache.sync(&shorter, 80, ChatMode::Default, |_| "unknown".to_string());
		assert_eq!(cache.len(), 2);
	}

	#[test]
	fn zero_width_is_no_op() {
		let mut cache = HeightsCache::default();
		cache.sync(&[msg(0, "x")], 0, ChatMode::Default, |_| {
			"unknown".to_string()
		});
		assert_eq!(cache.len(), 0);
	}

	#[test]
	fn get_returns_preview_or_expanded_height() {
		let mut cache = HeightsCache::default();
		let messages = vec![msg(0, "some text")];
		cache.sync(&messages, 80, ChatMode::Default, |_| "unknown".to_string());
		let p = cache.get(0, false);
		let e = cache.get(0, true);
		assert!(p >= 3); // header + body + margin 最低 3
		assert!(e >= 3);
	}

	#[test]
	fn clear_resets_cache() {
		let mut cache = HeightsCache::default();
		cache.sync(&[msg(0, "x")], 80, ChatMode::Default, |_| {
			"unknown".to_string()
		});
		assert_eq!(cache.len(), 1);
		cache.clear();
		assert_eq!(cache.len(), 0);
		assert_eq!(cache.width(), 0);
	}

	#[test]
	fn invalidate_recomputes_only_the_target_index() {
		let mut cache = HeightsCache::default();
		// 2 件: 最初は両方とも短い text。
		let mut messages = vec![msg(0, "short0"), msg(1, "short1")];
		cache.sync(&messages, 40, ChatMode::Default, |_| "unknown".to_string());
		let before_0 = cache.get(0, true);
		let before_1 = cache.get(1, true);

		// m-0 の text をはるかに長くして (wrap で行数が増える) 個別無効化。
		messages[0].text = Some("x".repeat(400));
		cache.invalidate(0, &messages[0], 40, ChatMode::Default, "unknown");

		let after_0 = cache.get(0, true);
		let after_1 = cache.get(1, true);
		assert!(
			after_0 > before_0,
			"invalidated entry must recompute and grow: {before_0} -> {after_0}"
		);
		assert_eq!(
			after_1, before_1,
			"non-invalidated entry stays untouched: {before_1} vs {after_1}"
		);
	}

	#[test]
	fn invalidate_is_noop_when_width_mismatch_or_out_of_range() {
		let mut cache = HeightsCache::default();
		let messages = vec![msg(0, "a"), msg(1, "b")];
		cache.sync(&messages, 40, ChatMode::Default, |_| "unknown".to_string());
		let before = cache.get(0, false);

		// 幅不一致 → 何もしない
		cache.invalidate(0, &messages[0], 80, ChatMode::Default, "unknown");
		assert_eq!(cache.get(0, false), before);

		// 0 width → 何もしない
		cache.invalidate(0, &messages[0], 0, ChatMode::Default, "unknown");
		assert_eq!(cache.get(0, false), before);

		// 範囲外 index → 何もしない (panic しない)
		cache.invalidate(99, &messages[0], 40, ChatMode::Default, "unknown");
		assert_eq!(cache.len(), 2);
	}
}
