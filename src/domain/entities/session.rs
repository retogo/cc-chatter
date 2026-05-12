//! Session entity。TS 版 `src/domain/entities/Session.ts` の移植。

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// セッション 1 件の中核情報。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntity {
	/// セッション ID (JSONL ファイル名から拡張子を除いたもの)。
	pub session_id: String,

	/// ワークスペース識別子 (例: `-Users-username-project`)。
	/// 絶対パス中の `/`, `.`, `_` を `-` に置換したエンコード後の文字列。
	pub workspace: String,

	/// セッション JSONL のフルパス。
	pub file_path: PathBuf,

	/// 最終更新日時。
	/// 算出は「最後の明示的ユーザーアクションの timestamp」→ 該当無しの場合は
	/// ファイルの mtime にフォールバック。詳細は `SessionFilterService` 相当で実装。
	pub updated_at: DateTime<Utc>,

	/// サブエージェントディレクトリのパス (`{session_id}/subagents/`)。
	pub subagents_dir: PathBuf,

	/// `subagents_dir` 配下の `agent-*.jsonl` 件数。`--show-all` 連動で
	/// HIDDEN プレフィックスを除いた件数 (既定) または全件 (show-all 時) を持つ。
	pub subagent_count: usize,

	/// 表示・ソートに使うメタデータ。
	pub metadata: SessionMetadata,
}

/// セッションの表示用メタデータ。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMetadata {
	/// 最初のユーザープロンプト (先頭 60 文字まで)。
	pub first_prompt: String,

	/// JSONL の `type: "summary"` エントリから取得したサマリー。
	pub summary: Option<String>,

	/// 最後のユーザー明示的アクション日時。`updated_at` の算出に使う。
	pub last_user_action_at: Option<DateTime<Utc>>,
}
