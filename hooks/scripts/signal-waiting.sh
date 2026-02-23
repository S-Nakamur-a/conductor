#!/bin/bash
# Signal to Conductor TUI that this Claude Code session is waiting for input.
# Creates a touch file in <repo-root>/.conductor/cc-waiting/<encoded-cwd>.

REPO_ROOT=$(cd "$(git rev-parse --git-common-dir 2>/dev/null)/.." 2>/dev/null && pwd)
if [ -z "$REPO_ROOT" ]; then
  exit 0
fi

WAITING_DIR="$REPO_ROOT/.conductor/cc-waiting"
mkdir -p "$WAITING_DIR"

ENCODED_CWD=$(echo "$PWD" | sed 's|/|__|g')
touch "$WAITING_DIR/$ENCODED_CWD"
