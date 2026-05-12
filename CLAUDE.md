# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

cc-chatter は Claude Code のサブエージェント間のやりとりをリアルタイムで可視化する TUI ツール。Rust / Ratatui 実装。

仕様変更は必ず `docs/spec.md` (SSOT) を先に更新してから実装に反映する。

## コマンド

```bash
cargo build                                   # dev ビルド
cargo build --release                         # release バイナリ (target/release/cc-chatter)
cargo test                                    # 全テスト実行
cargo clippy --all-targets -- -D warnings     # lint
cargo fmt --check                             # format チェック
cargo fmt                                     # 自動 format
cargo run -- --latest                         # 実行
```

## アーキテクチャ

DDD（Domain-Driven Design）4層アーキテクチャを採用。詳細は `docs/design.md` を参照。

```
ui (Presentation) → application → domain ← infrastructure
```

## コーディング規約

- rustfmt のデフォルト設定 (タブインデント)
- `#![deny(warnings)]` は使わず、`cargo clippy -- -D warnings` で CI ブロック
- 実装変更後は `cargo test` / `cargo clippy --all-targets -- -D warnings` / `cargo fmt --check` が全て通ることを確認する

## 過去の失敗と学び

- 設計上の判断と理由は @docs/decisions.md に ADR スタイル (Decision + Why) で簡潔に記録する。
- 詳細な経緯は git log で追える前提なので、decisions.md には背景を長々書かない。
- 同じ失敗を繰り返さないための知見、起こりやすい off-by-one、event loop / 再描画の落とし穴などはここに集約する。

## ドキュメント

以下のドキュメントを参照すること。また、実装において修正が必要な場合は、必ず本ドキュメントおよび以下のドキュメントを更新すること。

- @./docs/design.md
- @./docs/ui-design.md
- @./docs/spec.md
- @./docs/decisions.md
