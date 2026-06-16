//! Startup skip ロジック。TS 版 `src/application/usecases/startupSkipLogic.ts` の移植。
//!
//! `--latest` / `--session` / `--agent` CLI オプションの優先順位を解決して、
//! 初期画面を WATCHING / WAITING へスキップするかどうかを返す。
//!
//! TS 側は `process.exit(1)` でエラーメッセージを出していたが、Rust 側では
//! `Result<SkipResult, StartupError>` として呼び出し側 (CLI エントリ) に
//! 通知する。

use thiserror::Error;

use crate::domain::entities::{AgentEntity, SessionEntity};

/// 解決された初期ビュー。
///
/// TS 版 `AppState` 相当だが、M1 では UI/状態機械がまだ無いので最低限の
/// 情報のみを持つ。M2 で `appReducer` を起こすときに拡張する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedView {
	/// エージェントも決まっている (監視即開始)。
	Watching {
		session_id: String,
		agent_id: String,
	},
	/// セッションは決まったがエージェントが無い (新規起動待ち)。
	Waiting { session_id: String },
}

/// スキップ処理の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipResult {
	/// 初期画面をスキップすべきか。
	pub should_skip: bool,
	/// スキップ先のビュー (スキップしないとき None)。
	pub view: Option<ResolvedView>,
	/// 対象セッション ID (スキップするとき Some)。
	pub target_session_id: Option<String>,
	/// 対象エージェント ID (決定したとき Some)。
	pub target_agent_id: Option<String>,
}

impl SkipResult {
	fn no_skip() -> Self {
		Self {
			should_skip: false,
			view: None,
			target_session_id: None,
			target_agent_id: None,
		}
	}
}

/// `resolve_startup_state` が返しうるエラー。
///
/// TS 版では直に `process.exit(1)` していたため `stderr` に出すメッセージを
/// 同じ形で保持する。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StartupError {
	#[error("No session found matching prefix \"{prefix}\"")]
	NoSessionMatch { prefix: String },

	#[error("Multiple sessions match prefix \"{prefix}\": {matches:?}")]
	MultipleSessionMatches {
		prefix: String,
		matches: Vec<String>,
	},
}

/// `--latest` / `--session <prefix>` / `--agent <id>` の優先順位を解決する。
///
/// - `--session` 優先 (前方一致 1 件のみ許可、複数/0 件はエラー)
/// - `--latest` は sessions[0] に自動アタッチ (呼び出し側で降順ソート済み前提)
/// - `--agent` が存在エージェントとマッチしなければ最新エージェントにフォールバック
///
/// どれも指定されていない / セッションが空の場合は `should_skip=false` で返す。
pub fn resolve_startup_state(
	latest: bool,
	session_prefix: Option<&str>,
	agent_id: Option<&str>,
	sessions: &[SessionEntity],
	agents: &[AgentEntity],
) -> Result<SkipResult, StartupError> {
	let session_specified = session_prefix.is_some();
	if !session_specified && !latest {
		return Ok(SkipResult::no_skip());
	}
	if sessions.is_empty() {
		return Ok(SkipResult::no_skip());
	}

	// --- セッションを決定 ------------------------------------------------------
	let target_session: &SessionEntity = if let Some(prefix) = session_prefix {
		let matches: Vec<&SessionEntity> = sessions
			.iter()
			.filter(|s| s.session_id.starts_with(prefix))
			.collect();
		match matches.as_slice() {
			[] => {
				return Err(StartupError::NoSessionMatch {
					prefix: prefix.to_string(),
				});
			}
			[only] => only,
			multiple => {
				return Err(StartupError::MultipleSessionMatches {
					prefix: prefix.to_string(),
					matches: multiple.iter().map(|s| s.session_id.clone()).collect(),
				});
			}
		}
	} else {
		&sessions[0]
	};

	// --- エージェントを決定 ----------------------------------------------------
	if let Some(desired) = agent_id {
		if let Some(matched) = agents.iter().find(|a| a.agent_id == desired) {
			return Ok(SkipResult {
				should_skip: true,
				view: Some(ResolvedView::Watching {
					session_id: target_session.session_id.clone(),
					agent_id: matched.agent_id.clone(),
				}),
				target_session_id: Some(target_session.session_id.clone()),
				target_agent_id: Some(matched.agent_id.clone()),
			});
		}
		// 指定 agent が見つからなければ、TS 版と同じく最新エージェントにフォールバック
		// (TS 版: `return` せず下へ抜ける)
	}

	if let Some(latest_agent) = agents.first() {
		return Ok(SkipResult {
			should_skip: true,
			view: Some(ResolvedView::Watching {
				session_id: target_session.session_id.clone(),
				agent_id: latest_agent.agent_id.clone(),
			}),
			target_session_id: Some(target_session.session_id.clone()),
			target_agent_id: Some(latest_agent.agent_id.clone()),
		});
	}

	// エージェントなし → 待機
	Ok(SkipResult {
		should_skip: true,
		view: Some(ResolvedView::Waiting {
			session_id: target_session.session_id.clone(),
		}),
		target_session_id: Some(target_session.session_id.clone()),
		target_agent_id: None,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::domain::entities::SessionMetadata;
	use chrono::Utc;
	use std::path::PathBuf;

	fn session(id: &str) -> SessionEntity {
		SessionEntity {
			session_id: id.to_string(),
			workspace: "-ws".to_string(),
			file_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
			updated_at: Utc::now(),
			subagents_dir: PathBuf::from(format!("/tmp/{id}/subagents")),
			subagent_count: 0,
			metadata: SessionMetadata::default(),
		}
	}

	fn agent(id: &str) -> AgentEntity {
		AgentEntity {
			agent_id: id.to_string(),
			agent_type: "Explore".to_string(),
			output_path: PathBuf::from(format!("/tmp/agent-{id}.jsonl")),
			updated_at: Utc::now(),
			workflow_run: None,
			workflow_label: None,
		}
	}

	#[test]
	fn returns_no_skip_when_neither_flag_specified() {
		let res = resolve_startup_state(false, None, None, &[session("s1")], &[]).unwrap();
		assert!(!res.should_skip);
		assert!(res.view.is_none());
	}

	#[test]
	fn returns_no_skip_when_sessions_empty() {
		let res = resolve_startup_state(true, None, None, &[], &[]).unwrap();
		assert!(!res.should_skip);
	}

	#[test]
	fn latest_attaches_to_first_session_waiting_when_no_agents() {
		let sessions = vec![session("s1"), session("s2")];
		let res = resolve_startup_state(true, None, None, &sessions, &[]).unwrap();
		assert!(res.should_skip);
		assert_eq!(
			res.view,
			Some(ResolvedView::Waiting {
				session_id: "s1".to_string(),
			})
		);
		assert_eq!(res.target_session_id.as_deref(), Some("s1"));
		assert!(res.target_agent_id.is_none());
	}

	#[test]
	fn latest_attaches_to_first_agent_when_present() {
		let sessions = vec![session("s1")];
		let agents = vec![agent("a1"), agent("a2")];
		let res = resolve_startup_state(true, None, None, &sessions, &agents).unwrap();
		assert!(res.should_skip);
		assert_eq!(
			res.view,
			Some(ResolvedView::Watching {
				session_id: "s1".to_string(),
				agent_id: "a1".to_string(),
			})
		);
	}

	#[test]
	fn session_prefix_single_match_wins() {
		let sessions = vec![session("sess-abc123"), session("other-xyz")];
		let res = resolve_startup_state(false, Some("sess-abc"), None, &sessions, &[]).unwrap();
		assert_eq!(res.target_session_id.as_deref(), Some("sess-abc123"));
	}

	#[test]
	fn session_prefix_no_match_errors() {
		let sessions = vec![session("abc"), session("def")];
		let err = resolve_startup_state(false, Some("zzz"), None, &sessions, &[]).unwrap_err();
		assert_eq!(
			err,
			StartupError::NoSessionMatch {
				prefix: "zzz".to_string()
			}
		);
	}

	#[test]
	fn session_prefix_multiple_matches_errors() {
		let sessions = vec![session("abc-1"), session("abc-2")];
		let err = resolve_startup_state(false, Some("abc-"), None, &sessions, &[]).unwrap_err();
		match err {
			StartupError::MultipleSessionMatches { prefix, matches } => {
				assert_eq!(prefix, "abc-");
				assert_eq!(matches, vec!["abc-1".to_string(), "abc-2".to_string()]);
			}
			_ => panic!("unexpected error"),
		}
	}

	#[test]
	fn agent_id_matched_picks_that_agent() {
		let sessions = vec![session("s1")];
		let agents = vec![agent("a1"), agent("a2"), agent("a3")];
		let res = resolve_startup_state(true, None, Some("a2"), &sessions, &agents).unwrap();
		assert_eq!(
			res.view,
			Some(ResolvedView::Watching {
				session_id: "s1".to_string(),
				agent_id: "a2".to_string(),
			})
		);
	}

	#[test]
	fn agent_id_unmatched_falls_back_to_latest_agent() {
		let sessions = vec![session("s1")];
		let agents = vec![agent("a1"), agent("a2")];
		let res =
			resolve_startup_state(true, None, Some("does-not-exist"), &sessions, &agents).unwrap();
		assert_eq!(
			res.view,
			Some(ResolvedView::Watching {
				session_id: "s1".to_string(),
				agent_id: "a1".to_string(),
			})
		);
	}

	#[test]
	fn session_prefix_wins_over_latest() {
		// 両方指定されたら session 優先 (TS 仕様と同じ)
		let sessions = vec![session("abc-1"), session("xyz-1")];
		let res = resolve_startup_state(true, Some("xyz"), None, &sessions, &[]).unwrap();
		assert_eq!(res.target_session_id.as_deref(), Some("xyz-1"));
	}
}
