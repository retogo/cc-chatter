# cc-chatter

Claude Code のサブエージェント間やりとりをリアルタイム可視化する TUI ツール。Rust / Ratatui 製。

## インストール

```bash
# ワークツリーから直接インストール
cargo install --path . --locked
```

`~/.cargo/bin/cc-chatter` に実行可能ファイルが設置される (約 2.4 MB / release)。
`$PATH` に `~/.cargo/bin` が入っていない場合は加える。

アンインストールは `cargo uninstall cc-chatter`。

> 現状 `publish = false` なので `crates.io` からは入らない。

## 使い方

```bash
cc-chatter                                      # セッション選択 UI
cc-chatter -w .                                 # カレントディレクトリのワークスペースに絞る
cc-chatter --latest                             # 最新セッションに自動アタッチ
cc-chatter --latest --agent <agent-id>          # 特定エージェントにアタッチ
cc-chatter --session <prefix>                   # セッション ID 前方一致で自動選択
cc-chatter --limit 20                           # セッション一覧の件数制限
cc-chatter --show-all                           # 非表示エージェントも表示
cc-chatter --no-mouse                           # WATCHING のマウス reporting を無効化
```

キーバインド / マウス操作の詳細は [docs/ui-design.md](./docs/ui-design.md) と [docs/spec.md](./docs/spec.md)。

## 開発者向け

### コマンド

```bash
cargo build                                   # dev ビルド
cargo build --release                         # release バイナリ (target/release/cc-chatter)
cargo test                                    # 全テスト (unit + integration)
cargo test --test cli_options                 # CLI integration test のみ
cargo clippy --all-targets -- -D warnings     # lint
cargo fmt --check                             # format チェック
cargo fmt                                     # 自動 format
cargo run -- --latest                         # 実行
```

### デバッグログ

raw mode 下で stderr を出すと UI と混ざるので、`RUST_LOG` を明示したときだけ
`$TMPDIR/cc-chatter.log` にファイル出力する。

```bash
RUST_LOG=cc_chatter=debug cargo run --release -- --latest
# 別ターミナルで
tail -f "$TMPDIR/cc-chatter.log"                # macOS: /var/folders/.../T/
```

### パフォーマンス計測

```bash
cargo run --release --example bench_watching
```

1000 メッセージを WATCHING に流し込んで 100 フレーム回す合成ベンチ。
`terminal.draw` の elapsed をログに吐く。

### ディレクトリ構成

```
src/
├── main.rs           # エントリポイント (tokio runtime + panic/signal hook + TerminalGuard)
├── lib.rs            # pub mod 宣言
├── cli.rs            # clap derive: CliOptions
├── event_loop.rs     # tokio::select! で Key/Mouse/File/Tick を Msg に集約
├── domain/           # 純粋な型とビジネスルール (外部 I/O ゼロ)
├── infrastructure/   # ファイル I/O / JSONL パース / notify ベース watcher / TerminalGuard
├── application/      # ユースケース / 状態遷移 (Elm アーキテクチャ相当)
└── ui/               # ratatui による描画 (view + screens + layout)
tests/                # CLI integration test
examples/             # perf ベンチ
```

詳細な設計は [docs/](./docs/) を参照。

## ライセンス

MIT。詳細は [LICENSE](./LICENSE) を参照。
