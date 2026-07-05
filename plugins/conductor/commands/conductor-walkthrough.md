---
description: Generate a PR walkthrough (intent -> core -> ripple -> test) and save it via the conductor MCP server.
---

# Conductor Walkthrough

現在のブランチの merge-base diff を読み、レビュアーがコードジャンプしながら追える「ウォークスルー」（ステップの列）を組み立てて、`mcp__conductor__save_walkthrough` で一度だけ保存する。

## 1. 差分と関係コードを探索する

- 現在のブランチの merge-base diff（base ブランチとの差分）を確認する。
- 差分に出てくるファイルだけでなく、呼び出し元・呼び出し先など関係するコードも必要に応じて読み、変更の全体像を把握する。

## 2. ステップを組み立てる

ステップは **intent → core → ripple → test** のストーリー順で構成する。各ステップは `file_path`（リポジトリ相対）・任意で `line_start`/`line_end`・`kind`・`title`・`body` を持つ。

- **intent**: この変更で何をしたかったか（背景・動機）。
- **core**: 何をしたくてこう変えたか、既存コードへの影響。**代替案の比較は書かない** — 深掘りが必要ならレビュアーが手動で聞く。
- **ripple**: core の変更に伴う波及的な変更（呼び出し元の更新、設定/スキーマの追随など）。
- **test**: 何の振る舞いを検証しているかの要約。これを読めば元のテスト差分を全部読まなくてよいレベルの粒度にする。

ステップ数や各種別の個数に固定の制約はない。変更の実態に合わせて過不足なく構成する。

タイトル・summary・各ステップの title / body は、ユーザーの指定があればその言語で、なければこのコマンド文書と同じ言語（日本語）で書く。

## 3. 保存する

すべてのステップが揃ったら、`mcp__conductor__save_walkthrough` を **一度だけ** 呼び出す。

- `branch`: 現在の worktree のブランチ名（`git rev-parse --abbrev-ref HEAD` で取得できるもの）
- `title`: ウォークスルー全体の一行タイトル
- `summary`: 変更全体の短い概要（intent の要約でよい）
- `steps`: 上記で組み立てたステップ列（`seq` は 0 始まりの通し番号）

保存が終わったら、ステップ数と各 kind の内訳を簡潔に報告して終了する。
