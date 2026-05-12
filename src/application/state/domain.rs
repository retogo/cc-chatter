//! Domain state (sessions / agents / messages)。
//!
//! TS 版 `src/application/state/domainState.ts` の移植。

use crate::domain::constants::MAX_MESSAGES;
use crate::domain::entities::{AgentEntity, FormattedMessage, SessionEntity};

/// ビジネスデータの状態。
#[derive(Debug, Clone, Default)]
pub struct DomainState {
	pub sessions: Vec<SessionEntity>,
	pub agents: Vec<AgentEntity>,
	pub messages: Vec<FormattedMessage>,
	pub status_text: String,
}

impl DomainState {
	/// メッセージ配列に新規分を追記し、`MAX_MESSAGES` 超過分を頭から捨てる。
	pub fn append_messages(&mut self, mut new_messages: Vec<FormattedMessage>) {
		if new_messages.is_empty() {
			return;
		}
		self.messages.append(&mut new_messages);
		if self.messages.len() > MAX_MESSAGES {
			let drop = self.messages.len() - MAX_MESSAGES;
			self.messages.drain(0..drop);
		}
	}

	/// アタッチ切替時にメッセージをクリアする。
	pub fn clear_messages(&mut self) {
		self.messages.clear();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::domain::entities::Sender;
	use chrono::Utc;

	fn msg(idx: usize) -> FormattedMessage {
		FormattedMessage {
			id: format!("m-{idx}"),
			sender: Sender::Sub,
			agent_id: "a1".to_string(),
			timestamp: Utc::now(),
			text: Some(format!("msg{idx}")),
			tool_use: None,
			tool_result: None,
			tool_use_id: None,
			result_timestamp: None,
			is_final_response: false,
		}
	}

	#[test]
	fn append_drops_oldest_when_exceeding_max() {
		let mut state = DomainState::default();
		let many: Vec<_> = (0..(MAX_MESSAGES + 5)).map(msg).collect();
		state.append_messages(many);
		assert_eq!(state.messages.len(), MAX_MESSAGES);
		// 先頭 5 件が落ちているので id は m-5 から始まる
		assert_eq!(state.messages[0].id, "m-5");
	}
}
