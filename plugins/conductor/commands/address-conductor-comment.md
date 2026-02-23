---
description: "Pending なレビューコメントを取得し、Subagent で並列処理して解決する"
---

# Conductor Resolve

Conductor でついたレビューコメント（pending）を取得し、Subagent で並列処理して解決する。

$ARGUMENTS

## 手順

### 1. Pending コメントを取得

MCP ツール `mcp__conductor__get_pending_comments` を呼び出す。
- 引数なしで全 pending コメントを取得（現在のブランチに関わらず）
- コメントがなければ「対応するコメントはありません」と報告して終了

### 2. 各コメントのスレッドを取得

各コメントに対して `mcp__conductor__get_comment_thread` を呼び出し、詳細と返信履歴を取得する。
これらは並列実行可能。

### 3. TodoWrite で一覧化

取得したコメントを TodoWrite で一覧化し、進捗を可視化する。

### 4. Subagent で並列処理

各コメントに対して **Task ツール** を使用して並列処理する。

#### suggest（変更提案）の場合

```
Task tool を使用:
- subagent_type: "general-purpose"
- prompt: |
    あなたはコードレビューの変更提案に対応する専門エージェントです。

    ## コメント情報
    - ID: <id>
    - ファイル: <file_path>
    - 行: <line_start>-<line_end>
    - 提案内容: <body>

    ## 会話履歴（repliesがある場合）
    <repliesの内容を時系列で含める>

    ## 作業手順
    1. 該当ファイルの該当行を Read で確認
    2. 周辺コンテキストも把握（前後50行程度）
    3. 提案内容に沿ってコードを修正（Edit ツール使用）
    4. 修正内容を簡潔にまとめる
    5. MCP ツール `mcp__conductor__reply_to_comment` で対応内容を報告
       - comment_id: <id>
       - body: 修正内容の説明
    6. ユーザに修正内容を確認してもらい、resolve してよいか尋ねる
    7. ユーザが承認した場合のみ、MCP ツール `mcp__conductor__resolve_comment` でコメントを解決済みにする
       - comment_id: <id>

    ## ガイドライン
    - 最小限の変更に留める
    - 既存のコードスタイルに合わせる
    - 修正の理由を返信に含める
    - 会話履歴がある場合は最新のユーザー返信に対応する

    ## 出力形式
    処理完了後、以下を報告：
    - コメントID
    - ファイル:行
    - 変更内容の要約
    - 修正したファイルのリスト
```

#### question（質問）の場合

```
Task tool を使用:
- subagent_type: "general-purpose"
- prompt: |
    あなたはコードレビューの質問に回答する専門エージェントです。

    ## コメント情報
    - ID: <id>
    - ファイル: <file_path>
    - 行: <line_start>-<line_end>
    - 質問: <body>

    ## 会話履歴（repliesがある場合）
    <repliesの内容を時系列で含める>

    ## 作業手順
    1. 該当ファイルの該当行を Read で確認
    2. 質問に答えるために必要な周辺コード・関連ファイルを調査
    3. 簡潔かつ正確な回答を作成
    4. MCP ツール `mcp__conductor__reply_to_comment` で回答を返信
       - comment_id: <id>
       - body: 回答内容
    ※ 質問への回答後は resolve しない（レビュアーが確認してから解決する）

    ## ガイドライン
    - 質問に直接答える
    - 必要に応じてコードを引用
    - 技術的な根拠を示す
    - 推測ではなく事実に基づく
    - 会話履歴がある場合は最新のユーザー返信に対応する

    ## 出力形式
    処理完了後、以下を報告：
    - コメントID
    - ファイル:行
    - 質問の要約
    - 回答の要約
```

### 5. サマリーを報告

全 Subagent の完了後、結果をまとめて報告：

```
## Conductor Resolve 完了

### 処理結果
- 提案 (suggest): N件 → 修正・resolve 済み
- 質問 (question): N件 → 回答済み

### 詳細
（各 Subagent からの報告をまとめる）
```

## 重要

- **独立したコメントは並列処理する** — 同時に複数の Task を起動
- **同一ファイルの近い行への変更**がある場合は、競合を避けるため順次処理
- Subagent が MCP ツールで直接返信するため、Conductor の UI で確認可能
- メインコンテキストはオーケストレーションのみで消費を最小化
