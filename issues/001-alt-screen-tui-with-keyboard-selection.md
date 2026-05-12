# 001: Alt-screen TUI 化とキーボード選択による個別開閉

- Status: **Planned**（別ブランチで実装予定）
- Priority: High
- Depends on: なし
- Related: 現 `main` ブランチの WATCHING 画面（`src/ui/components/screens/WatchingScreen.tsx`）

## 背景 / モチベーション

現行の WATCHING 画面は ink の `<Static>` で全メッセージを scrollback に確定印字しているため、以下の問題が残っている:

1. **長いプロンプトやレスポンスで画面が占有される**。tool_use の引数全文や tool_result の出力が長いケースで顕著
2. 現在の Ctrl+O（詳細モード全体トグル）は **既に印字済みのメッセージに遡及適用できない**（ターミナル出力済みの行は書き換え不可という原理的制約）
3. 個別メッセージの開閉ができない

`<Static>` でのアプローチでは「一度印字した出力を後から再レンダリングする」ことが原理的にできないため、個別開閉を実現するには **scrollback を捨て、ink の live frame 内で自前 viewport を管理する TUI** にリライトする必要がある。

## ゴール

- 長文メッセージをデフォルト折りたたみで表示し、任意のメッセージを選択して個別に展開/折りたたみできる
- 任意のタイミングで全メッセージをスクロールして参照できる（新着が届いてもスクロール位置が保持される）
- キーボードのみで完結する操作性（zellij / tmux 内での利用を前提）

## 仕様

### キーバインド

| キー | 動作 |
|------|------|
| `↑` / `k` | 選択を 1 つ上へ |
| `↓` / `j` | 選択を 1 つ下へ |
| `Enter` / `Space` | 選択中メッセージの開閉トグル |
| `g` | 先頭へジャンプ |
| `G` | 末尾へジャンプ |
| `Ctrl+D` | 半ページ下スクロール |
| `Ctrl+U` | 半ページ上スクロール |
| `Esc` | SUBAGENT_SELECT 画面へ戻る（既存） |
| `Ctrl+C` | 終了（既存） |

### 画面構成

```
┌─ cc-chatter - Watching 🔍 agent-xxx (Explore) ─────────────┐
│ Status: watching                                          │
├───────────────────────────────────────────────────────────┤
│ 🤖 Main  12:34                                            │  ← 折りたたみ（プレビュー）
│ ▶ こんにちは、コードレビューをお願いします...              │
│                                                           │
│ 🔍 Explore  12:35  ◄ selected                             │  ← 選択中（ハイライト）
│ ▼ full expanded content here                              │  ← 展開済み
│   more lines...                                           │
│   more lines...                                           │
│                                                           │
│ 🤖 Main  12:36                                            │
│ ▶ ありがとうございます...                                  │
├───────────────────────────────────────────────────────────┤
│ [2/5] j/k: move | Enter: toggle | g/G: top/bot | Esc: back│
└───────────────────────────────────────────────────────────┘
```

### 折りたたみ / 展開の基準

- **デフォルト**: 全メッセージ折りたたみ（現 `isDetailedMode=false` 相当のプレビュー表示）
- **展開**: 選択して Enter すると、そのメッセージの `isDetailedMode=true` 相当を適用
- 展開状態は `Set<messageId>` でメッセージごとに保持

### 新着メッセージの扱い

- デフォルトでは**新着は末尾に追加されるが、現在の選択位置・スクロール位置は変更しない**（ユーザーの閲覧を邪魔しない）
- 末尾（`G` 位置）にいるときだけは追従する（「最新を見ている」モード）
- 追従状態のインジケータをフッターに表示（例: `[live]` / `[paused]`）

## 設計方針

### TUI 化の骨子

1. **alt-screen buffer を使わない**。ink は alt-screen を自動でトグルしないが、live frame の高さをターミナル高さ以下に抑えれば自前 viewport は実現できる
   - → 代わりに `<Static>` を削除し、全表示を live frame 内に閉じ込める
   - → 高さ計算をきっちり行い、`stdout.rows - header - footer` を超えないメッセージ範囲だけ描画
2. **viewport state** を `useAppController` 系に追加
   - `scrollOffset: number`（先頭から何メッセージ目が画面先頭か、ではなく**何行目**で管理する方が可変高に対応しやすい）
   - `selectedIndex: number`（メッセージ配列のインデックス）
   - `expandedIds: Set<string>`（messageId で管理）
   - `followTail: boolean`（末尾追従フラグ）
3. **メッセージの高さ測定**: ChatBubble の高さは内容によって可変。ink の `measureElement` を活用するか、折りたたみ時は定数行（3 行程度）、展開時は wrap 後の行数を計算する関数を別途実装する
4. **スクロール/選択の連動**: 選択が画面外に出たら自動で scroll を追従させる
5. **リサイズ対応**: `stdout.on("resize")` を拾って再描画をトリガーする

### State 設計（案）

```ts
// application/state/viewportState.ts
interface ViewportState {
  scrollOffset: number;       // 画面先頭に対応するメッセージ index
  selectedIndex: number;       // 選択中のメッセージ index
  expandedIds: Set<string>;    // 展開中メッセージの id
  followTail: boolean;         // 末尾追従モード
}

type ViewportAction =
  | { type: "MOVE_UP" }
  | { type: "MOVE_DOWN" }
  | { type: "TOGGLE_EXPAND" }
  | { type: "JUMP_TOP" }
  | { type: "JUMP_BOTTOM" }
  | { type: "PAGE_UP" }
  | { type: "PAGE_DOWN" }
  | { type: "MESSAGE_APPENDED"; newLength: number };
```

### ChatBubble の拡張

現行の `isDetailedMode: boolean` prop を維持。代わりに `WatchingScreen` 側で `expandedIds.has(msg.id)` を見て個別に `isDetailedMode` を渡す。

- **未実装のメッセージ ID**: 現 `FormattedMessage` に `id` フィールドを追加する（timestamp + index の合成でも可）

### メッセージ識別子の付与

`MessageMapper` もしくは ingest 経路で `FormattedMessage.id` を付与する。

```ts
interface FormattedMessage {
  id: string;           // 新規追加（例: `${timestamp}-${index}` or uuid）
  sender: "main" | "sub";
  timestamp: Date;
  text?: string;
  toolUse?: ToolUse;
  toolResult?: ToolResult;
}
```

## 影響範囲 / 修正対象

- `src/ui/components/screens/WatchingScreen.tsx`: 全面書き直し
- `src/ui/components/ChatBubble.tsx`: props / レイアウト微調整（選択ハイライト対応）
- `src/application/hooks/useAppController.ts`: viewport アクションを追加
- `src/application/state/appReducer.ts`: 選択/スクロール状態を追加 or 新 reducer を導入
- `src/domain/entities/LogEntry.ts`: `FormattedMessage.id` 追加
- `src/application/mappers/MessageMapper.ts`: id 付与
- `src/ui/App.tsx`: viewport 系キー入力の dispatch
- `docs/design.md` / `docs/ui-design.md` / `docs/spec.md`: キーバインド表と設計更新

## トレードオフ / 受け入れる制約

- **cc-chatter 終了後、ターミナルに履歴が残らなくなる**（live frame 内で管理するため）
  - less / vim / htop などと同じ挙動。ライブ監視ツールとしては妥当
- **alt-screen の使用の有無**は実装時に判断。alt-screen を使わなくても live frame 方式で十分スクロール実装可能。画面チラつき（スクロール時の clearTerminal 回避）を抑えたい場合は alt-screen を検討する
- **ChatBubble の高さ計算の精度**: wrap 後の行数を正確に求めないと描画が崩れる。ink の `measureElement` を使うか、`wrap-ansi` で事前計算するか要検討

## マウス対応（将来）

本 issue のスコープ外。キーボード実装が落ち着いてからの追加拡張として検討。

- xterm mouse reporting（SGR プロトコル）を有効化
- ink には component 単位のクリックハンドラがないため、生 stdin をパースして Y 座標 → メッセージ index を逆引きする必要あり
- zellij / tmux 環境では mouse 設定が必要（環境依存のハマりポイント）
- 工数目安: 本 issue 完了後に +1 日程度

## 作業ブランチ

本 issue は別ブランチで実装する。ブランチ名案: `feat/tui-viewport-expand`

## 受け入れ基準

- [ ] 新着メッセージが届いてもスクロール位置が保持される
- [ ] `↑↓` / `j k` で選択を移動できる
- [ ] `Enter` / `Space` で選択中メッセージの開閉ができる
- [ ] `g` / `G` で先頭・末尾へジャンプできる
- [ ] `Ctrl+D` / `Ctrl+U` で半ページスクロールできる
- [ ] `G` 位置での末尾追従が機能する
- [ ] ターミナルリサイズに追従する
- [ ] 既存テストが全て通る
- [ ] 新規追加した reducer / hook に単体テストがある
