# cc-chatter Specification

Claude Code のサブエージェント間やりとりをリアルタイム可視化する TUI ツール。

## 用語

| 用語          | 説明                                                  |
| ------------- | ----------------------------------------------------- |
| workspace     | Claude Code プロジェクト単位（ログパスに含まれる）    |
| sessionId     | Claude Code のセッション識別子（`{sessionId}.jsonl`） |
| agentId       | サブエージェント識別子（`agent-{agentId}.jsonl`）     |
| subagent_type | サブエージェント種別（Explore / Plan / Bash 等）      |

## CLI

```bash
cc-chatter                                      # セッション選択UI
cc-chatter -w .                                 # カレントディレクトリのワークスペースに絞る
cc-chatter --latest                             # 最新セッションに自動アタッチ
cc-chatter --latest --agent <agent-id>          # 特定エージェントにアタッチ
cc-chatter --latest --show-all                  # 非表示エージェントも表示
cc-chatter --session <id>                       # セッションID前方一致指定で自動選択
cc-chatter --session <id> --agent <agent-id>    # セッション + エージェント指定
cc-chatter --limit 20                           # セッション一覧の表示件数を制限
cc-chatter --since 30d                          # 直近 30 日のセッションのみ表示
```

### オプション

| オプション               | 説明                                                                                                                              |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------- |
| `-w, --workspace <path>` | 対象 workspace を絞る（指定なしはカレントディレクトリ配下に絞る）                                                                 |
| `--latest`               | 最新セッションに自動アタッチ                                                                                                      |
| `--session <id>`         | セッションID前方一致指定（マッチ1件で自動選択、複数/0件はエラー）                                                                 |
| `--agent <agent-id>`     | 特定エージェントにアタッチ（`--latest` または `--session` と併用）                                                                |
| `--since <duration>`     | セッション一覧を `updatedAt` が直近 `<duration>` 以内のものに絞る。相対期間のみ受理（例: `7d` / `24h` / `30m`）。デフォルト: `7d` |
| `--limit <num>`          | セッション一覧の最大表示件数（デフォルト: 50）。`--since` と併用され、期間で絞った後さらに件数で切る                              |
| `--show-all`             | 非表示エージェントも表示                                                                                                          |
| `--no-mouse`             | WATCHING 画面のマウス reporting を無効化（クリック/ホイール無効）                                                                 |

## 機能一覧

### セッション選択

- デフォルトでカレントディレクトリ配下のワークスペースのセッションを表示
- `-w` オプション指定時はそちらを優先（指定値がなければカレントディレクトリを使用）
- セッション一覧の絞り込みは **期間 (`--since`) と件数 (`--limit`) の併用**
  - `--since` でまず `updatedAt` が直近 N 期間以内のセッションだけに絞る（デフォルト: 直近 7 日）
  - その上で `--limit` の件数上限を適用する（デフォルト: 50 件）
  - `--since` の値は相対期間のみ。`Nd` / `Nh` / `Nm`（`N` は 1 以上の整数）を受理する
- Ctrl+R で手動更新（自動更新はしない）
- セッション選択後、既存サブエージェントの有無で分岐：
  - あり → 「新規待ち」or「既存から選択」
  - なし → 新しいサブエージェント起動を待機

#### ソート順（updatedAt）

セッション一覧は `updatedAt` の降順でソート。`updatedAt` は以下のルールで算出：

- **最後の明示的なユーザーアクション**の timestamp を使用
- ローカルコマンド関連のメッセージ（`/exit` 等）は除外
- システム的な書き込み（summary 追加など）は `updatedAt` に影響しない
- 該当するユーザーアクションがない場合はファイルの mtime にフォールバック

#### セッション表示テキスト

セッション一覧には以下の優先順位でテキストを表示：

1. `summary`（`type: "summary"` エントリから取得）
2. 最初のユーザープロンプト（`type: "user"` エントリ）
3. 「(新規セッション)」

#### サブエージェント数の表示

セッション一覧では `updatedAt` の右側に **そのセッションに紐づくサブエージェント数** を `[ N ]` 形式で表示する。

- カウント対象は `subagents/` 配下の `agent-*.jsonl` ファイル（`.meta.json` 等は除外）
  - **Workflow ツール経由のサブエージェント** (`subagents/workflows/<wf_runId>/agent-*.jsonl`) も
    1 階層下を walk してカウントに含める（詳細は「サブエージェント監視 / Workflow
    agent の可視化」を参照）
- `--show-all` の状態と連動する
  - `--show-all` 無し（既定）: `HIDDEN_AGENT_PREFIXES`（`aprompt_suggestion` 等）にマッチする `agent_id` を除いた件数
  - `--show-all` 指定時: HIDDEN を含む総件数
- N が 0 でも `[ 0 ]` を表示してカラム幅を揃える

※ ローカルコマンド関連のメッセージ（`<local-command-caveat>`, `<command-name>` 等で始まるもの）はスキップ

> **TODO**: セッション要約の取得方法のベストプラクティスを調査（現在はプレフィックスでのフィルタリング）

### サブエージェント監視

- サブエージェントの出力ファイルを notify で監視
- JSONL を解析し、追記分をリアルタイムに UI へ反映
- サブエージェントの名前としてagentTypeを表示
- **Workflow agent の可視化** (Workflow ツール経由のサブエージェント):
  - Claude Code の Workflow ツールが `agent()` で fan-out するサブエージェントは、
    通常の `subagents/agent-*.jsonl` ではなく
    **`subagents/workflows/<wf_runId>/agent-*.jsonl`** に 1 階層深く書かれる
    (隣に `agent-*.meta.json` と、run 全体の `journal.jsonl` がある)
  - cc-chatter はこの 1 階層下も walk して通常のサブエージェントと **同じ
    フラットなリスト**に統合表示する。multi-attach (Space 選択 → Enter) も
    通常 agent と同様に使える
  - `agent_type` は各 agent の `meta.json` (`{"agentType":"..."}`) から解決する。
    Workflow の generic な agent は `"workflow-subagent"`、ドメイン reviewer 等は
    `"consistency-reviewer"` のようなカスタム型を持つ
  - 区別のため、SUBAGENT_SELECT では workflow agent に **🧩 アイコン + `wf:<run>`
    マーカー**を付ける。generic な `workflow-subagent` については、agent の最初の
    prompt 先頭から **短いロール label を導出**して型カラムに表示する
    (例: `code-review の finder`)。`agent()` に渡される人間可読な label そのものは
    Claude Code 側で永続化されないため、prompt 先頭からの導出で代替する
  - watcher は run ディレクトリの追加・追記も検知できるよう、メインログ監視
    (subagents ディレクトリ監視) を再帰的に行う。個別 agent ログの追尾は従来通り
    対象ファイルを直接監視する
- **マルチエージェントビュー** (Multi-attach mode):
  - SUBAGENT_SELECT 画面で `Space` キーを押すとカーソル位置のエージェントの
    選択状態 (`[ ]` / `[x]`) をトグルできる。複数選択した状態で `Enter`
    すると、選択した全エージェントを同時に監視し、1 つのチャットに統合表示する
  - 何もチェックしないまま `Enter` した場合は従来どおりカーソル位置の 1 件に
    アタッチする
  - メッセージは **受信順 append**（同一エージェント内では時系列順、エージェント
    間ではバースト順）で並ぶ。`@` 付き mention は従来どおり「誰宛の発言か」を
    表すので、独り言と対話を視覚的に区別できる
  - エージェントごとの色分けやツール表示の最小化は別タスク（このマイルストーン
    では基本表示のみ）
- ツール呼び出し (tool_use) と結果 (tool_result) は **同一バブルに統合表示** される
  - **closed (折りたたみ)**: ツール名 + 引数 preview のみ。`↪ result` ラベル /
    結果 preview / 区切りの空行は出さない (ノイズ削減のためツール表示を
    ミニマム化)
  - **open (展開)**: ツール名 + 引数 全文 + 区切り 1 行 + `↪ result` (またはエラー時
    `↪ error`) + 結果 全文。**インデントなし** (左揃え)
  - 並列ツール呼び出しでは `tool_use_id` で個別に紐付くため、各バブルに対応する
    結果が取り込まれる
  - 対応する呼び出しが見つからない結果 (orphan) は独立バブルとして fallback 表示
- **「最新の Bash」はデフォルトで Open** で表示される
  - Bash の tool_use bubble が新たに append された瞬間に自動で「開」になる
    (内部的には `expanded_ids` に id を insert)。結果 (tool_result) が後から
    到着しても即座に view に表示され、リアルタイムの出力確認に近い体験になる
  - Bash 以外の tool_use (Read / Grep / WebFetch 等) は閉じたまま (= 結果省略)。
    必要に応じてユーザーが Enter / Space / クリックで開く
  - **過去の Bash は自動で閉じる**: 同一エージェント内で新しい Bash bubble が
    到着すると、そのエージェントの **前回 latest Bash** は自動的に
    `expanded_ids` から除外される (= 「常に最新 1 件のみ Open」)
  - **明示的にユーザーが開閉操作した bubble は自動 close 対象から除外** される。
    Enter / Space / 個別クリックでトグルされた id は `user_toggled_ids` に
    記録され、以降の Bash 自動 close ではスキップされる (= ユーザーの意思を
    尊重する)
  - **Ctrl+O は明示的操作に含まれない**: 全体トグルは `expanded_ids` を
    mass-insert / clear するだけで `user_toggled_ids` には記録しない。
    したがって Ctrl+O で開いた過去 Bash は、次の Bash 到着で auto-close される
  - **マルチエージェント時は agent ごとに最新 1 件**: `auto_open_bash_ids:
    HashMap<agent_id, bubble_id>` で各 subagent の最新 Bash を独立に track。
    複数の subagent をアタッチしているとき、それぞれの最新 Bash が同時に
    Open 状態を保つ
  - Ctrl+O / Enter / Space / クリックでの開閉操作は他のバブルと同様に機能する
- ツール呼び出しバブルには **実行ステータスマーカー** を tool_use ラベル行の
  末尾に表示する
  - **実行中** (`tool_use` あり / `tool_result` 未到着): Braille スピナー
    `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` を 100ms ごとに 1 フレーム進めてアニメーションする (Magenta)
  - **完了** (`tool_result` 到着 / `is_error=false`): `✓` を Green で表示
  - **エラー** (`tool_result.is_error=true`): `✗` を Red で表示
  - orphan な tool_result はマーカー対象外 (ラベル `↪ error` / `↪ result` で
    状態判別できる)
  - text バブルもマーカー対象外
  - スピナーは「viewport 内に未完了 tool_use がある間だけ」アニメする
    (画面外に流れた未完了バブルや 0 件のときは Tick で再描画されない)
- `Bash` の preview は Git 操作を特別扱いする
  - `git add` / `git stage` 相当は `Stage`
  - `git commit` は `Commit`
  - `git push` は `Push`
  - `git pull` は `Pull`
  - `git merge` は `Merge`
  - `git rebase` は `Rebase`
  - `git checkout` / `git switch` / `git restore` は `Checkout`
  - `git log` は `Git Log`
  - `cd ... && git ...` や `git -C ...` のように Git 実行前にディレクトリ移動や
    オプションが入るケースも同じ分類で表示する
  - 上記以外は従来どおり Bash コマンドの 1 行目をそのまま preview 表示する
- text バブルには **対話相手を示す mention prefix** を差し込む (Slack / Discord
  風)。本文 1 行目の先頭に `@xxx` (Cyan + Bold) が付く
  - Main → agent の prompt: `@{agent_type}` (どのエージェントに向けた指示か明示)
  - agent → Main の最終応答 (`stop_reason == "end_turn"` の最後の text item):
    `@Main` (Main 宛ての成果報告であることを示す)
  - agent の中間 text (stop_reason が `null` / `"tool_use"` など、Main には
    届かない独り言): mention なし + 本文全体を **DarkGray で薄色化** する
    (独り言 / 思考過程であることを視覚的に区別)
  - tool_use / tool_result バブルには mention / 薄色は適用しない

### ChatBubble の表示モード

起動時の表示モードは常に `default`。WATCHING 画面で `m` を押すたびに
`default -> line -> slack -> default` と循環切替できる。

| モード    | 寄せ                     | 吹き出し幅                    | 枠線                                                                 |
| --------- | ------------------------ | ----------------------------- | -------------------------------------------------------------------- |
| `default` | Main=右寄せ / Sub=左寄せ | 端末幅の 60% (最低 30 列)     | 左 1 文字 `▏` の縦線マーカーのみ (現状どおり)                        |
| `line`    | Main=右寄せ / Sub=左寄せ | 端末幅の 60% (default と同じ) | 上下左右 4 辺を `╭─...─╮ / │ ... │ / ╰─...─╯` で囲う (LINE アプリ風) |
| `slack`   | すべて左寄せ             | **端末幅 100% (全幅使用)**    | 左 1 文字 `▏` の縦線マーカーのみ (色は sender 別に維持)              |

- 左ボーダー色は 3 モードとも sender 別 (Main=黄 / Sub=緑) を維持する。
  slack モードでも mention prefix (`@Main` / `@{agent_type}`) と合わせて
  「誰の発言か」が分かるようにするため。
- `slack` は全バブルを左寄せにするため、右側に余白を残すと間延びして見える。
  これを避けるため bubble 幅 = `area_width` そのもの。
- `line` モードでは bubble の本文幅が左右ボーダー 1 列ずつ分縮み、高さも top /
  bottom border の 2 行ぶん増える。viewport の行ベーススクロール / 選択追従 /
  heights キャッシュはすべてこの差分を吸収する (`estimate_bubble_height_full`
  と `render_bubble_rows` が同じ `effective_inner_width` を共有)。
- **バブル間の末尾空行は枠線外スペーサー**扱いで、どのモードでも `│` / `▏`
  を描かない (`BubbleRowKind::Margin`)。LINE モードで `╰──╯` の 1 行下に
  `│ │` だけ残るバグを防ぐための共通ルール。

### メインログ監視 & マッピング

- メインログから `agentId ↔ subagent_type` マッピングを構築
- サブエージェントをタイプ別アイコンで装飾表示

### エージェントタイプアイコン

| タイプ            | アイコン | 説明                        |
| ----------------- | -------- | --------------------------- |
| Main Agent        | 🤖       | メインエージェント          |
| Explore           | 🔍       | コードベース探索            |
| general-purpose   | 🔹       | 汎用エージェント            |
| Plan              | 📋       | 計画立案                    |
| Bash              | 💻       | コマンド実行                |
| claude-code-guide | 📚       | ドキュメント検索            |
| statusline-setup  | ⚙️        | ステータスライン設定        |
| workflow-subagent | 🧩       | Workflow ツール経由の agent |
| unknown           | 🔸       | 未知のタイプ                |

> SUBAGENT_SELECT 画面では、`workflow_run` を持つ agent（Workflow ツール経由）は
> agentType に関わらず 🧩 を優先表示し、行頭に `wf:<run>` マーカーを付ける。

### 非表示エージェント

以下プレフィックスの agentId はデフォルト非表示：

- `aprompt_suggestion`（Claude Code 内部のプロンプト提案機能）

`--show-all` で表示可能。

### キーボードショートカット

| キー          | 画面                            | 動作                                                                                        |
| ------------- | ------------------------------- | ------------------------------------------------------------------------------------------- |
| ↑/↓           | SESSION_SELECT, SUBAGENT_SELECT | 選択移動                                                                                    |
| Space         | SUBAGENT_SELECT                 | カーソル位置のエージェントの選択 (`[x]`) をトグル                                           |
| Enter         | SESSION_SELECT, SUBAGENT_SELECT | 決定。SUBAGENT_SELECT で `[x]` が 1 件以上ならその全件、0 件ならカーソル位置 1 件にアタッチ |
| Ctrl+R        | SESSION_SELECT, SUBAGENT_SELECT | リスト更新                                                                                  |
| ↑/k           | WATCHING                        | 選択を 1 つ上へ                                                                             |
| ↓/j           | WATCHING                        | 選択を 1 つ下へ                                                                             |
| Enter / Space | WATCHING                        | 選択中メッセージの開閉トグル                                                                |
| g             | WATCHING                        | 先頭へジャンプ                                                                              |
| G             | WATCHING                        | 末尾へジャンプ（末尾追従 ON）                                                               |
| Ctrl+D        | WATCHING                        | 半ページ下スクロール                                                                        |
| Ctrl+U        | WATCHING                        | 半ページ上スクロール                                                                        |
| Ctrl+O        | すべて（WATCHING で効果）       | 全メッセージを強制展開（詳細モード）のトグル                                                |
| m             | WATCHING                        | ChatBubble 表示モードを `default -> line -> slack` で循環切替                               |
| Esc           | SUBAGENT_SELECT                 | セッション選択に戻る                                                                        |
| Esc           | WAITING                         | セッション選択に戻る                                                                        |
| Esc           | WATCHING                        | サブエージェント選択に戻る                                                                  |
| Ctrl+C        | すべて                          | 終了                                                                                        |

### マウス操作（WATCHING 画面のみ）

TUI 起動時に SGR mouse reporting（`CSI ? 1006 h` + `CSI ? 1002 h`）を有効化し、
以下の操作を受け付ける。`--no-mouse` フラグで無効化できる。

| 操作                             | 動作                                                          |
| -------------------------------- | ------------------------------------------------------------- |
| スクロールホイール（上）         | viewport を **1 行** 上にスクロール（選択カーソルは動かない） |
| スクロールホイール（下）         | viewport を **1 行** 下にスクロール（選択カーソルは動かない） |
| ChatBubble を左クリック          | そのメッセージを選択（viewport も選択に追従）                 |
| 選択中の ChatBubble を再クリック | 開閉トグル（= `Enter` / `Space`）                             |

スクロールは行単位で動く（メッセージ単位だと高さが可変でカクつくため）。
1 tick = 1 行は TUI で可能な最小粒度（セルはアトミック）。ゆっくり回せば
細かく、勢いよく回せば端末が複数 tick を連続で送るので自然に速くなる。
スクロールと選択が独立しているため、ホイールで履歴を見渡しつつ、気になる
メッセージをクリックで選択する、という操作が自然に行える。選択の追従を
使いたい場合はキーボード（`j/k/↑/↓/g/G/Ctrl+D/Ctrl+U`）を使う。

キーボード操作はマウスと並列で常に有効。マウスを使えない環境でも既存の
キーバインドで完結するよう設計されている。

#### マルチプレクサ / SSH 環境での注意事項

- **zellij**: マウス機能を内部アプリへ通すには設定で `mouse_mode: true`
  が必要（デフォルトは有効）。`Ctrl+Shift+...` 系の zellij 側ショートカット
  を使うと mouse reporting が奪われる場合がある。
- **tmux**: `set -g mouse on` を `~/.tmux.conf` に入れていないと
  マウスイベントが内側に届かない。`tmux source ~/.tmux.conf` を忘れずに。
- **ssh 経由**: クライアント（iTerm2, Alacritty, WezTerm など）側で
  マウスレポートを許可している必要がある。iTerm2 はデフォルトで通る。
- **OS 標準 Terminal.app**: SGR mouse reporting は対応しているが、
  Option+クリック等は端末側で食われることがある。
- **ssh で転送が効かない場合**や動作が怪しい場合は `--no-mouse` で opt-out
  することでキーボードのみのモードに戻せる。終了時に reporting は必ず
  解除されるためシェルが壊れることはない。
