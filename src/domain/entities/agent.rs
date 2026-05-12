//! Agent entity。TS 版 `src/domain/entities/Agent.ts` の移植。

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// エージェントタイプ。
///
/// TS 版は built-in の union を持っていたが、Rust では文字列のまま扱う。
/// built-in チェックは `ui/icons.rs` 側の map lookup で行う。
pub type AgentType = String;

/// サブエージェント 1 体の中核情報。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntity {
	/// エージェント ID (例: `af1bd66`)。
	pub agent_id: String,

	/// エージェントタイプ (`Explore` / `general-purpose` / カスタム等)。
	#[serde(rename = "type")]
	pub agent_type: AgentType,

	/// 出力ファイルのパス (シンボリックリンク)。
	pub output_path: PathBuf,

	/// ファイルの mtime。
	pub updated_at: DateTime<Utc>,
}

/// `agent_id` → `agent_type` のマッピング情報。
///
/// メインログの Agent / Task tool call から構築される。詳細は
/// `AgentMapperImpl` 相当 (M1 後半で移植) を参照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMapping {
	/// エージェント ID。
	pub agent_id: String,

	/// サブエージェントタイプ。未指定の場合は `general-purpose` が埋められる。
	pub subagent_type: AgentType,

	/// Agent / Task tool call の tool_use id。
	pub tool_use_id: String,

	/// Agent / Task tool call の description (3〜5 語の短い説明)。
	pub description: Option<String>,

	/// 使用モデル (短縮形: `sonnet` / `opus` / `haiku` 等)。
	pub model: Option<String>,
}
