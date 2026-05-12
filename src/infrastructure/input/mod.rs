//! 入力デバイス関連 (terminal / mouse)。
//!
//! TS 版 `src/infrastructure/input/MouseReporter.ts` + ink の setRawMode
//! 管理に相当する機能を `crossterm` + ratatui の初期化に集約する。

pub mod terminal_guard;

pub use terminal_guard::TerminalGuard;
