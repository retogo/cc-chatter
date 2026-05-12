//! エージェントタイプ別アイコン。
//!
//! TS 版 `src/ui/constants/agentIcons.ts` と同じマップ。仕様は `docs/spec.md`
//! の「エージェントタイプアイコン」節を参照。

/// subagent_type → 表示用絵文字。未知の type は `🔸`。
pub fn get_agent_icon(agent_type: &str) -> &'static str {
	match agent_type {
		"main" => "🤖",
		"Explore" => "🔍",
		"general-purpose" => "🔹",
		"Plan" => "📋",
		"Bash" => "💻",
		"claude-code-guide" => "📚",
		"statusline-setup" => "⚙️",
		_ => "🔸",
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn maps_known_types() {
		assert_eq!(get_agent_icon("main"), "🤖");
		assert_eq!(get_agent_icon("Explore"), "🔍");
		assert_eq!(get_agent_icon("general-purpose"), "🔹");
		assert_eq!(get_agent_icon("Plan"), "📋");
	}

	#[test]
	fn unknown_type_falls_back_to_diamond() {
		assert_eq!(get_agent_icon("unknown"), "🔸");
		assert_eq!(get_agent_icon(""), "🔸");
		assert_eq!(get_agent_icon("SomeCustomType"), "🔸");
	}
}
