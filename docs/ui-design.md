# cc-chatter UI Design

## コンポーネント構成

```
src/ui/
├── view.rs                       # 画面ディスパッチ (App::current_view → screen 関数)
├── screens/                      # 画面ごとの render 関数
│   ├── session_select.rs
│   ├── subagent_select.rs
│   ├── waiting.rs
│   └── watching.rs
├── components/
│   └── chat_bubble.rs            # ChatBubble 描画 (tool_use+tool_result 統合 / mention / 薄色化)
├── layout/
│   └── viewport_layout.rs        # 高さ見積もり + 行ベース viewport + hit test
├── format.rs                     # format_date_time / format_tool_preview / format_result_preview
└── icons.rs                      # エージェントタイプ別アイコン
```

## 状態機械（`App` + `Msg`）

Elm 流の Model + Msg + update パターン。`src/application/app.rs` の `App` が
Model (state) を保持し、`update(&mut self, Msg)` が Msg に応じて state を変更。
ui 層は `App` を引数で受け取り、`current_view` に応じた screen の `render(frame, area, &app)` を呼ぶ。

```
SESSION_SELECT ⇄ SUBAGENT_SELECT ⇄ WATCHING
                      ⇅                ↓
                   WAITING ─────────→ WATCHING
```

### state の分割

| モジュール | 責務 |
|------------|------|
| `state::app` | 画面遷移（currentView, selection, cursorIndex, isDetailedMode） |
| `state::domain` | ビジネスデータ（sessions, agents, messages） |
| `state::viewport` | WATCHING 画面の選択・スクロール・展開・末尾追従状態 |
| `state::heights_cache` | ChatBubble の (preview_h, expanded_h) キャッシュ |

### アプリレベルの Msg

| Msg | 説明 |
|-----|------|
| `SessionSelected` | セッション選択完了 |
| `SubAgentSelected` | サブエージェント選択完了（単数 / 複数共通。複数なら multi-attach） |
| `ToggleAgentSelection` | SUBAGENT_SELECT で `Space` キーが押されたとき、対象 agent_id の `selected_agent_ids` (HashSet) をトグル |
| `WaitRequested` | 新規エージェント待機開始 |
| `Attached` | エージェントへのアタッチ成功 |
| `NoExistingAgents` | 既存エージェントなし（自動遷移） |
| `Back` | 戻り操作 |
| `ToggleDetailMode` | Ctrl+O による詳細表示モード切替（全体） |
| `CursorMove` | カーソル移動 |
| `SessionsLoaded` / `AgentsLoaded` | repository 応答 |
| `MessagesAppended { outcomes }` | サブエージェントログの追記通知 |
| `Tick` | 250ms 周期のタイマー（dirty が立っていないフレームは描画 skip） |
| `Resize` / `Key` / `Mouse` | crossterm イベントラップ |

### Viewport Msg

WATCHING 画面専用。`App` は `messages` の長さ変化を監視して、初期状態に
限り末尾吸着を行う（`on_messages_appended`）。それ以外はキー/マウス入力から
直接 dispatch。

| Msg | 説明 |
|-----|------|
| `MoveUp` / `MoveDown` | 選択を 1 つ上/下に移動。`scroll_offset_rows` は reducer では触らず、描画層が選択追従する |
| `PageUp` / `PageDown` | 半ページ（`pageSize/2`）上/下 |
| `JumpTop` / `JumpBottom` | 先頭/末尾にジャンプ |
| `ScrollByRows { row_delta }` | 行ベーススクロール（マウスホイール用）。`scroll_offset_rows` を加算。選択は動かさない。`follow_tail` は上方向のみ false |
| `SetScrollOffset { offset, follow_tail_reached }` | 描画層からの clamp 済み値書き戻し |
| `ToggleExpand` | 選択中メッセージの展開/折りたたみをトグル |
| `SelectById { id }` | 指定 id のメッセージを選択（マウスクリック用） |
| `SelectOrToggleById { id }` | 未選択なら SELECT、既選択なら TOGGLE_EXPAND（クリック → 再クリック） |
| `MessagesAppended` | 新着通知。`selected_index=None` かつ `follow_tail=true` のときだけ末尾に吸着 |
| `Reset` | アタッチ切替等で viewport を初期化 |

### 状態と遷移

| 状態 | 説明 |
|------|------|
| SESSION_SELECT | セッション選択中 |
| SUBAGENT_SELECT | 既存サブエージェントがいる場合の選択 |
| WAITING | 新規サブエージェント起動待ち |
| WATCHING | 監視中（チャット表示更新） |

### 戻り遷移（Escape キー）

| 現在の状態 | Esc 押下時 | 必要な処理 |
|-----------|-----------|----------|
| SESSION_SELECT | なし | 最初の画面なので戻る場所がない |
| SUBAGENT_SELECT | SESSION_SELECT へ | セッション選択解除 |
| WAITING | SESSION_SELECT へ | watcher detach + セッション選択解除 |
| WATCHING | SUBAGENT_SELECT へ | watcher detach |

※ WAITING から SUBAGENT_SELECT に戻っても、サブエージェントがいない場合は再び WAITING に自動遷移するため、SESSION_SELECT に直接戻る。

### リスト更新（Ctrl+R）

| 現在の状態 | Ctrl+R 押下時 | 処理 |
|-----------|--------------|------|
| SESSION_SELECT | セッション一覧を再取得 | `refresh_sessions` |
| SUBAGENT_SELECT | エージェント一覧を再取得 | `refresh_sub_agents` |
| WAITING | なし | 自動監視中のため不要 |
| WATCHING | なし | 自動監視中のため不要 |

### 詳細モード切替（Ctrl+O）

全画面で有効。WATCHING 画面でのみ効果あり。

| モード | ツール呼び出し表示 | ツール結果表示 |
|--------|-------------------|---------------|
| 通常 | ツール名 + 代表引数プレビュー（2 行） | ラベル + 結果 1 行プレビュー（2 行） |
| 詳細 | ツール名 + 引数全文 | 結果全文 |

`Enter` / `Space` で**選択中メッセージのみ**を個別に展開/折りたたみできる
（`expanded_ids: HashSet<String>` で管理）。両者を併用した場合、メッセージは
`is_detailed_mode || expanded_ids.contains(msg.id)` の条件で展開表示される。
heights_cache は両方のパターン (`preview_h` / `expanded_h`) を事前計算している
ので、モード切替時も O(1) lookup で済む。

### WATCHING 画面の viewport

- `compute_row_based_viewport` が `terminal_rows - HEADER_LINES - FOOTER_LINES
  - SAFETY_MARGIN` に収まる範囲を決定し、画面内に映る分しか描画しない
  （レイアウト計算コストを N 件全件にかけない）。
- `estimate_bubble_height` で各メッセージの推定高さを算出し、累積行高さから
  行ベースで slice する。メッセージ単位で動かすと高さが可変で 1 tick の
  移動量がばらつきカクつくため、スクロール位置は行単位で持つ。
- `▲ more above` / `▼ more below` インジケータは表示しない。位置情報は
  ヘッダーの `[X/N]` で把握できるので、不要な視覚ノイズを避ける。
- **選択とスクロールは独立**: `selected_index` は選択カーソル位置、
  `scroll_offset_rows` は画面先頭からの累積行オフセット（clamp 前）。
  キーボード（↑↓/jk/g/G/Ctrl+D/U）は選択を動かすだけで、描画層の
  `compute_row_based_viewport` が「選択が画面外なら offset を寄せる」ロジックで
  自動追従する。マウスホイール（`ScrollByRows`）は `scroll_offset_rows` のみ
  動かし、選択は据え置く。ホイール 1 tick = 1 行（`WHEEL_DELTA_ROWS`）。
  TUI で可能な最小粒度。ゆっくり回せば細かく、勢いよく回せば端末が複数 tick
  を連続で送るので自然に速くなる。
- **clamp は描画層の責任**: reducer は clamp 前の素朴な値を保持し、描画層で
  `compute_row_based_viewport` が `[0, max_offset]` に clamp + 選択追従 + 末尾
  追従を適用した `effective_offset` を算出する。state と乖離していれば
  `SetScrollOffset` を dispatch して書き戻す（reducer 側で同値チェックして
  いるので dirty loop しない）。
- **`pending_follow_selection: bool` フラグ**: 選択を動かす Msg
  (`MoveUp/Down`, `PageUp/Down`, `JumpTop/Bottom`, `SelectById`, 新規選択の
  `SelectOrToggleById`) で true にする。描画層が毎フレーム 1 回だけ
  `consume_pending_follow_selection()` で読み出し、その結果を
  `auto_follow_selection` に渡す。キー/クリック直後の 1 フレームだけ選択追従が
  有効になり、ホイール操作時は reducer 側で `scroll_offset_rows` がそのまま
  保たれる（loop 回避）。
- 末尾追従モード (`follow_tail`): `effective_offset` が `max_offset` に達して
  いるとき true。ホイールで上に動かすと false、末尾まで戻すと true に復帰。
  新着メッセージ到着時は true のときだけ描画層が `scroll_offset_rows` を末尾に
  寄せる（選択は動かさない。ただし初期 `selected_index=None` のときは互換のため
  選択も末尾に吸着する）。
- 先頭/末尾のメッセージが viewport の上下に数行はみ出るケースはある
  （行単位クリップは未対応。コスト対効果の妥協）。スクロール体感を損なわない
  程度のはみ出しに抑えるため、safety margin で余裕を持たせる。
- crossterm の `Resize` イベントで `content_width` が変わると heights_cache は
  全件再計算される。

### WATCHING 画面のキーバインド

| キー | 動作 |
|------|------|
| ↑ / k | 選択を 1 つ上へ |
| ↓ / j | 選択を 1 つ下へ |
| Enter / Space | 選択中メッセージの開閉トグル |
| g | 先頭へジャンプ |
| G | 末尾へジャンプ（末尾追従 ON） |
| Ctrl+D | 半ページ下スクロール |
| Ctrl+U | 半ページ上スクロール |
| Ctrl+O | 全体の詳細モードをトグル |
| m | ChatBubble 表示モードを `default → line → slack` で循環切替 |
| Esc | SUBAGENT_SELECT に戻る |
| Ctrl+C | 終了 |

### WATCHING 画面のマウス操作

SGR mouse reporting (`\x1b[?1006h\x1b[?1002h`) を `TerminalGuard::new` で
有効化し、crossterm `EventStream` が `MouseEvent` として配信する。
`--no-mouse` 指定時は `TerminalGuard` が mouse reporting を有効化しない。

| 入力 | Msg | 選択カーソルは動く？ |
|------|-----|----------------------|
| wheel-up | `ScrollByRows { row_delta: -1 }`（viewport を 1 行上に） | 動かない |
| wheel-down | `ScrollByRows { row_delta: +1 }`（viewport を 1 行下に） | 動かない |
| 未選択の bubble を左クリック | `SelectOrToggleById` | 動く（選択 + scroll 追従） |
| 選択中の bubble を左クリック | `SelectOrToggleById`（開閉トグル） | 動かない（開閉のみ） |

クリック位置の Y 座標は `compute_row_based_viewport` が返す `hit_items`
(`{message_id, start_row, end_row}` の配列) で逆引きする。
右クリック / 中クリック / ドラッグ / マウス移動はスコープ外で無視する
（ホイールや左クリック以外の Mouse Msg では `mark_dirty` を呼ばない）。

## コンポーネント詳細

### screen 関数

各 screen は `render(frame: &mut Frame, area: Rect, app: &App)` の純粋な描画
関数として実装される。状態管理はしない。

- **session_select**: セッション選択画面（上下キー + Enter）。各行は
  `{updated_at}  [ {subagent_count} ]  {display_text}` の 3 段構成で、
  サブエージェント数は `--show-all` の状態に連動した件数を `[ N ]` で表示する
- **subagent_select**: サブエージェント選択画面（アイコン付き）。
  各行頭に `[x]` / `[ ]` のチェックボックスを表示し、`Space` でカーソル位置
  のエージェントの選択をトグルできる。`Enter` で `[x]` が 1 件以上ならその
  全件、0 件ならカーソル位置 1 件にアタッチして WATCHING に遷移する
- **waiting**: 新規サブエージェント待機画面
- **watching**: サブエージェント監視画面（メッセージリスト表示）

### ChatBubble

- メッセージをチャット形式で表示
- 寄せ / ボーダー / 吹き出し幅は `App.chat_mode` (`settings::ChatMode`) で 3 モードに
  切り替わる。起動時は常に `default`、WATCHING 画面で `m` キーを押すたびに
  `default → line → slack → default` と循環する
  - `default`: Main → 右寄せ / Sub → 左寄せ、左 1 文字 `▏` マーカーのみ。
    吹き出し幅 = 端末幅の 60% (最低 30 列)
  - `line`: Main → 右寄せ / Sub → 左寄せ、上下左右 4 辺を `╭─...─╮ / │ │ /
    ╰─...─╯` で囲う (LINE アプリ風)。吹き出し幅は default と同じ 60%。
    `content_width == bubble_width` (4 辺枠線を bubble 全幅で描く)、本文 wrap
    幅はさらに左右 `│` の 2 列を引いた `content_width - 2`、高さは top/bottom
    border の +2 行
  - `slack`: Main / Sub 問わず **左寄せ**、左 1 文字 `▏` マーカーのみ。
    **吹き出し幅 = 端末幅 100% (全幅使用)**。左寄せで右側に余白を残すと
    間延びするため `compute_bubble_width` が `area_width` をそのまま返す
  - 左ボーダー色は 3 モード共通で sender 別 (Main=黄 / Sub=緑) を維持
- `render_bubble_rows` は行単位で `BubbleRowKind` を tag する:
  - `TopBorder` / `BottomBorder`: LINE モードのみ。`╭──╮` / `╰──╯` を描く
  - `Body`: ヘッダー / 本文行。左に `▏` (default/slack) または左右に `│`
    (line) が付く
  - `Margin`: バブル間の末尾空行。**枠線外スペーサー扱い**で `draw_bubble_row`
    は何も描かない。LINE モードで下枠 `╰──╯` の 1 行下に `│ │` が残って
    見えるバグの対策 (全モードでルールを揃える)
- タイムスタンプ表示（`format_date_time` でシステムローカル時刻に変換）
- `is_detailed_mode || expanded_ids.contains(id)` で展開表示を切り替え
- 選択中はヘッダーを inverse 表示 + `◄ selected` マーカーを出す
- **ツール呼び出し (tool_use) と結果 (tool_result) は同一バブル内に統合表示**
  される。ノイズ削減のためツール表示はミニマム化。
  - **通常モード (closed / preview)**: `ToolName` + input preview のみの
    **2 行構成 + ヘッダー / 末尾マージン = 計 4 行**。区切り (空行) /
    `↪ result` ラベル / result preview は出さない (= 単独 tool_use と同じ shape)
  - **詳細モード (expanded)**: ツール名 + input 全文 (折り返し) + 区切り (1) +
    `↪ result` + result 全文 (折り返し)。**インデントなし** (左揃え)
  - 結果が未到着の bubble は呼び出しのみ表示 (待機中)
  - 対応 tool_use が無い orphan な tool_result は独立 bubble として表示
  - `is_error=true` のとき `↪ result` の代わりに `↪ error` を赤色で表示
  - `tool_use.name == "Bash"` の bubble は append された瞬間に `expanded_ids` へ
    自動挿入される (`application::app::handle_messages_appended`)。これにより
    最新の Bash はデフォルトで Open 状態となり、結果が到着次第すぐ可視化される
  - **常に最新 1 件のみ Open**: 同一エージェント内で新 Bash が来たとき、
    `viewport.auto_open_bash_ids: HashMap<agent_id, bubble_id>` から前回 latest
    の id を引き、`viewport.user_toggled_ids` に **含まれない** ものは
    `expanded_ids` から remove する。マルチエージェント時は agent_id ごとに
    独立に track されるため、各 subagent の最新 Bash が同時に Open のまま保たれる
  - **`user_toggled_ids` は個別 toggle 操作のみ記録**: `viewport::toggle_expand`
    (Enter / Space / 個別クリック経由) で id を insert する。Ctrl+O の全体
    トグルは `expanded_ids` を mass-insert / clear するだけで `user_toggled_ids`
    には書かない (要件: 「Ctrl+O は明示的操作に含めない」)。一度 user_toggled
    に入った bubble は以降 latest Bash auto-close 対象から除外される
  - Bash 以外のツールは挿入対象外 (closed のまま)
- **ツール実行ステータスマーカー** を tool_use ラベル行の末尾に表示する
  (`ToolName  <マーカー>` の形)。`render_bubble_lines` / `render_bubble_rows`
  に `spinner_phase: u8` を渡し、`push_tool_use_lines` がメッセージの状態に応じて
  Span を末尾に差し込む。
  - 実行中 (`tool_use=Some && tool_result=None`): Braille スピナー文字
    `SPINNER_FRAMES[phase % SPINNER_FRAMES.len()]` (Magenta)
  - 完了 (`tool_use=Some && tool_result=Some && !is_error`): `✓` (Green + Bold)
  - エラー (`tool_use=Some && tool_result=Some && is_error`): `✗` (Red + Bold)
  - text / orphan tool_result バブルにはマーカーを付けない
  - マーカー Span は本文の折り返しには影響しない (ラベル行末尾の固定 1 文字 +
    前置きスペース)。`estimate_bubble_height` の tool_use 行数は従来通り 1
  - 描画層 (watching.rs) は viewport 内に **未完了 tool_use** が 1 件でも
    あるかを `ViewportCache.has_inflight_in_view` に集計し、`Tick` ハンドラが
    このフラグを見て `spinner_phase += 1 + mark_dirty` を発火する。なければ
    Tick で何もしないので画面は静止し、再描画コストはゼロ
- **text バブルに mention prefix + 中間 text 薄色化** を適用
  (対話相手と独り言の視覚的区別)
  - mention prefix は 1 行目の先頭に inline Span として差し込まれる
    (Cyan + Bold、末尾に半角スペース 1)
    - `Sender::Main` の text → `@{agent_type}` (Main → agent の prompt)
    - `Sender::Sub` + `is_final_response=true` → `@Main` (agent → Main 最終応答)
    - `Sender::Sub` + `is_final_response=false` → mention なし (独り言)
  - 中間 text (`Sender::Sub` + `!is_final_response`) は本文全体を `Color::DarkGray`
    で描画して薄く見せる。ヘッダー (アイコン + ラベル + timestamp) と末尾マージン
    は現行色のまま (選択ハイライトと干渉させないため)
  - 1 行目の折り返し幅は `content_width - mention_width` に縮み、2 行目以降は
    通常の `content_width`。`estimate_bubble_height` の
    `wrap_line_count_with_prefix` と `render_bubble_lines` の
    `wrap_lines_iter_with_prefix` が同じロジックを共有 (lessons.md の
    「見積もり == 実描画行数」原則)
  - tool_use / tool_result バブルは mention / 薄色の対象外 (適用しない)

### 描画最適化

- **dirty flag**: `App::needs_redraw` が立っているフレームだけ `terminal.draw`
  を呼ぶ。`Tick` 単体では描画しない
- **Buffer 直書き**: ChatBubble の各行は `Buffer::set_line` で直接書き込む
  （`Paragraph::wrap` 経由だと pre-wrap 済みなのに WordWrapper を二重にかけて
  しまう。`render_bubble_lines` は 1 Line = 1 物理行を保証する）
- **event_loop の batch drain**: watcher からの `cmd_rx` burst は
  `drain_ready_messages` で一気に消化して最終状態だけ描画する。
  crossterm イベント（キー/マウス）は `biased` select で逐次処理して
  スムーズスクロールを維持

## UI ユーティリティ

### icons.rs

- `get_agent_icon(agent_type)`: エージェントタイプに対応するアイコンを取得

### format.rs

- `format_date_time(ts, fmt)`: UTC 時刻をシステムローカルに変換してから strftime
  書式で整形する。全 UI 時刻表示はこのヘルパー経由で統一する（`DateTime<Utc>`
  に `.format()` 直接呼びは UTC のまま整形されるので禁止）
- `format_time(ts)`: `format_date_time(ts, "%H:%M:%S")` のエイリアス
- `format_tool_input(input, max_len)`: ツール入力パラメータを `key="value"`
  形式に整形
- `format_tool_preview(name, input)`: ツール呼び出しの 2 行目プレビュー
- `format_result_preview(content)`: ツール結果の 1 行プレビュー

## zellij での利用例

```bash
# 右ペインでビューワー起動
zellij run -f -- cc-chatter

# 複数サブエージェント監視は複数ペインで
zellij run -f -- cc-chatter &
zellij run -f -- cc-chatter &
# → それぞれ別のサブエージェントに自動アタッチ（ロック機構により排他）
```
