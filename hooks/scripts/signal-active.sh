#!/bin/bash
# Signal to Conductor TUI that this Claude Code session is active again.
# Removes the touch file from <repo-root>/.conductor/cc-waiting/<encoded-cwd>.

REPO_ROOT=$(cd "$(git rev-parse --git-common-dir 2>/dev/null)/.." 2>/dev/null && pwd)
if [ -z "$REPO_ROOT" ]; then
  exit 0
fi

WAITING_DIR="$REPO_ROOT/.conductor/cc-waiting"
ENCODED_CWD=$(echo "$PWD" | sed 's|/|__|g')
rm -f "$WAITING_DIR/$ENCODED_CWD"
