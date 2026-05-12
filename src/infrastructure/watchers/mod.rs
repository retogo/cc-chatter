//! ファイル監視系 (chokidar 相当)。
//!
//! TS 版 `src/infrastructure/watchers/` の移植。
//! - [`sub_agent_watcher::SubAgentWatcher`]: 指定 agent の jsonl を監視し、
//!   差分行を `FormattedMessage` に変換して `Msg::MessagesAppended` を発火
//! - [`main_watcher::MainWatcher`]: セッションの `subagents/` ディレクトリを
//!   監視し、新規 agent-*.jsonl の出現を検知して `Msg::NewAgentAppeared` を発火
//!
//! いずれも `notify::PollWatcher` (100ms interval) を使う。chokidar の
//! usePolling 互換でシンボリックリンク経由でも動く。

pub mod main_watcher;
pub mod sub_agent_watcher;

pub use main_watcher::MainWatcher;
pub use sub_agent_watcher::SubAgentWatcher;
