//! UI レイアウト系ユーティリティ。

pub mod viewport_layout;

pub use viewport_layout::{
	compute_row_based_viewport, estimate_bubble_height, estimate_bubble_height_full,
	estimate_bubble_height_with_mode, estimate_bubble_height_with_prefix, find_message_at_row,
	wrap_lines_iter, wrap_lines_iter_with_prefix, wrap_string_lines, HitItem, ViewportSlice,
};
pub(crate) use viewport_layout::{effective_inner_width, LINE_BORDER_COLS};
