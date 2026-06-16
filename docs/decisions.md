# Decisions

> cc-chatter の設計上の判断と理由の索引。各項目は「決定」+「なぜ」の最小情報に留める。
> 詳細な経緯は git log を、現在の実装は `docs/design.md` を参照。

## 外部スキーマとの境界

- **外部プロトコル literal は union + fallback で受ける**
  Claude Code のログスキーマは黙って変わる (`tool_use.name` が `Task` → `Agent`、
  `progress` / async `tool_result` が消える等)。`#[serde(default)] + Option<T>`
  で省略・未知値を吸収し、新→旧の順で fallback する。
- **UI 層は外部 literal を直接判定しない**
  `stop_reason == "end_turn"` のような判定は Mapper で `is_final_response: bool`
  に bool 化してから UI に渡す。スキーマ変更が UI まで伝播するのを防ぐ。
- **Workflow agent は `subagents/workflows/<wf_runId>/` に 1 階層深く出る**
  Workflow ツールの `agent()` が吐くログは通常の `subagents/agent-*.jsonl` では
  なく nested。`get_sub_agents` / `count_subagents` を 1 階層 walk して統合し、
  meta は各 agent の自ディレクトリ基準で解決する。main_watcher は再帰監視に
  切り替えて nested の新規追加を検知する (NonRecursive だと取りこぼす)。
- **`agent()` の label は永続化されない → prompt 先頭から導出**
  `journal.jsonl` は agentId とハッシュ key の対応しか持たず、`review:bugs` の
  ような人間可読 label は残らない。generic な `workflow-subagent` は先頭 prompt
  行から短いロール label を導出して代替する (カスタム型はそのまま型名を使う)。

## TUI スクロールと選択

- **スクロールは行ベース** (メッセージ単位だと高さ可変でカクつく)。
  ホイール 1 tick = 1 行が TUI の最小粒度。
- **clamp は描画層の責任**。reducer は素朴な値を保持し、描画層が
  `effective_offset` を算出して書き戻す。reducer 側で同値ガードして dirty
  loop を防ぐ。
- **選択 → offset の追従は 1 方向のみ**。逆向きを足すと必ず loop する。
- **「次のフレームだけ有効なフラグ」で意図区分**
  キー/クリックは `pending_follow_selection` を立てて選択追従を 1 回だけ発火、
  ホイールは触らない。描画層 1 箇所で consume する。

## 再描画とイベント処理

- **dirty flag は handler 内で state が変わったときだけ立てる**
  Msg 種別での early-dirty は粗すぎて毎フレーム描画になる。Mouse Moved /
  watcher 系は除外必須 (SGR で秒間数十件届く)。
- **`tokio::select!` は `biased`**: 対話 app は input 優先
  (`crossterm → cmd_rx → tick`)。
- **batch drain は watcher だけ**。crossterm event を drain するとスクロールが
  ジャンプする。
- **見積もりキャッシュは 3 軸だけ無効化**: content_width 変化 / 先頭 drain /
  AttachResult mutate。トグル系 (`is_detailed_mode` / `expanded_ids`) は
  preview/expanded 両方を事前計算しておけば無効化不要。

## レイアウトの整合性

- **「見積もり == 実描画行数」を invariant にする**
  別関数 (数える / 文字列を返す) に分離し、回帰テストで出力一致を担保。
  共有して single source of truth にすると描画向け alloc が hot path に乗る。
- **モード差は単一 helper で吸収**
  例: `compute_content_width(area_width, chat_mode)`。呼び出し側で式を書かせない。
  「同名変数がモードで別物を指す」は必ず off-by-one を生む。
- **closed / open でバブル形状が変わる場合、見積もりと描画の両方に分岐を入れる**
  片方だけだと follow_tail / hit test が壊れる。

## 状態モデル

- **Mapper は per-entry pure function、pairing 適用はアプリ層**
  append-only ログを UI 列に projection する event sourcing 構造。AttachResult
  は batch → 既存 messages 逆順 → orphan の順で探索する。
- **「自動展開」と「ユーザー操作」を意図レベルで区別**
  `auto_open_bash_ids` (最新 1 件を track) と `user_toggled_ids` (個別操作のみ
  記録) を分離。Ctrl+O の全体トグルは `user_toggled_ids` に入れない。マルチ
  key は `HashMap<key, id>` で独立 track する。

## ターミナル復旧

- **TerminalGuard (RAII) + panic_hook + signal_hook の三重化**
  SGR mouse reporting は終了時に必ず解除しないとシェルが壊れる。
  Drop / panic / SIGINT/SIGTERM の全経路で解除する。

## ユーティリティ

- **時刻表示は `format_date_time` ヘルパー必須**
  `DateTime<Utc>::format()` は UTC のまま整形される。Local 変換を helper に
  集約する。

## 調査と検証のメタ原則

- **入力が期待通り流れているかを実ログで先に見る**
  「コードは正しい」と錯覚しやすい。
  `find ~/.claude/projects -name "*.jsonl" | xargs grep -c <pattern>` で先に確認。
- **bench と実ログは別物**。trackpad 1 フリック = 40+ event burst のような
  実入力パターンは bench で再現できない。`RUST_LOG=cc_chatter=debug` で取り、
  Msg 種別ごとの頻度と dirty 比率を `sort | uniq -c` で集計する。
- **回帰テストは実機症状をそのまま書く**。Buffer の x 座標で `assert_eq!` する
  (「`╰──╯` の次の行に `│` が無い」「右 `│` と `╯` の x が同じ」など)。
- **多角的に調査する** (Agent Team)。1 視点だと目立つ箇所に焦点が集まり、
  独立した原因 (event loop / 再描画トリガ / hot path) を見逃す。

## TS / ink 版の下敷き (現行 Rust 版では直接再現しない)

- **ライブコンテンツと scrollback の分離**: viewport スライシングは perf 上も
  必須 (フレーム計算負荷)。
- **「見積もり == 実描画行数」invariant** は ink 時代から続く設計知見。
