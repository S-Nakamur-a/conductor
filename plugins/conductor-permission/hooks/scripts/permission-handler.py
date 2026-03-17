#!/usr/bin/env python3
"""
PermissionRequest hook handler for Claude Code.

Reads hook input from stdin, decides whether to allow/deny the tool execution,
and outputs the decision as JSON to stdout.

Features:
- Manual mode: shows macOS osascript dialog with Allow / Deny
- Auto mode: uses a pluggable LLM handler with PERMISSION.md rules to judge

Requires: python3 (no external dependencies)
Optional: LLM handler file for auto_permission mode (default: llm-handler-gemini.py)

Configuration via environment variables:
  AUTO_PERMISSION=1              Enable LLM auto-judgment (default: off)
  PERMISSION_LLM_HANDLER=...    Path to a Python file with a judge(system_prompt, user_message) -> str function.
                                 (default: scripts/llm-handler-gemini.py next to this script)
"""

import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path


def main():
    raw = sys.stdin.read()
    try:
        hook_input = json.loads(raw)
    except json.JSONDecodeError:
        # Invalid input — return empty (fall through to native dialog).
        sys.exit(0)

    tool_name = hook_input.get("tool_name", "")
    tool_input = hook_input.get("tool_input", {})
    cwd = hook_input.get("cwd", "")
    suggestions = hook_input.get("permission_suggestions", [])

    auto_permission = os.environ.get("AUTO_PERMISSION", "") == "1"

    if auto_permission:
        response = judge_permission(hook_input, cwd)
    else:
        response = ask_user_permission(tool_name, tool_input, cwd, suggestions)

    if response is not None:
        print(json.dumps(response))


def make_allow():
    return {
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": {"behavior": "allow"},
        }
    }


# TODO: Re-enable once updatedPermissions works reliably.
# def make_allow_with_permissions(permissions):
#     return {
#         "hookSpecificOutput": {
#             "hookEventName": "PermissionRequest",
#             "decision": {
#                 "behavior": "allow",
#                 "updatedPermissions": permissions,
#             },
#         }
#     }


def make_deny(reason=""):
    return {
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": {"behavior": "deny", "message": reason},
        }
    }


# ---------------------------------------------------------------------------
# Manual permission via osascript
# ---------------------------------------------------------------------------


def ask_user_permission(tool_name, tool_input, cwd, suggestions):
    tool_summary = summarize_tool_input(tool_name, tool_input)
    description = tool_input.get("description", "")

    # Build dialog text.
    text_parts = [f"{tool_name}: {tool_summary}"]
    if description:
        text_parts.append(description)
    text_parts.append(f"cwd: {cwd}")
    # osascript: use `" & return & "` for newlines.
    dialog_text = '" & return & "'.join(
        s.replace("\\", "\\\\").replace('"', '\\"') for s in text_parts
    )

    # Build buttons: Allow, Deny.
    # TODO: Always allow is disabled until updatedPermissions works reliably.
    # for i, suggestion in enumerate(suggestions):
    #     if len(buttons) >= 2:
    #         break
    #     label = format_suggestion_label(suggestion)[:40]
    #     buttons.append((label, i))
    buttons = [("Allow", None)]
    buttons.append(("Deny", None))

    buttons_str = ", ".join(
        f'"{b[0].replace(chr(92), chr(92)*2).replace(chr(34), chr(92)+chr(34))}"'
        for b in buttons
    )
    default_button = buttons[0][0].replace("\\", "\\\\").replace('"', '\\"')

    script = (
        f'display dialog "{dialog_text}" '
        f'with title "Permission" '
        f"buttons {{{buttons_str}}} "
        f'default button "{default_button}"'
    )

    try:
        result = subprocess.run(
            ["osascript", "-e", script],
            capture_output=True,
            text=True,
            timeout=55,
        )
    except subprocess.TimeoutExpired:
        return make_deny("Permission dialog timeout")
    except Exception:
        return make_deny("Failed to show permission dialog")

    if result.returncode != 0:
        return make_deny("User cancelled permission dialog")

    pressed = result.stdout.strip().removeprefix("button returned:").strip()

    if pressed == "Deny":
        return make_deny("User denied")

    # TODO: Always allow handling is disabled until updatedPermissions works reliably.
    # See: https://github.com/anthropics/claude-code/issues/24540
    # for label, suggestion_idx in buttons:
    #     if label == pressed and suggestion_idx is not None:
    #         suggestion = suggestions[suggestion_idx]
    #         apply_suggestion_to_settings(suggestion, cwd)
    #         return make_allow_with_permissions([suggestion])

    return make_allow()


# ---------------------------------------------------------------------------
# Auto permission via LLM API
# ---------------------------------------------------------------------------


def judge_permission(hook_input, cwd):
    tool_name = hook_input.get("tool_name", "")
    tool_input = hook_input.get("tool_input", {})

    # Resolve repo root from cwd.
    repo_root = cwd
    try:
        git_dir = subprocess.run(
            ["git", "-C", cwd, "rev-parse", "--git-common-dir"],
            capture_output=True,
            text=True,
        )
        if git_dir.returncode == 0:
            repo_root = str(
                Path(git_dir.stdout.strip()).resolve().parent
            )
    except Exception:
        pass

    home = os.environ.get("HOME", "")

    # Read PERMISSION.md rules.
    project_rules = _read_file(Path(repo_root) / ".claude" / "PERMISSION.md")
    global_rules = _read_file(Path(home) / ".claude" / "PERMISSION.md")
    settings_perms = read_settings_permissions(repo_root, home)

    if not project_rules and not global_rules and not settings_perms:
        return make_allow()

    # Build rules section.
    if global_rules and project_rules:
        rules_section = (
            "以下の2つのルールがあります。矛盾する場合はプロジェクトルールを優先してください。\n\n"
            f"### グローバルルール (~/.claude/PERMISSION.md)\n{global_rules}\n\n"
            f"### プロジェクトルール (.claude/PERMISSION.md) ※こちらが優先\n{project_rules}"
        )
    elif project_rules:
        rules_section = project_rules
    else:
        rules_section = global_rules

    if settings_perms:
        rules_section += (
            "\n\n### settings.json / settings.local.json の許可・拒否パターン\n"
            "ユーザーが明示的に設定した allow/deny パターンです。これらに合致する場合は優先してください。\n"
            f"{settings_perms}"
        )

    tool_context = json.dumps(
        {"tool_name": tool_name, "tool_input": tool_input}, ensure_ascii=False
    )

    prompt = (
        f"ツール実行の許可判定を行ってください。\n\n"
        f"## ルール\n{rules_section}\n\n"
        f"## 判定対象\nツール詳細: {tool_context}\n"
        f"作業ディレクトリ: {cwd}\n\n"
        f"action は allow, deny, ask_user のいずれか。\n"
        f"判断できない場合は ask_user を選択。reason は日本語で1文。"
    )

    system_prompt = (
        "You are a permission judgment assistant. Output JSON only. "
        'You must output a JSON object with two fields: '
        '"action" (one of "allow", "deny", "ask_user") and '
        '"reason" (a brief explanation in Japanese).'
    )

    # Load LLM handler: custom (PERMISSION_LLM_HANDLER) or built-in Gemini.
    llm_judge = load_llm_handler()
    if not llm_judge:
        return ask_user_permission(
            tool_name, tool_input, cwd, hook_input.get("permission_suggestions", [])
        )

    try:
        raw = llm_judge(system_prompt, prompt)
    except Exception as e:
        sys.stderr.write(f"LLM handler error: {e}\n")
        return ask_user_permission(
            tool_name, tool_input, cwd, hook_input.get("permission_suggestions", [])
        )

    # Strip markdown code fences.
    stripped = raw.strip()
    for prefix in ("```json\n", "```json", "```\n", "```"):
        if stripped.startswith(prefix):
            stripped = stripped[len(prefix) :]
            break
    for suffix in ("\n```", "```"):
        if stripped.endswith(suffix):
            stripped = stripped[: -len(suffix)]
            break
    stripped = stripped.strip()

    try:
        parsed = json.loads(stripped)
    except json.JSONDecodeError:
        return ask_user_permission(
            tool_name, tool_input, cwd, hook_input.get("permission_suggestions", [])
        )

    action = parsed.get("action", "ask_user")
    reason = parsed.get("reason", "")

    if action == "allow":
        return make_allow()
    elif action == "deny":
        return make_deny(reason)
    else:
        return ask_user_permission(
            tool_name, tool_input, cwd, hook_input.get("permission_suggestions", [])
        )


# ---------------------------------------------------------------------------
# LLM handler loading
# ---------------------------------------------------------------------------


def load_llm_handler():
    """Return a callable (system_prompt, user_message) -> str.

    Loads the judge() function from the Python file specified by
    PERMISSION_LLM_HANDLER. Defaults to llm-handler-gemini.py in the
    same directory as this script.
    """
    default_handler = Path(__file__).parent / "llm-handler-gemini.py"
    handler_path = os.environ.get("PERMISSION_LLM_HANDLER", str(default_handler))
    path = Path(handler_path).expanduser().resolve()

    if not path.is_file():
        sys.stderr.write(f"LLM handler not found: {path}\n")
        return None
    try:
        spec = importlib.util.spec_from_file_location("_llm_handler", path)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
    except Exception as e:
        sys.stderr.write(f"Failed to load LLM handler {path}: {e}\n")
        return None
    judge_fn = getattr(mod, "judge", None)
    if not callable(judge_fn):
        sys.stderr.write(f"LLM handler {path} has no callable judge() function\n")
        return None
    return judge_fn


# ---------------------------------------------------------------------------
# settings.local.json workaround (disabled)
# ---------------------------------------------------------------------------

# TODO: Re-enable once updatedPermissions works reliably.
# See: https://github.com/anthropics/claude-code/issues/24540
#
# def apply_suggestion_to_settings(suggestion, cwd):
#     settings_path = Path(cwd) / ".claude" / "settings.local.json"
#     try:
#         settings = json.loads(settings_path.read_text())
#     except Exception:
#         settings = {}
#     permissions = settings.setdefault("permissions", {})
#     allow_list = permissions.setdefault("allow", [])
#     stype = suggestion.get("type", "")
#     if stype == "addDirectories":
#         dirs = suggestion.get("directories", [])
#         for d in dirs:
#             for rule in [
#                 f"Read({d}/**)", f"Write({d}/**)",
#                 f"Edit({d}/**)", f"Bash({d}/**)",
#             ]:
#                 if rule not in allow_list:
#                     allow_list.append(rule)
#     elif stype == "toolAlwaysAllow":
#         tool = suggestion.get("tool", "")
#         if tool and tool not in allow_list:
#             allow_list.append(tool)
#     else:
#         return
#     settings_path.parent.mkdir(parents=True, exist_ok=True)
#     settings_path.write_text(json.dumps(settings, indent=2, ensure_ascii=False) + "\n")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def summarize_tool_input(tool_name, tool_input):
    if tool_name == "Bash":
        return (tool_input.get("command", "") or "")[:120]
    elif tool_name in ("Write", "Edit", "Read"):
        return tool_input.get("file_path", "") or ""
    else:
        return json.dumps(tool_input, ensure_ascii=False)[:120]


def format_suggestion_label(suggestion):
    tool = suggestion.get("tool")
    if tool:
        return f"Always allow ({tool})"
    return "Always allow"


def read_settings_permissions(repo_root, home):
    files = [
        ("~/.claude/settings.json", Path(home) / ".claude" / "settings.json"),
        (
            "~/.claude/settings.local.json",
            Path(home) / ".claude" / "settings.local.json",
        ),
        (".claude/settings.json", Path(repo_root) / ".claude" / "settings.json"),
        (
            ".claude/settings.local.json",
            Path(repo_root) / ".claude" / "settings.local.json",
        ),
    ]
    parts = []
    for label, path in files:
        try:
            data = json.loads(path.read_text())
        except Exception:
            continue
        perms = data.get("permissions", {})
        allow = perms.get("allow")
        deny = perms.get("deny")
        if allow is None and deny is None:
            continue
        section = f"**{label}**\n"
        if allow:
            items = ", ".join(f"`{s}`" for s in allow if isinstance(s, str))
            section += f"- allow: {items}\n"
        if deny:
            items = ", ".join(f"`{s}`" for s in deny if isinstance(s, str))
            section += f"- deny: {items}\n"
        parts.append(section)
    return "\n".join(parts)


def _read_file(path):
    try:
        return Path(path).read_text()
    except Exception:
        return ""


if __name__ == "__main__":
    main()
