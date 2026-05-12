//! # cc-chatter (Rust 版)
//!
//! Claude Code のサブエージェント間やりとりをリアルタイム可視化する TUI。
//!
//! 階層は TS 版と同じ DDD 4 層を踏襲する:
//! - [`domain`]: 純粋な型とビジネスルール。外部 I/O 依存ゼロ
//! - [`infrastructure`]: ファイル I/O / JSONL パース / 監視などの具象実装
//! - [`application`]: ユースケース / 状態遷移 (Elm アーキテクチャ相当)
//! - [`ui`]: ratatui で描画するプレゼンテーション層 (M2 以降)
//!
//! 依存方向は外側 → 内側のみ。`domain` は他層を参照しない。

pub mod application;
pub mod cli;
pub mod domain;
pub mod event_loop;
pub mod infrastructure;
pub mod settings;
pub mod ui;
