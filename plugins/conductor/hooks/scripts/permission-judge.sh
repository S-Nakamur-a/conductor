#!/bin/bash
# Smart permission judge for Conductor.
# Called by the Notification hook when notification_type is "permission_prompt".
#
# Reads the transcript JSONL to extract tool context, then calls claude -p
# (haiku) with PERMISSION.md rules to decide: approve / deny / ask_user.
#
# Writes the decision to .conductor/cc-permissions/<session_id>.json

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

# ── Find PERMISSION.md ──────────────────────────────────────────────
REPO_ROOT=$(cd "$(git -C "$CWD" rev-parse --git-common-dir 2>/dev/null)/.." 2>/dev/null && pwd)
if [ -z "$REPO_ROOT" ]; then
  REPO_ROOT="$CWD"
fi

PERMISSION_MD="$REPO_ROOT/PERMISSION.md"
if [ ! -f "$PERMISSION_MD" ]; then
  exit 0
fi

# ── Extract context from transcript + call claude -p ────────────────
PERMISSIONS_DIR="$REPO_ROOT/.conductor/cc-permissions"
mkdir -p "$PERMISSIONS_DIR"

python3 - "$TRANSCRIPT_PATH" "$PERMISSION_MD" "$MESSAGE" "$CWD" "$SESSION_ID" "$PERMISSIONS_DIR" <<'PYEOF'
import sys, json, subprocess, os, time

transcript_path, permission_md_path, hook_message, cwd, session_id, permissions_dir = sys.argv[1:7]

# ── Extract context from transcript JSONL ───────────────────────────
with open(transcript_path, 'r') as f:
    lines = f.readlines()

# Take last 15 lines for context
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

tool_context = json.dumps({
    'tool': tool_info or {},
    'user_message': user_message or '(unknown)',
}, ensure_ascii=False)

# ── Read PERMISSION.md ──────────────────────────────────────────────
with open(permission_md_path, 'r') as f:
    permission_rules = f.read()

# ── Build prompt ────────────────────────────────────────────────────
prompt = f"""ツール実行の許可判定を行ってください。

## ルール
{permission_rules}

## 判定対象
通知: {hook_message}
ツール詳細: {tool_context}
作業ディレクトリ: {cwd}

action は approve, deny, ask_user のいずれか。reason は日本語で1文。"""

# ── Call claude -p with structured output ───────────────────────────
json_schema = json.dumps({
    "type": "object",
    "properties": {
        "action": {"type": "string", "enum": ["approve", "deny", "ask_user"]},
        "reason": {"type": "string"}
    },
    "required": ["action", "reason"]
})

try:
    result = subprocess.run(
        [
            'claude', '-p',
            '--model', 'haiku',
            '--output-format', 'json',
            '--json-schema', json_schema,
            '--allowedTools', '',
            '--max-budget-usd', '0.10',
        ],
        input=prompt,
        capture_output=True,
        text=True,
        timeout=30,
    )

    raw = result.stdout.strip()
    outer = json.loads(raw)
    # --json-schema puts structured output in "structured_output" field
    decision = outer.get('structured_output') or {}
    if not decision:
        # Fallback: try "result" field
        inner = outer.get('result', '')
        if isinstance(inner, str) and inner:
            decision = json.loads(inner)
        elif isinstance(inner, dict):
            decision = inner

except Exception as e:
    decision = {'action': 'ask_user', 'reason': f'判定失敗: {str(e)[:80]}'}

# ── Write decision ──────────────────────────────────────────────────
output = {
    'session_id': session_id,
    'action': decision.get('action', 'ask_user'),
    'reason': decision.get('reason', ''),
    'tool': (tool_info or {}),
    'user_message': user_message or '',
    'cwd': cwd,
    'timestamp': int(time.time()),
}

output_path = os.path.join(permissions_dir, f'{session_id}.json')
with open(output_path, 'w') as f:
    json.dump(output, f, ensure_ascii=False)
PYEOF
