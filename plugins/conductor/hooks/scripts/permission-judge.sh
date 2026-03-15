#!/bin/bash
# Send permission prompt context to Conductor's Unix socket server.
# Called by the Notification hook when notification_type is "permission_prompt".
#
# Extracts tool context from the CC transcript and sends it to
# .conductor/server.sock for Conductor to judge via claude -p.

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

SOCK_PATH="$REPO_ROOT/.conductor/server.sock"
if [ ! -S "$SOCK_PATH" ]; then
  exit 0  # No Conductor server running.
fi

# ── Extract tool context from transcript and send to socket ─────────
python3 - "$TRANSCRIPT_PATH" "$MESSAGE" "$CWD" "$SESSION_ID" "$SOCK_PATH" <<'PYEOF'
import sys, json, socket, time

transcript_path, hook_message, cwd, session_id, sock_path = sys.argv[1:6]

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

# Send to Conductor's Unix socket.
payload = {
    'session_id': session_id,
    'message': hook_message,
    'tool': tool_info or {},
    'user_message': user_message or '',
    'cwd': cwd,
    'timestamp': int(time.time()),
}

try:
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(2)
    sock.connect(sock_path)
    sock.sendall(json.dumps(payload, ensure_ascii=False).encode('utf-8'))
    sock.shutdown(socket.SHUT_WR)
    sock.close()
except Exception:
    pass  # Conductor not running or socket error — silently ignore.
PYEOF
