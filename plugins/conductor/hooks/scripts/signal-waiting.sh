#!/bin/bash
# この Claude Code セッションが入力待ちであることを Conductor の TUI に伝える。
# 即時に届くソケット通知を送り、届かない場合に備えてファイルにも書く。

REPO_ROOT=$(cd "$(git rev-parse --git-common-dir 2>/dev/null)/.." 2>/dev/null && pwd)
if [ -z "$REPO_ROOT" ]; then
  exit 0
fi

# Conductor が管理しているリポジトリでだけ動かす
if [ ! -d "$REPO_ROOT/.conductor" ]; then
  exit 0
fi

# ソケット経由の通知 (即時)
SOCK="$REPO_ROOT/.conductor/cc-notify.sock"
if [ -S "$SOCK" ]; then
  echo "waiting $PWD" | nc -U "$SOCK" 2>/dev/null
fi

# ファイル経由のフォールバック (念のため常に書く)
ENCODED_CWD=$(echo "$PWD" | sed 's|/|__|g')
mkdir -p "$REPO_ROOT/.conductor/cc-waiting"
touch "$REPO_ROOT/.conductor/cc-waiting/$ENCODED_CWD"
rm -f "$REPO_ROOT/.conductor/cc-active/$ENCODED_CWD"
