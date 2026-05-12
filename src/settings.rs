/// ChatBubble の表示モード。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatMode {
	/// 現状どおり (Main=右 / Sub=左、左 1 文字マーカーのみ)。
	#[default]
	Default,
	/// LINE アプリ風: 上下左右 4 辺の枠線で囲む。寄せは default と同じ。
	Line,
	/// Slack 風: すべて左寄せ。枠線は default と同じ (左 1 文字マーカー)。
	Slack,
}

impl ChatMode {
	pub fn cycle(self) -> Self {
		match self {
			Self::Default => Self::Line,
			Self::Line => Self::Slack,
			Self::Slack => Self::Default,
		}
	}
}
