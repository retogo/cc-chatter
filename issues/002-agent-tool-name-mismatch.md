# 002: Agent tool 名不整合によりマッピングが構築されず全エージェントが unknown 表示になる

- Status: **Planned**（issue 001 完了後に別ブランチで対応予定）
- Priority: High
- Depends on: issue 001（同じ `src/domain/entities/LogEntry.ts` を触るためマージ競合回避のため直列化）
- Related: `src/infrastructure/repositories/AgentMapperImpl.ts`, `src/domain/entities/LogEntry.ts`

## 症状

WATCHING 画面および SUBAGENT_SELECT 画面で、サブエージェントのタイプが全て `unknown`（🔸）として表示される。本来は `general-purpose` なら 🔹、`Explore` なら 🔍 などのタイプ別アイコンが出るはず。

## 根本原因

**Claude Code の subagent 起動ツール名は `Agent` だが、cc-chatter の `AgentMapperImpl` は `"Task"` をハードコードで探している**。

該当コード: `src/infrastructure/repositories/AgentMapperImpl.ts:89`

```ts
if (item.type === "tool_use" && (item as TaskToolUse).name === "Task") {
```

型定義: `src/domain/entities/LogEntry.ts:111-116`

```ts
export interface TaskToolUse {
    type: "tool_use";
    id: string;
    name: "Task";  // ← "Agent" ではない
    input: TaskToolInput;
}
```

そのため Task tool call として認識されるエントリが 0 件になり、`toolUseId → subagent_type` の pending マッピングが構築されない。結果として `FileSystemSessionRepository.getSubAgents()` 側の以下の式が常に `"unknown"` にフォールバックする:

```ts
// src/infrastructure/repositories/FileSystemSessionRepository.ts:442
const agentType = mapper?.getMapping(entry.agentId)?.type ?? "unknown";
```

## 実ログ調査

`~/.claude/projects/` 配下 625 ファイルを走査した結果（本 issue 起票時点）:

```
name: "Task"   → 0 件
name: "Agent"  → 573 件
```

subagent_type 内訳（上位）:

| subagent_type | 件数 |
|---|---|
| Explore | 222 |
| general-purpose | 118 |
| gitlab-operator | 79 |
| null（未指定） | 48 |
| claude-code-guide | 40 |
| firestore-operator | 30 |
| Plan | 19 |

`null`（48 件）は Agent tool 呼び出し時に `subagent_type` を省略したケース。Claude Code のデフォルトは `general-purpose` なので、**未指定なら `general-purpose` として扱う**のが妥当。

## 修正方針

### 1. `src/domain/entities/LogEntry.ts`

`TaskToolUse.name` を `"Task"` 単一リテラルから `"Task" | "Agent"` の union に拡張（旧ログとの後方互換を維持）。

```ts
export interface TaskToolUse {
    type: "tool_use";
    id: string;
    name: "Task" | "Agent";  // ← 拡張
    input: TaskToolInput;
}
```

`TaskToolInput.subagent_type` はすでに optional ではないが、未指定のログが実在するため **optional に変更**（または AgentMapperImpl 側でデフォルト補完）。後者の方がドメイン型の純粋さを保ちやすい。

### 2. `src/infrastructure/repositories/AgentMapperImpl.ts`

- L89 の tool name チェックを拡張:
  ```ts
  const toolName = (item as TaskToolUse).name;
  if (item.type === "tool_use" && (toolName === "Task" || toolName === "Agent")) {
  ```
- L91 で `subagent_type` のデフォルト補完:
  ```ts
  const subagentType = toolUse.input?.subagent_type ?? "general-purpose";
  ```

### 3. テスト追加

`src/infrastructure/repositories/AgentMapperImpl.test.ts` に以下を追加:

- `name: "Agent"` の tool_use でマッピングが構築されるケース
- `subagent_type` が `undefined` / `null` のとき `"general-purpose"` にフォールバックするケース
- `name: "Task"` の後方互換（既存テストが通ること）

### 4. ドキュメント更新

以下の記述を `Task tool` → `Agent tool` に更新（`Task` 表記が意図的な箇所以外）:

- `docs/design.md` の Claude Code ログ構造 / AgentMapperImpl 説明
- `docs/spec.md`
- 型定義のコメント（`TaskToolUse` / `TaskToolInput` など。**インターフェース名自体は変更しない**。import 箇所が広範で無駄な diff になる）

## 受け入れ基準

- [ ] 実ログ（`~/.claude/projects/` 配下）で cc-chatter を起動したとき、サブエージェントに正しいアイコン・タイプが表示される
- [ ] `subagent_type` 未指定の Agent tool 呼び出しが `general-purpose` として認識される
- [ ] `name: "Task"` の旧ログでも引き続き動作する（後方互換）
- [ ] 既存テスト + 新規テストが全て通る
- [ ] `bun run typecheck` / `bunx biome check .` / `bun test` が全て通る
- [ ] `.claude/lessons.md` に「Claude Code の subagent 起動ツール名変更への追随」という学びを記録

## 作業ブランチ

`claude/002-agent-tool-name-mismatch`

## 備考

- 影響範囲が小さく、修正 30 分〜1 時間程度で完了する見込み
- issue 001 との同時作業はファイル競合リスクがあるため直列化する（issue 001 完了 → マージ後に本 issue 着手）
