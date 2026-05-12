# cc-chatter Design

## 技術スタック

| ライブラリ | 用途 |
|------------|------|
| ratatui | TUI レンダリング (immediate mode) |
| crossterm | キー/マウス/画面操作 (EventStream + raw mode) |
| tokio | 非同期ランタイム / event loop |
| notify | ファイル監視 |
| clap (derive) | CLI 引数 |
| serde / serde_json | JSONL パース |
| chrono | タイムスタンプ |
| tracing | 構造化ログ (`RUST_LOG` でファイル出力) |
| color-eyre / signal-hook | panic / signal 時の端末復旧 |

## アーキテクチャ

DDD（Domain-Driven Design）4層アーキテクチャ。依存方向は外側→内側のみ。
状態遷移は Elm 流の `Model` / `Msg` / `update` (reducer) パターン。

```
ui (Presentation) → application → domain ← infrastructure
```

## フォルダ構成

```
src/
├── main.rs                            # エントリポイント (tokio runtime + panic/signal hook + TerminalGuard)
├── lib.rs                             # pub mod 宣言
├── cli.rs                             # clap derive: CliOptions
├── event_loop.rs                      # tokio::select! で Key/Mouse/File/Tick を Msg に集約
│
├── settings.rs                        # ChatMode (default / line / slack) — UI 表示モードの runtime state
│
├── domain/                            # ドメイン層（純粋な型・ルール・外部 I/O ゼロ）
│   ├── entities/
│   │   ├── session.rs                 # SessionEntity, SessionMetadata
│   │   ├── agent.rs                   # AgentEntity, AgentType, AgentMapping
│   │   └── log_entry.rs               # ログエントリ型, FormattedMessage
│   ├── services/
│   │   └── session_filter.rs          # セッション表示テキスト / ローカルコマンド除外
│   └── constants.rs                   # HIDDEN_AGENT_PREFIXES, MAX_MESSAGES 等
│
├── infrastructure/                    # インフラ層（外部依存の実装）
│   ├── parsers/
│   │   └── jsonl_parser.rs            # JSONL 差分読込（バイトオフセット追跡）
│   ├── repositories/
│   │   ├── fs_session_repo.rs         # FileSystemSessionRepository
│   │   └── agent_mapper_impl.rs       # マッピング構築 (.meta.json 優先 + mapper fallback)
│   ├── input/
│   │   └── terminal_guard.rs          # raw mode / mouse reporting の有効化と Drop / panic / signal での復旧
│   └── watchers/
│       ├── main_watcher.rs            # notify ベースのメインログ監視
│       └── sub_agent_watcher.rs       # サブエージェント .jsonl 監視
│
├── application/                       # アプリケーション層（ユースケース + 状態遷移）
│   ├── app.rs                         # App: Model + update (Msg -> State 変更)
│   ├── msg.rs                         # Msg 列挙 (Key/Mouse/File/Tick/Cmd 応答)
│   ├── state/
│   │   ├── app.rs                     # 画面遷移 (SESSION_SELECT → ... → WATCHING)
│   │   ├── domain.rs                  # sessions / agents / messages
│   │   ├── viewport.rs                # WATCHING の selection / scrollOffsetRows / expandedIds / followTail
│   │   └── heights_cache.rs           # ChatBubble の (preview, expanded) 高さキャッシュ
│   ├── mappers/
│   │   └── message_mapper.rs          # SubAgentLogEntry → Vec<MapperOutcome> (Append | AttachResult)
│   └── usecases/
│       └── startup_skip.rs            # --latest/--session/--agent による画面スキップ
│
└── ui/                                # プレゼンテーション層（描画のみ）
    ├── view.rs                        # 画面ディスパッチ (App.view → current screen)
    ├── screens/
    │   ├── session_select.rs
    │   ├── subagent_select.rs
    │   ├── waiting.rs
    │   └── watching.rs
    ├── components/
    │   └── chat_bubble.rs             # tool_use + tool_result 統合バブル / mention prefix / 薄色化
    ├── layout/
    │   └── viewport_layout.rs         # 高さ見積もり (wrap_line_count_*) + 行ベース viewport
    ├── format.rs                      # format_date_time / format_tool_preview / format_result_preview
    └── icons.rs                       # エージェントタイプ別アイコン
```

## データフロー

```
┌─────────────────────────────────────────────────────────────┐
│  Claude Code (メインエージェント)                              │
│  ~/.claude/projects/{workspace}/{sessionId}.jsonl           │
└────────┬────────────────────────────────────────────────────┘
         │ Agent tool call (subagent_type, tool_use_id)
         │ ※ 旧バージョンでは Task tool name も後方互換で受け付ける
         ▼
┌─────────────────────────────────────────────────────────────┐
│  Subagent (サブエージェント)                                   │
│  ~/.claude/projects/{workspace}/{sessionId}/subagents/      │
│  agent-{agentId}.jsonl                                      │
│  agent-{agentId}.meta.json  ← agent_type の権威ソース        │
└────────┬────────────────────────────────────────────────────┘
         │ notify (ファイル監視)
         ▼
┌─────────────────────────────────────────────────────────────┐
│  cc-chatter                                                   │
│  1. main_watcher がメインログからマッピング更新を通知          │
│  2. sub_agent_watcher がサブエージェントログの追記を通知       │
│  3. JsonlParser が差分読込（バイトオフセット追跡）             │
│  4. MessageMapper が Vec<MapperOutcome> を生成                │
│  5. event_loop が各種イベントを Msg に畳み込む                 │
│  6. App::update が Msg を受けて State を変更 + dirty flag     │
│  7. ui::view がダーティ時だけ terminal.draw を呼ぶ             │
└─────────────────────────────────────────────────────────────┘
```

## Claude Code ログのファイル構造

```
~/.claude/projects/{workspace}/
├── {sessionId}.jsonl                    ← メインエージェントの会話ログ
└── {sessionId}/subagents/
    ├── agent-{agentId}.jsonl            ← サブエージェントの会話ログ（実体）
    └── agent-{agentId}.meta.json        ← agent_type / description のサイドカー
```

## 各層の詳細

### Domain 層

外部依存なし。純粋な型定義とビジネスルール。

- **entities/**: `SessionEntity`, `AgentEntity`, `FormattedMessage` 等のドメインモデル
  - **`FormattedMessage.is_final_response: bool`**: Sub エージェントが
    `stop_reason == "end_turn"` で停止した最後の text item のみ true。UI 層は
    この値で `@Main` mention 付与 / 中間 text 薄色化を分岐する。`AssistantMessage`
    (`log_entry.rs`) に `stop_reason: Option<String>` を `#[serde(default)]`
    で追加し、null / 省略を `Option::None` に吸収する (Claude Code のログ
    フォーマット差異への耐性)
- **services/**: `session_filter` — セッション表示テキストの優先順位 (summary →
  firstPrompt → "(新規セッション)") とローカルコマンドの除外ロジック
- **constants.rs**: `HIDDEN_AGENT_PREFIXES`, `LOCAL_COMMAND_PREFIXES`, `MAX_MESSAGES` 等

### Infrastructure 層

ファイルシステムやライブラリへの依存を隠蔽。

- **JsonlParser**: バイトオフセット追跡による差分読込 (追記型 JSONL の追加行だけ
  読む)
- **FileSystemSessionRepository**: `~/.claude/projects/` を走査してセッション
  一覧を取得
- **AgentMapperImpl**: マッピング構築
  - 通常: assistant の tool_use → pending、progress の agentId → 確定
  - Background agent: assistant の tool_use → pending、user の async tool_result → 確定
  - セッション選択時はメインログ末尾から部分読み＋倍々拡大で構築
  - subagent 起動ツール名は `"Agent"`（現行 Claude Code）と `"Task"`（旧バージョン）の両方を受理
  - `subagent_type` 未指定（null / undefined）の場合は `"general-purpose"` にフォールバック（Claude Code のデフォルト挙動と同じ）
- **`.meta.json` サイドカー読込**（現行 Claude Code の公式マッピングソース）
  - Claude Code は各 `agent-<id>.jsonl` の隣に `agent-<id>.meta.json` を書き、
    `{ "agentType": "...", "description"?: "..." }` の形で agent_type を持つ
  - 現行版では `progress` / async `tool_result` エントリは一切出ない。サイドカーが
    事実上唯一の権威あるソース
  - `FileSystemSessionRepository::get_sub_agents` はまずサイドカーを読み、
    取れなかった agent だけメインログ経由の `AgentMapperImpl` にフォールバックする
    （古いログ互換のため mapper ルートは残す）
- **main_watcher / sub_agent_watcher**: notify ベース。watcher スレッド内で
  `Msg` を channel に流し、event_loop で `tokio::select!` に畳み込む
- **TerminalGuard (`infrastructure/input/terminal_guard.rs`)**: raw mode と SGR
  mouse reporting (`\x1b[?1002h\x1b[?1006h`) を `new()` で有効化し、`Drop` で
  必ず解除する RAII ラッパー。さらに `install_panic_hook` と
  `install_signal_hook` を main.rs で張ることで、panic / SIGINT / SIGTERM 時も
  必ず解除される三重化構造

### Application 層

Elm 流の Model + Msg + update パターン。

- **`App`** (`application/app.rs`): Model + update 関数。`update(&mut self, Msg)`
  が Msg に応じて State を変更し、必要なら `needs_redraw` フラグを立てる
- **`Msg`** (`application/msg.rs`): Key/Mouse/Resize/Tick/FileEvent/Cmd 応答等を
  統一する列挙型
- **state/app.rs**: 画面遷移（SESSION_SELECT → SUBAGENT_SELECT → WAITING/WATCHING）。
  WATCHING の attach 対象は `attached_agent_ids: Vec<String>` で複数 agent を
  保持できる（multi-attach）。`selected_agent_ids: HashSet<String>` は
  SUBAGENT_SELECT 画面で Space トグルされた選択候補
- **state/domain.rs**: `sessions`, `agents`, `messages` のビジネスデータ
- **state/viewport.rs**: WATCHING 画面の `selected_index` / `scroll_offset_rows` /
  `expanded_ids` / `follow_tail` / `pending_follow_selection` を管理
- **state/heights_cache.rs**: ChatBubble の `(preview_h, expanded_h)` を
  `content_width` / `chat_mode` 変化時に全件計算、追加メッセージは sync で
  追記、AttachResult で mutate された index は `invalidate` で部分再計算。
  `chat_mode` は WATCHING 画面の `m` キーで runtime に切り替わるため、
  `sync` が `chat_mode` 変化を検出して全件再計算する
- **MessageMapper** (`application/mappers/message_mapper.rs`):
  - `SubAgentLogEntry → Vec<MapperOutcome>` の **projection**
    (Slack スレッド風の集約のため)
    - `Append(FormattedMessage)`: text / tool_use 単独 → 末尾追加
    - `AttachResult { tool_use_id, content, is_error, timestamp }`: 既存メッセージ
      の `tool_result` を埋める指示
  - mapper は per-entry pure function (input → output が一意)
  - pairing の適用 (既存メッセージ列の mutate) はアプリ層 (`application::app`)
    の責務。Mapper は append-only のイベントソース、`FormattedMessage` 列は
    その projection (event sourcing パターン)
  - **`is_final_response: bool` 判定**:
    `AssistantMessage.stop_reason == "end_turn"` の entry に限り、**最後の
    text item のみ** `is_final_response = true` とする。中間 text
    (`stop_reason == null` / `"tool_use"` / その他) は常に false = Main に
    届かない独り言扱い。Main からの user prompt (`Value::String` の user
    message) は mention 判定に stop_reason を使わないため常に false。
    UI 層はこのフラグで `@Main` mention prefix 付与 / 中間 text 薄色化を
    分岐する (spec.md「サブエージェント監視」の mention 仕様を参照)
- **アプリ層の pairing 適用**:
  - `Append` → messages に push
  - `AttachResult` → batch 内の append 済みメッセージを rev で探索 → 無ければ
    既存 messages を逆順 linear search し、同じ `tool_use_id` を持つメッセージの
    `tool_result` / `result_timestamp` を mutate する
  - mutate した index は `HeightsCache::invalidate(idx, msg, content_width)` で
    部分再計算 + `mark_dirty()` で再描画要求
  - 一致する tool_use が見つからない orphan tool_result は独立メッセージとして
    fallback push (先頭 drain 済み / 欠損ログ対策)
- `id` フィールド: `{timestamp_ms}-{uuid}-{idx}` 形式。orphan tool_result
  だけ `orphan-{tool_use_id}-{ms}` で採番
- **startup_skip**: `--latest`/`--session`/`--agent` オプションによる画面
  スキップ処理

### UI 層

描画のみ。Model (`App`) を受け取って ratatui で描く。状態管理はしない。

- **`event_loop.rs`**: `tokio::select! { biased; crossterm_events → cmd_rx → tick }`
  で優先順位付きに Msg を取り出し、`App::update` を呼ぶ。watcher からの
  バースト (100+ event) は `drain_ready_messages` で `cmd_rx` 側だけ batch
  drain。user input (crossterm) は逐次処理してスムーズスクロールを維持。
  Tick interval は **100ms**。スピナーの 1 フレームに合わせた粒度で、未完了
  tool_use がない間は `App::update` が `mark_dirty()` を呼ばないので
  描画コストはゼロ
- **`ui::view`**: `App::current_view` で各 screen に dispatch。dirty flag が
  立っているフレームだけ `terminal.draw` を呼ぶ (Tick のみで無変化なら skip)
- **スピナー dirty 制御**: WATCHING 描画中に `viewport_cache.has_inflight_in_view`
  を集計 (slice 範囲内に `tool_use=Some && tool_result=None` が 1 件でもあれば
  true)。`App::update(Msg::Tick)` はこのフラグが true のときだけ
  `spinner_phase = wrapping_add(1)` + `mark_dirty()` する。viewport の外に
  流れた未完了 bubble や、すべて完了済みのケースでは Tick で何も起きない
- **`ChatBubble`** (`ui::components::chat_bubble`): tool_use + tool_result 統合
  バブル描画。mention prefix / 薄色化 / 詳細モード展開の制御。
  `render_bubble_rows(model, content_width, chat_mode)` が `BubbleRow`
  (`TopBorder` / `Body` / `BottomBorder` / `Margin`) の列を返し、
  `draw_bubble_row` がモードに応じて寄せ (Main=右 or Sub=左 / slack=全左) と
  枠線 (`line` は 4 辺、他は左 1 文字のみ) を描き分ける。末尾の `Margin` は
  バブル間スペーサーで、どのモードでも何も描かない (LINE で `╰──╯` の下に
  `│ │` が残るバグ対策)。
  - **`compute_bubble_width(area_width, chat_mode)`** で吹き出し幅を分岐:
    slack のみ端末幅 100%、default/line は 60% (最低 30)
  - **`compute_content_width(area_width, chat_mode)`** で `render_bubble_rows`
    / `estimate_bubble_height_full` / `HeightsCache::sync` に渡す `content_width`
    を一箇所で算出。default/slack は `bubble_width - 1` (左 `▏` マーカー 1 セル分
    を差し引く)、LINE は `bubble_width` そのもの (4 辺枠線は bubble 全幅で描く
    ため)。これが揃わないと右枠 `╯` と本文の右 `│` が 1 セルずれる off-by-one
    を生むため、watching.rs / `App::current_content_width` / cache 3 経路を
    同じ helper に集約する
  - `chat_mode` は `App::chat_mode` (`settings::ChatMode`) を runtime に保持し、
    WATCHING 画面の `m` キーで `default → line → slack` を循環切替する
    (`App::set_chat_mode` 経由で更新 + `mark_dirty`)
- **WATCHING 画面 (`ui::screens::watching`)**: `compute_row_based_viewport` で
  端末高さに収まる範囲にスライスした行だけを描画する。スクロール位置は **行ベース**
  (`scroll_offset_rows`) で管理し、ホイール 1 tick = 1 行で動かす
  (`WHEEL_DELTA_ROWS` は TUI で可能な最小粒度)。
  - 選択とスクロールは独立。選択は reducer で動かす、ホイールは
    `scroll_offset_rows` のみ動かす
  - `pending_follow_selection` フラグが立ったフレームだけ、描画層が
    「選択が画面外なら offset を寄せる」auto-follow を発火させる
  - ratatui の `Buffer::set_line` 直書きで描画 (Paragraph::wrap は使わない)。
    高さ見積もり (`wrap_line_count_with_prefix`) と描画
    (`wrap_lines_iter_with_prefix`) で **同じ折り返しロジック** を共有し、
    「見積もり == 実描画行数」不変条件を保つ
- **viewport_layout** (`ui::layout::viewport_layout`): `wrap_string_lines` /
  `wrap_line_count_*` / `compute_row_based_viewport` を提供する純粋関数群。
  ChatBubble の折り返し後行数を事前計算し、行ベースで slice を決める。
  mention prefix がある場合は 1 行目の折り返し幅だけ縮むので
  `*_with_prefix` 系を使う
