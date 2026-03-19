#!/bin/bash
# Signal to Conductor TUI that this Claude Code session is active (working).
# Sends a socket message for instant delivery, with file-based fallback.

REPO_ROOT=$(cd "$(git rev-parse --git-common-dir 2>/dev/null)/.." 2>/dev/null && pwd)
if [ -z "$REPO_ROOT" ]; then
  exit 0
fi

# Guard: only fire in Conductor-managed repos
if [ ! -d "$REPO_ROOT/.conductor" ]; then
  exit 0
fi

# Socket-based notification (instant)
SOCK="$REPO_ROOT/.conductor/cc-notify.sock"
if [ -S "$SOCK" ]; then
  echo "active $PWD" | nc -U "$SOCK" 2>/dev/null
fi

# File-based fallback (always write for safety)
ENCODED_CWD=$(echo "$PWD" | sed 's|/|__|g')
mkdir -p "$REPO_ROOT/.conductor/cc-active"
touch "$REPO_ROOT/.conductor/cc-active/$ENCODED_CWD"
rm -f "$REPO_ROOT/.conductor/cc-waiting/$ENCODED_CWD"
