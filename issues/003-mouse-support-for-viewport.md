# 003: WATCHING 画面のマウス操作サポート

- Status: **Planned**（issue 001 / 002 完了後に別ブランチで対応予定）
- Priority: Medium
- Depends on: issue 001（自前 viewport 実装）、issue 002（Agent tool 名不整合の修正）
- Related: `src/ui/components/screens/WatchingScreen.tsx`, `src/ui/utils/viewportLayout.ts`, `src/ui/App.tsx`

## 背景 / モチベーション

issue 001 で自前 viewport によるキーボード操作（`j/k`、`Enter`/`Space`、`g/G`、`Ctrl+D/U`）が実装された。
キーボードのみでも運用は可能だが、以下の場面でマウスが使えると体験が良くなる:

- トラックパッド / マウスホイールで直感的にスクロールしたい
- 長いメッセージリストから目的のバブルをクリックで直接選択したい
- クリックで個別開閉（現在の `Enter` / `Space` と等価）

## ゴール

キーボード操作はそのまま残しつつ、以下のマウス操作を追加する:

1. **スクロールホイール上下** → 1 行分（= 1 メッセージ）選択を移動
2. **ChatBubble をクリック** → そのメッセージを選択
3. **選択中の ChatBubble を再クリック / ダブルクリック** → 開閉トグル
4. （将来）フッターの `▲ N more above` / `▼ N more below` をクリック → 半ページスクロール

## 技術的ハードル

### ink にはコンポーネント単位のクリックイベントが無い

ink の `useInput` はキー入力のみ。マウスイベントは生の stdin に `CSI < b ; x ; y M/m` のような
xterm mouse reporting プロトコルの ANSI エスケープシーケンスとして流れてくるため、
**自前でパースする必要がある**。

### Y 座標 → メッセージ index の逆引き

ChatBubble の高さは内容可変。`viewportLayout.ts` の `estimateBubbleHeight` で事前計算
している高さをそのまま使えるが、**描画順にヒット判定用の `{startRow, endRow, messageId}`
マッピング配列を作成**する必要がある。

現状は表示範囲だけ `startIndex`/`endIndex` を持つ `ViewportSlice` を返しているが、
マウスヒット用に各メッセージの表示上の Y 範囲を返すよう拡張する。

### 環境依存のハマりポイント

- **zellij / tmux**: `set -g mouse on` / mouse plugin が有効でないとマウスイベントを
  内側のアプリに通さない。ドキュメントで既知の制約として明記する。
- **ssh 経由**: ssh クライアント側でマウス転送が無効なケースがある（iTerm2 / Alacritty
  などは通常問題なし）。
- **OS 標準 Terminal.app**: xterm mouse reporting は対応しているが、Option+クリック等の
  修飾キー付きクリックはターミナル側で食われる場合がある。
- **Raw mode + mouse protocol**: 有効化 `\x1b[?1000h` / `\x1b[?1002h` / `\x1b[?1006h`、
  無効化 `\x1b[?1000l` 等。アプリ終了時に必ず無効化して元の mode に戻さないと、
  終了後のシェルでマウスが壊れる。

## 実装方針

### 1. Mouse input layer

`src/infrastructure/input/MouseReporter.ts`（新規）:

- プロセス起動時に `\x1b[?1006h\x1b[?1002h` を stdout に書き込んで SGR mouse reporting
  + button-event tracking を有効化
- `process.stdin` の `data` イベントを購読し、`\x1b[<{b};{x};{y}M/m` 形式を正規表現でパース
- `MouseEvent { button: "left"|"right"|"middle"|"wheel-up"|"wheel-down", x, y, action: "press"|"release" }` を
  EventEmitter で発火
- `ink` の useInput と共存させる（ink は非対応のエスケープを無視するが、両方が同じ
  stdin を listen するので読み取った data を両方に流す形にする）
- プロセス終了時（`signal-exit` 等で）に `\x1b[?1002l\x1b[?1006l` で必ず解除

### 2. React 側のフック

`src/application/hooks/useMouse.ts`（新規）:

- `MouseReporter` をラップして React の useEffect で購読
- 呼び出し側で `(event: MouseEvent) => void` を受け取る

### 3. Viewport layout の拡張

`src/ui/utils/viewportLayout.ts`:

- `computeViewportSlice` の戻り値に `items: Array<{messageId, startRow, endRow}>` を追加
- `startRow` は header 行数からのオフセット。WATCHING 画面で `HEADER_LINES` を足せば絶対 Y になる
- Y 座標 → messageId の逆引きヘルパ `findMessageAtRow(items, y): messageId | null` を追加

### 4. WatchingScreen への統合

`src/ui/components/screens/WatchingScreen.tsx`:

- `useMouse` で購読
- ホイールイベント → `viewportDispatch({ type: "MOVE_UP" | "MOVE_DOWN" })`
- 左クリック → `findMessageAtRow(items, event.y - HEADER_LINES)` で messageId を取得
  - 取得できたら `SELECT_BY_ID` アクションを dispatch（新規）
- 選択済みの bubble を再クリック → `TOGGLE_EXPAND` を dispatch
  - 二回目クリックの判定: state に `lastClickedId` と `lastClickedAt` を持たせるか、
    viewport reducer に `SELECT_OR_TOGGLE_BY_ID` のような意味論的アクションを追加

### 5. viewportReducer への新アクション

`src/application/state/viewportState.ts`:

- `SELECT_BY_ID`: `{ type, messageId, messageIds }` — 指定 id を選択、followTail 再計算
- `SELECT_OR_TOGGLE_BY_ID`: 既選択なら TOGGLE_EXPAND、そうでなければ SELECT_BY_ID
  （ダブルクリック相当の簡易実装）

## 設計判断が必要な点

### ホイール量

- 1 イベント = 1 メッセージ移動（`MOVE_UP` / `MOVE_DOWN`）
- 粗すぎる場合はスムーズスクロール（行単位）を検討するが、現在の viewport は
  メッセージ単位なので 1 メッセージで十分と判断
- Shift+ホイール → 半ページ（= `PAGE_UP` / `PAGE_DOWN`）を割り当ててもよい

### 右クリック / 中クリック

- スコープ外。ドキュメントに明記して将来対応とする
- 左クリック + ホイールのみをサポート

### ダブルクリック vs 単クリックトグル

- 案 A: 未選択 bubble を 1 クリックで選択、選択中 bubble を再度 1 クリックで開閉
- 案 B: 1 クリックで選択、ダブルクリックで開閉
- 案 A の方が「選択」と「操作」が 1 アクションずつで分かりやすい。既存の Enter/Space と
  意味論的に一致（Enter/Space は選択中 bubble に対して作用するため）
- **採用: 案 A**

### マウスが無効な環境

- `TERM` / 環境変数で判定して自動 off にする（`dumb`, `cons25` 等）
- もしくは CLI オプション `--no-mouse` で opt-out

## 受け入れ基準

- [ ] iTerm2 / Terminal.app / Alacritty でホイールスクロールが動く
- [ ] ChatBubble をクリックすると選択される
- [ ] 選択中 bubble を再クリックすると展開/折りたたみする
- [ ] Esc / Ctrl+C で終了したとき、シェルのマウスが壊れていない（reporting 解除される）
- [ ] 既存のキーボード操作（`j/k`, `Enter`, `Ctrl+D/U` 等）がそのまま動く
- [ ] `--no-mouse` フラグでマウス無効化ができる
- [ ] zellij / tmux 内での挙動をドキュメント (`docs/spec.md` / README) に記載
- [ ] 新規 `MouseReporter` / `useMouse` / viewport アクションに単体テストがある
- [ ] `bun run typecheck` / `bunx biome check .` / `bun test` が全て通る

## 作業ブランチ

`claude/003-mouse-support`

## 備考

- 工数目安: 土台（MouseReporter + useMouse + layout 拡張）で 1 日、ヒット判定 + viewport
  統合で 0.5 日、クロスターミナル動作確認とバグ修正で 0.5 日 = 合計 2 日程度
- issue 001 / 002 が両方 main にマージされた後で着手すること。依存する viewportLayout
  の構造に手を入れるため、同時並列化するとマージ競合が激しくなる
- マウスを使えない環境でもキーボードだけで完結する現状の体験を壊さないのが大前提
