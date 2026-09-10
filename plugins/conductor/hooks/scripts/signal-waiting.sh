#!/bin/bash
# この Claude Code セッションが入力待ちであることを Conductor の TUI に伝える。
#
# パネルの中は本体の --settings が同じ通知を送る。両方が送ると、別イベント同士が
# 追い越したときに遅い方が古い状態を上書きしてストリップが嘘をつく。
if [ -n "$CONDUCTOR_PANEL_ID" ]; then
  exit 0
fi

REPO_ROOT=$(cd "$(git rev-parse --git-common-dir 2>/dev/null)/.." 2>/dev/null && pwd)
if [ -z "$REPO_ROOT" ]; then
  exit 0
fi

# ソケットはリポジトリの外にある。居場所は Conductor が bind したときに書き残す。
POINTER="$REPO_ROOT/.conductor/cc-notify.path"
if [ ! -f "$POINTER" ]; then
  exit 0
fi
SOCK=$(head -n 1 "$POINTER")

if [ -S "$SOCK" ]; then
  echo "waiting $PWD" | nc -U "$SOCK" 2>/dev/null
fi
