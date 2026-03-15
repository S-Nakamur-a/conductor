#!/bin/bash
# Save permission prompt context for Conductor to process.
# Called by the Notification hook when notification_type is "permission_prompt".
#
# Extracts tool context from the CC transcript and writes a pending
# judgment file to .conductor/cc-permissions/<session_id>.json.
# Conductor reads this file and decides whether to call claude -p.

set -euo pipefail

# ── Read hook stdin (JSON) and extract fields ───────────────────────
INPUT=$(cat)

eval "$(echo "$INPUT" | python3 -c "
import sys, json, shlex
d = json.load(sys.stdin)
for k in ('notification_type', 'session_id', 'transcript_path', 'cwd', 'message'):
    print(f'{k.upper()}={shlex.quote(d.get(k, \"\"))}')
")"

if [ "$NOTIFICATION_TYPE" != "permission_prompt" ]; then
  exit 0
fi

if [ -z "$TRANSCRIPT_PATH" ] || [ ! -f "$TRANSCRIPT_PATH" ]; then
  exit 0
fi

# ── Resolve repo root ──────────────────────────────────────────────
REPO_ROOT=$(cd "$(git -C "$CWD" rev-parse --git-common-dir 2>/dev/null)/.." 2>/dev/null && pwd)
if [ -z "$REPO_ROOT" ]; then
  REPO_ROOT="$CWD"
fi

PERMISSIONS_DIR="$REPO_ROOT/.conductor/cc-permissions"
mkdir -p "$PERMISSIONS_DIR"

# ── Extract tool context from transcript and write pending file ─────
python3 - "$TRANSCRIPT_PATH" "$MESSAGE" "$CWD" "$SESSION_ID" "$PERMISSIONS_DIR" <<'PYEOF'
import sys, json, os, time

transcript_path, hook_message, cwd, session_id, permissions_dir = sys.argv[1:6]

# Extract context from last 15 lines of the transcript JSONL.
with open(transcript_path, 'r') as f:
    lines = f.readlines()

tail_lines = lines[-15:]
tool_info = None
user_message = None

for line in reversed(tail_lines):
    try:
        obj = json.loads(line.strip())
    except:
        continue

    if tool_info is None and obj.get('type') == 'assistant':
        content = obj.get('message', {}).get('content', [])
        for block in content:
            if block.get('type') == 'tool_use':
                tool_info = {
                    'tool_name': block.get('name', ''),
                    'tool_input': block.get('input', {}),
                }
                break

    if user_message is None and obj.get('type') == 'user':
        msg_content = obj.get('message', {}).get('content', '')
        if isinstance(msg_content, str) and msg_content:
            user_message = msg_content

    if tool_info and user_message:
        break

# Write a pending judgment file for Conductor to pick up.
output = {
    'status': 'pending',
    'session_id': session_id,
    'message': hook_message,
    'tool': tool_info or {},
    'user_message': user_message or '',
    'cwd': cwd,
    'timestamp': int(time.time()),
}

output_path = os.path.join(permissions_dir, f'{session_id}.json')
with open(output_path, 'w') as f:
    json.dump(output, f, ensure_ascii=False)
PYEOF
