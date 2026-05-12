//! SessionFilterService。TS 版 `src/domain/services/SessionFilterService.ts` の移植。
//!
//! Rust では class を作らず純粋関数として公開する。状態を持たないので struct
//! でラップする必要がない。

use crate::domain::constants::LOCAL_COMMAND_PREFIXES;
use crate::domain::entities::SessionMetadata;

/// ユーザーの明示的なアクションかどうか判定する。
///
/// ローカルコマンド関連のプレフィックス (`<local-command-caveat>` 等) で
/// 始まる場合は `false`。
pub fn is_explicit_user_action(content: &str) -> bool {
	!LOCAL_COMMAND_PREFIXES
		.iter()
		.any(|prefix| content.starts_with(prefix))
}

/// セッション一覧に出す表示テキストを返す。
///
/// 優先順位: `summary` (非空) > `first_prompt` (非空) > `"(新規セッション)"`。
pub fn get_display_text(metadata: &SessionMetadata) -> String {
	if let Some(summary) = metadata.summary.as_deref() {
		if !summary.is_empty() {
			return summary.to_string();
		}
	}

	if !metadata.first_prompt.is_empty() {
		return metadata.first_prompt.clone();
	}

	"(新規セッション)".to_string()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn explicit_for_plain_text() {
		assert!(is_explicit_user_action("Hello, world!"));
		assert!(is_explicit_user_action("This is a normal message"));
		assert!(is_explicit_user_action("こんにちは"));
	}

	#[test]
	fn not_explicit_for_local_command_prefixes() {
		assert!(!is_explicit_user_action("<local-command-caveat>message"));
		assert!(!is_explicit_user_action("<local-command-caveat>"));
		assert!(!is_explicit_user_action("<command-name>exit"));
		assert!(!is_explicit_user_action("<command-name>"));
		assert!(!is_explicit_user_action("<command-message>foo"));
		assert!(!is_explicit_user_action("<command-message>"));
		assert!(!is_explicit_user_action("<local-command-stdout>out"));
		assert!(!is_explicit_user_action("<local-command-stdout>"));
	}

	#[test]
	fn explicit_when_prefix_is_embedded_not_leading() {
		assert!(is_explicit_user_action(
			"Some text <local-command-caveat>message"
		));
		assert!(is_explicit_user_action("Prefix <command-name>exit"));
	}

	#[test]
	fn explicit_for_empty_string() {
		assert!(is_explicit_user_action(""));
	}

	#[test]
	fn display_text_prefers_summary() {
		let meta = SessionMetadata {
			first_prompt: "First prompt".to_string(),
			summary: Some("This is summary".to_string()),
			last_user_action_at: None,
		};
		assert_eq!(get_display_text(&meta), "This is summary");
	}

	#[test]
	fn display_text_falls_back_to_first_prompt() {
		let meta = SessionMetadata {
			first_prompt: "This is first prompt".to_string(),
			summary: None,
			last_user_action_at: None,
		};
		assert_eq!(get_display_text(&meta), "This is first prompt");
	}

	#[test]
	fn display_text_keeps_placeholder_if_first_prompt_is_placeholder() {
		let meta = SessionMetadata {
			first_prompt: "(新規セッション)".to_string(),
			summary: None,
			last_user_action_at: None,
		};
		assert_eq!(get_display_text(&meta), "(新規セッション)");
	}

	#[test]
	fn display_text_returns_placeholder_when_empty() {
		let meta = SessionMetadata {
			first_prompt: String::new(),
			summary: None,
			last_user_action_at: None,
		};
		assert_eq!(get_display_text(&meta), "(新規セッション)");
	}

	#[test]
	fn display_text_falls_back_when_summary_is_empty() {
		let meta = SessionMetadata {
			first_prompt: "First prompt".to_string(),
			summary: Some(String::new()),
			last_user_action_at: None,
		};
		assert_eq!(get_display_text(&meta), "First prompt");
	}
}
