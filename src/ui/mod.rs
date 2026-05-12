//! UI 層: ratatui による描画。
//!
//! 構成:
//! view (トップレベル) / screens (画面ごと) / components (ChatBubble 等) /
//! layout (viewport 計算) / icons + format (アイコンマップ / フォーマッタ)。

pub mod components;
pub mod format;
pub mod icons;
pub mod layout;
pub mod screens;
pub mod view;
