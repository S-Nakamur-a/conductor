#!/usr/bin/env python3
"""revidere: git diff を、変更行が 1 行も漏れないレビューにするための道具。

    revidere.py prepare [--base REF]   作業 dir を作り、差分と変更位置の台帳を置く
    revidere.py check DIR [--retry]    DIR/review.json が台帳を漏れなく説明しているか
    revidere.py render DIR             DIR/review.html を書く
    revidere.py published DIR URL      publish した Artifact の URL を DIR/meta.json に書き足す

標準ライブラリだけで Python 3.9 から動く。
"""

from __future__ import annotations

import argparse
import bisect
import html
import json
import os
import subprocess
import sys
import tempfile
from urllib.parse import quote, urlsplit

IMPORTANCE = ["core", "ripple", "follow", "minor"]
IMPORTANCE_JA = {"core": "本題", "ripple": "影響あり", "follow": "影響なし", "minor": "おまけ"}
OVERVIEW_FIELDS = ["problem", "change", "mechanism", "placement", "scope"]
OVERVIEW_JA = {
    "problem": "問題",
    "change": "変更",
    "mechanism": "仕組み",
    "placement": "置き場所",
    "scope": "範囲",
}
CONTEXT = 3


class Failure(Exception):
    """利用者に見せて exit 2 で終わる失敗。"""


# ---- git ----


def git(root: str, *args: str, ok=(0,)) -> bytes:
    cmd = ["git", "--no-optional-locks", "-c", "core.quotepath=false", "-C", root, *args]
    p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if p.returncode not in ok:
        err = p.stderr.decode("utf-8", "replace").strip()
        raise Failure(f"git {' '.join(args)} が失敗した: {err}")
    return p.stdout


def git_text(root: str, *args: str) -> str:
    return git(root, *args).decode("utf-8", "replace").strip()


def repo_root(cwd: str) -> str:
    return git_text(cwd, "rev-parse", "--show-toplevel")


def commit_of(root: str, rev: str) -> str:
    out = git(root, "rev-parse", "--verify", "--quiet", "--end-of-options", rev + "^{commit}", ok=(0, 1))
    oid = out.decode().strip()
    if not oid:
        raise Failure(f"{rev} はコミットとして解決できない")
    return oid


def exists(root: str, rev: str) -> bool:
    try:
        commit_of(root, rev)
        return True
    except Failure:
        return False


def guess_base(root: str) -> str:
    """origin/HEAD、origin/main、origin/master、main、master の順。

    ローカルを後に回すのは、fetch がローカルの main を進めないから。取り残された
    main を起点にすると、その間に main へ入った他人の変更までレビュー対象になる。
    """
    head = git(root, "symbolic-ref", "--quiet", "refs/remotes/origin/HEAD", ok=(0, 1)).decode().strip()
    candidates = []
    if head.startswith("refs/remotes/"):
        candidates.append(head[len("refs/remotes/"):])
    candidates += ["origin/main", "origin/master", "main", "master"]
    for c in candidates:
        if exists(root, c):
            return c
    raise Failure("ベースを推定できない。--base で指定してほしい")


DIFF_FLAGS = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    # 利用者の diff.mnemonicPrefix / diff.noprefix で見出しの a/ b/ が変わるのを止める。
    "--src-prefix=a/",
    "--dst-prefix=b/",
]


def untracked(root: str) -> list[str]:
    out = git(root, "ls-files", "--others", "--exclude-standard", "-z")
    return [p for p in out.decode("utf-8", "replace").split("\0") if p]


def diff_text(root: str, base_oid: str) -> str:
    """base から作業ツリーまで。未追跡ファイルは 1 件ずつ --no-index で起こして繋ぐ。

    git add -N なら 1 回で済むが、利用者の index を書き換えるので採らない。
    """
    # git diff は stat だけ変わったファイルを見ると、--no-optional-locks を付けても
    # 利用者の index を書き戻す (git 2.45 で実測)。diff-index は index を読むだけ。
    out = git(root, "diff-index", "-p", "--find-renames", *DIFF_FLAGS, base_oid, "--")
    for path in untracked(root):
        # 差分があると 1 を返す。それ以外 (入れ子のリポジトリなど) は台帳に載せない。
        p = subprocess.run(
            ["git", "--no-optional-locks", "-c", "core.quotepath=false", "-C", root,
             "diff", *DIFF_FLAGS, "--no-index", "--", "/dev/null", path],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        if p.returncode in (0, 1):
            out += p.stdout
        else:
            err = p.stderr.decode("utf-8", "replace").strip()
            print(f"revidere: 未追跡の {path} を台帳に載せない: {err}", file=sys.stderr)
    return out.decode("utf-8", "replace")


def dirty_paths(root: str) -> list[str]:
    # --name-only は stat だけ変わったファイルも挙げるので、中身の差分から取る。
    patch = git(root, "diff-index", "-p", "--no-renames", *DIFF_FLAGS, "HEAD", "--")
    paths = [f["path"] for f in parse_diff(patch.decode("utf-8", "replace"))]
    return sorted(set(paths + untracked(root)))


def on_origin(root: str, oid: str) -> bool:
    out = git_text(root, "branch", "-r", "--format=%(refname:short)", "--contains", oid)
    return any(line.startswith("origin/") for line in out.splitlines())


def github_repo(url: str) -> str | None:
    """origin の URL から owner/repo。認証情報やホスト別名は持ち出さない。"""
    url = url.strip()
    if "://" in url:
        parts = urlsplit(url)
        host = (parts.hostname or "").lower()
        path = parts.path
    elif ":" in url and not url.startswith("/"):
        host_part, path = url.split(":", 1)
        host = host_part.rsplit("@", 1)[-1].lower()
    else:
        return None
    if host not in ("github.com", "ssh.github.com", "www.github.com"):
        return None
    segments = [s for s in path.strip("/").split("/") if s]
    if len(segments) != 2:
        return None
    owner, repo = segments
    if repo.endswith(".git"):
        repo = repo[: -len(".git")]
    return f"{owner}/{repo}" if owner and repo else None


def origin_url(root: str) -> str | None:
    out = git(root, "remote", "get-url", "origin", ok=(0, 2, 128))
    return out.decode("utf-8", "replace").strip() or None


# ---- unified diff ----


def unquote_c(s: str) -> str:
    if not (len(s) >= 2 and s[0] == '"' and s[-1] == '"'):
        return s
    body = s[1:-1]
    out = bytearray()
    simple = {"a": 7, "b": 8, "t": 9, "n": 10, "v": 11, "f": 12, "r": 13, '"': 34, "\\": 92}
    i = 0
    while i < len(body):
        c = body[i]
        if c != "\\" or i + 1 == len(body):
            out += c.encode("utf-8")
            i += 1
            continue
        nxt = body[i + 1]
        if nxt in simple:
            out.append(simple[nxt])
            i += 2
        elif body[i + 1 : i + 4].isdigit() and len(body[i + 1 : i + 4]) == 3:
            out.append(int(body[i + 1 : i + 4], 8) & 0xFF)
            i += 4
        else:
            out += nxt.encode("utf-8")
            i += 2
    return out.decode("utf-8", "replace")


def side_path(token: str) -> str | None:
    # パスの中のタブは引用の中で \t になるので、生のタブは見出しの区切りだけ。
    token = unquote_c(token.split("\t", 1)[0])
    if token == "/dev/null":
        return None
    for prefix in ("a/", "b/"):
        if token.startswith(prefix):
            return token[len(prefix):]
    return token


def split_git_header(rest: str) -> tuple[str | None, str | None]:
    """`diff --git a/X b/Y` の残り。---/+++ も rename 行も無い変更のための後詰め。"""
    if rest.startswith('"'):
        end = 1
        while end < len(rest):
            if rest[end] == "\\":
                end += 2
                continue
            if rest[end] == '"':
                break
            end += 1
        return side_path(rest[: end + 1]), side_path(rest[end + 2 :])
    cuts = [i for i in range(len(rest)) if rest.startswith(" b/", i) or rest.startswith(' "b/', i)]
    for i in cuts:
        left, right = rest[:i], rest[i + 1 :]
        if side_path(left) == side_path(right):
            return side_path(left), side_path(right)
    if cuts:
        return side_path(rest[: cuts[-1]]), side_path(rest[cuts[-1] + 1 :])
    return None, None


def hunk_header(line: str):
    if not line.startswith("@@ "):
        return None
    close = line.find(" @@", 3)
    if close < 0:
        return None
    spec = line[3:close].split()
    if len(spec) < 2 or not spec[0].startswith("-") or not spec[1].startswith("+"):
        return None

    def start_count(s):
        a, _, b = s.partition(",")
        return int(a), int(b) if b else 1

    try:
        old_start, old_count = start_count(spec[0][1:])
        new_start, new_count = start_count(spec[1][1:])
    except ValueError:
        return None
    return {
        "header": line[close + 3 :].strip(),
        "old_start": old_start,
        "new_start": new_start,
        "old_left": old_count,
        "new_left": new_count,
        "lines": [],
    }


def parse_diff(text: str) -> list[dict]:
    """行は hunk の残り行数で数えて読む。hunk の中では `--- ` で始まる削除行も中身。"""
    files: list[dict] = []
    cur = None
    hunk = None
    old_no = new_no = 0
    for raw in text.split("\n"):
        if hunk is not None and (hunk["old_left"] > 0 or hunk["new_left"] > 0):
            if raw.startswith("\\"):
                continue
            tag = raw[:1] or " "
            body = raw[1:]
            if tag == "+":
                hunk["lines"].append(("+", None, new_no, body))
                new_no += 1
                hunk["new_left"] -= 1
                continue
            if tag == "-":
                hunk["lines"].append(("-", old_no, None, body))
                old_no += 1
                hunk["old_left"] -= 1
                continue
            if tag == " ":
                hunk["lines"].append((" ", old_no, new_no, body))
                old_no += 1
                new_no += 1
                hunk["old_left"] -= 1
                hunk["new_left"] -= 1
                continue
            hunk = None
        if raw.startswith("diff --git "):
            a, b = split_git_header(raw[len("diff --git "):])
            cur = {"path": b or a or "", "old_path": a, "new_path": b, "kind": "modified", "hunks": [],
                   "renamed": False, "mode": False, "binary": False}
            files.append(cur)
            hunk = None
            continue
        if cur is None:
            continue
        h = hunk_header(raw)
        if h is not None:
            cur["hunks"].append(h)
            hunk = h
            old_no, new_no = h["old_start"], h["new_start"]
            continue
        if raw.startswith("\\"):
            continue
        if raw.startswith("--- "):
            cur["old_path"] = side_path(raw[4:])
        elif raw.startswith("+++ "):
            cur["new_path"] = side_path(raw[4:])
        elif raw.startswith("rename from "):
            cur["old_path"] = unquote_c(raw[len("rename from "):])
            cur["renamed"] = True
        elif raw.startswith("rename to "):
            cur["new_path"] = unquote_c(raw[len("rename to "):])
            cur["renamed"] = True
        elif raw.startswith("new file mode "):
            cur["kind"] = "added"
            cur["old_path"] = None
        elif raw.startswith("deleted file mode "):
            cur["kind"] = "deleted"
            cur["new_path"] = None
        elif raw.startswith("old mode ") or raw.startswith("new mode "):
            cur["mode"] = True
        elif raw.startswith("Binary files ") or raw.startswith("GIT binary patch"):
            cur["binary"] = True
    for f in files:
        finish_file(f)
    return files


def finish_file(f: dict) -> None:
    old, new = f["old_path"], f["new_path"]
    if old is None and new is not None:
        f["path"], f["kind"] = new, "added"
    elif new is None and old is not None:
        f["path"], f["kind"] = old, "deleted"
    elif old is not None and new is not None:
        f["path"] = new
        if old != new:
            f["kind"] = "renamed"
    if f["kind"] == "modified" and not f["hunks"] and f["mode"] and not f["binary"]:
        f["kind"] = "mode-only"
    f["old_path"] = old if old is not None and old != f["path"] else None
    for key in ("new_path", "renamed", "mode"):
        f.pop(key)
    for h in f["hunks"]:
        h.pop("old_left")
        h.pop("new_left")


# ---- 台帳 ----

# 位置は (path, side, line)。side は new / old / file で、file のとき line は 0。
# 削除行の位置も新パスで数える (rename + 編集の old 行は新パスで指す)。


def file_positions(f: dict) -> list[tuple]:
    out = []
    for h in f["hunks"]:
        for tag, old_no, new_no, _ in h["lines"]:
            if tag == "+":
                out.append((f["path"], "new", new_no))
            elif tag == "-":
                out.append((f["path"], "old", old_no))
    return out or [(f["path"], "file", 0)]


def ledger_of(files: list[dict]) -> list[tuple]:
    return sorted({p for f in files for p in file_positions(f)})


def fold(lines: list[int]) -> str:
    parts = []
    i = 0
    while i < len(lines):
        j = i
        while j + 1 < len(lines) and lines[j + 1] == lines[j] + 1:
            j += 1
        parts.append(str(lines[i]) if i == j else f"{lines[i]}-{lines[j]}")
        i = j + 1
    return ", ".join(parts)


def ledger_summary(files: list[dict]) -> str:
    out = []
    for f in files:
        out.append(f"{f['path']} [{'binary' if f['binary'] else f['kind']}]")
        if f["old_path"]:
            out.append(f"  (renamed from {f['old_path']})")
        positions = file_positions(f)
        new = sorted({n for _, s, n in positions if s == "new"})
        old = sorted({n for _, s, n in positions if s == "old"})
        if not new and not old:
            out.append("  file (no line-level change)")
        if new:
            out.append(f"  new: {fold(new)}")
        if old:
            out.append(f"  old: {fold(old)}")
    return "\n".join(out) + "\n"


# ---- review.json ----


def is_line_number(v) -> bool:
    # bool は int の仲間なので先に弾く。true が 1 行目に化ける。
    return type(v) is int and v >= 1


def nonblank(v) -> bool:
    return isinstance(v, str) and v.strip() != ""


def validate(r) -> None:
    if not isinstance(r, dict):
        raise Failure("review.json の一番外が JSON オブジェクトでない")
    o = r.get("overview")
    if not isinstance(o, dict):
        raise Failure("overview が無い")
    for name in OVERVIEW_FIELDS:
        if not nonblank(o.get(name)):
            raise Failure(f"overview.{name} が空")
    sections = r.get("sections")
    if not isinstance(sections, list) or not sections:
        raise Failure("sections が空")
    for i, s in enumerate(sections):
        where = f"sections[{i}]"
        if not isinstance(s, dict):
            raise Failure(f"{where} がオブジェクトでない")
        if not nonblank(s.get("title")):
            raise Failure(f"{where}.title が空")
        if not isinstance(s.get("body"), str):
            raise Failure(f"{where}（{s['title']}）に body が無い")
        if s.get("importance") not in IMPORTANCE:
            raise Failure(f"{where}.importance が {IMPORTANCE} のどれでもない: {s.get('importance')!r}")
        if not nonblank(s.get("reason")):
            raise Failure(f"{where}（{s['title']}）に reason が無い")
        ranges = s.get("ranges")
        if not isinstance(ranges, list):
            raise Failure(f"{where}.ranges が配列でない")
        for j, rg in enumerate(ranges):
            validate_range(rg, f"{where}.ranges[{j}]")
        relations = s.get("relations", [])
        if not isinstance(relations, list):
            raise Failure(f"{where}.relations が配列でない")
        for j, rel in enumerate(relations):
            if not isinstance(rel, dict) or not isinstance(rel.get("to"), str) or not isinstance(rel.get("reason"), str):
                raise Failure(f"{where}.relations[{j}] に to と reason が無い")
            if not isinstance(rel.get("primary", False), bool):
                raise Failure(f"{where}.relations[{j}].primary が true / false でない")
        concerns = s.get("concerns", [])
        if not isinstance(concerns, list) or not all(nonblank(c) for c in concerns):
            raise Failure(f"{where}（{s['title']}）の concerns が空でない文字列の配列でない")
    impacts = r.get("impacts", [])
    if not isinstance(impacts, list):
        raise Failure("impacts が配列でない")
    for i, im in enumerate(impacts):
        where = f"impacts[{i}]"
        if not isinstance(im, dict) or not nonblank(im.get("feature")):
            raise Failure(f"{where}.feature が空")
        if im.get("confidence") not in ("fact", "guess"):
            raise Failure(f"{where}.confidence が fact / guess でない: {im.get('confidence')!r}")
        for key in ("change", "verify"):
            if not isinstance(im.get(key), str):
                raise Failure(f"{where}.{key} が無い")
        if im["confidence"] == "fact" and not im["change"].strip():
            raise Failure(f"{where} は fact なのに change が空")


def validate_range(rg, where: str) -> None:
    if not isinstance(rg, dict) or not nonblank(rg.get("path")):
        raise Failure(f"{where}.path が空")
    side = rg.get("side")
    if side not in ("new", "old", "file"):
        raise Failure(f"{where}.side が new / old / file でない: {side!r}")
    if side == "file":
        return
    start, end = rg.get("start"), rg.get("end", None)
    if not is_line_number(start) or (end is not None and not is_line_number(end)) or (end is not None and start > end):
        raise Failure(f"{where} の行範囲が壊れている（{rg['path']} start={start!r} end={end!r}）")


def load_review(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as fh:
            r = json.load(fh)
    except FileNotFoundError:
        raise Failure(f"{path} が無い")
    except json.JSONDecodeError as e:
        raise Failure(f"{path} が JSON として読めない: {e}")
    validate(r)
    return r


def sorted_sections(r: dict) -> list[dict]:
    return sorted(r["sections"], key=lambda s: IMPORTANCE.index(s["importance"]))


# ---- 説明もれ検査 ----


def range_hits(rg: dict, index: dict) -> tuple[list[tuple], list[tuple]]:
    """range が指す位置のうち台帳にあるものと、台帳に無い区間 (path, side, start, end)。

    1 行ずつ展開しないのは、新規ファイル全体を end=10^9 のように雑に指す答えでも
    止まらないため。
    """
    path, side = rg["path"], rg["side"]
    if side == "file":
        pos = (path, "file", 0)
        return ([pos], []) if pos in index["all"] else ([], [(path, "file", 0, 0)])
    start = rg["start"]
    end = rg.get("end") or start
    lines = index["lines"].get((path, side), [])
    lo, hi = bisect.bisect_left(lines, start), bisect.bisect_right(lines, end)
    inside = lines[lo:hi]
    gaps, at = [], start
    for n in inside:
        if n > at:
            gaps.append((path, side, at, n - 1))
        at = n + 1
    if at <= end:
        gaps.append((path, side, at, end))
    return [(path, side, n) for n in inside], gaps


def ledger_index(ledger: list[tuple]) -> dict:
    lines: dict = {}
    for path, side, n in ledger:
        if side != "file":
            lines.setdefault((path, side), []).append(n)
    for v in lines.values():
        v.sort()
    return {"all": set(ledger), "lines": lines}


def coverage(ledger: list[tuple], sections: list[dict]) -> dict:
    index = ledger_index(ledger)
    hits: dict = {}
    per_section = []
    for s in sections:
        mine: set = set()
        unknown: list = []
        for rg in s["ranges"]:
            found, gaps = range_hits(rg, index)
            mine.update(found)
            unknown += gaps
        # 同じ節が範囲を重ねて書いても、取り合いとは数えない。
        for p in mine:
            hits[p] = hits.get(p, 0) + 1
        per_section.append({"positions": mine, "unknown": merge_gaps(unknown)})
    return {
        "total": len(ledger),
        "classified": sum(1 for n in hits.values() if n == 1),
        "unclassified": [p for p in ledger if p not in hits],
        "conflicts": sorted(p for p, n in hits.items() if n > 1),
        "unknown": merge_gaps([g for ps in per_section for g in ps["unknown"]]),
        "sections": per_section,
    }


def merge_gaps(gaps: list[tuple]) -> list[tuple]:
    out: list = []
    for g in sorted(set(gaps)):
        if out and out[-1][:2] == g[:2] and g[1] != "file" and g[2] <= out[-1][3] + 1:
            out[-1] = (g[0], g[1], out[-1][2], max(out[-1][3], g[3]))
        else:
            out.append(g)
    return out


def is_complete(cov: dict) -> bool:
    return not (cov["unclassified"] or cov["conflicts"] or cov["unknown"])


def gap_summary(positions: list[tuple]) -> str:
    out = [f"{p} file" for p, s, _ in positions if s == "file"]
    by: dict = {}
    for p, s, n in positions:
        if s != "file":
            by.setdefault((p, s), []).append(n)
    for (p, s), lines in sorted(by.items()):
        out.append(f"{p} {s}: {', '.join(str(n) for n in sorted(lines))}")
    return "\n".join(out) + ("\n" if out else "")


def describe_gaps(gaps: list[tuple]) -> str:
    return "".join(
        f"{p} file\n" if s == "file" else f"{p} {s}: {a}" + (f"-{b}" if b != a else "") + "\n"
        for p, s, a, b in gaps
    )


def coverage_report(cov: dict) -> str:
    out = [f"台帳 {cov['total']} 件のうち、ちょうど 1 節に属したもの {cov['classified']} 件"]
    if cov["unclassified"]:
        out.append(f"\nどの節にも属していない位置 ({len(cov['unclassified'])} 件):\n" + gap_summary(cov["unclassified"]))
    if cov["conflicts"]:
        out.append(f"\n2 つ以上の節が取り合っている位置 ({len(cov['conflicts'])} 件):\n" + gap_summary(cov["conflicts"]))
    if cov["unknown"]:
        out.append("\n台帳に無い位置 (変更されていない行か、存在しない行):\n" + describe_gaps(cov["unknown"]))
    if is_complete(cov):
        out.append("説明もれは無い")
    return "\n".join(out)


# ---- 作業 dir ----


def read_json(path: str):
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def write_json(path: str, value) -> None:
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(value, fh, ensure_ascii=False, indent=1)


def load_workdir(d: str) -> tuple[dict, list[dict], list[tuple]]:
    meta = read_json(os.path.join(d, "meta.json"))
    # newline="": 行の中の \r を改行に読み替えると hunk の行数が合わなくなる。
    with open(os.path.join(d, "diff.patch"), encoding="utf-8", newline="") as fh:
        files = parse_diff(fh.read())
    return meta, files, ledger_of(files)


# ---- HTML ----


def link(meta: dict, f: dict, side: str, start: int = 0, end: int = 0) -> tuple[str | None, str]:
    """GitHub の permalink と、張れないときの理由。"""
    repo = meta.get("github")
    if not repo:
        return None, ""
    if side == "old" or (side == "file" and f["kind"] == "deleted"):
        oid, path, pushed = meta["base_oid"], f["old_path"] or f["path"], meta["base_pushed"]
        if not pushed:
            return None, "ベースが未 push"
    else:
        oid, path = meta["head_oid"], f["path"]
        if path in meta["dirty"]:
            return None, "未コミット"
        if not meta["head_pushed"]:
            return None, "未 push"
    url = f"https://github.com/{repo}/blob/{oid}/{quote(path, safe='/')}"
    if side != "file":
        url += f"?plain=1#L{start}" + (f"-L{end}" if end != start else "")
    return url, ""


def esc(s) -> str:
    return html.escape(str(s), quote=True)


def prose(text: str, cls: str = "") -> str:
    # スクリプトが動かなくても読めるよう、生の文を入れておいて読み込めたら置き換える。
    return f'<div class="md {cls}" data-md>{esc(text)}</div>'


def link_html(meta, f, side, start=0, end=0, label="GitHub") -> str:
    url, why = link(meta, f, side, start, end)
    if url:
        return f'<a class="gh" href="{esc(url)}" target="_blank" rel="noopener">{esc(label)} ↗</a>'
    return f'<span class="nolink">{esc(why)}</span>' if why else ""


def excerpt(f: dict, h: dict, focus: set, conflicts: set, meta: dict) -> str:
    lines = h["lines"]

    def pos(line):
        tag, old_no, new_no, _ = line
        if tag == "+":
            return (f["path"], "new", new_no)
        if tag == "-":
            return (f["path"], "old", old_no)
        return None

    marks = [pos(l) in focus if pos(l) else False for l in lines]
    keep = set()
    for i, m in enumerate(marks):
        if m:
            keep.update(range(max(0, i - CONTEXT), min(len(lines), i + CONTEXT + 1)))
    gap = '<tr class="gap"><td></td><td></td><td>…</td></tr>'
    rows, last = [], -1
    for i in sorted(keep):
        if i != last + 1:
            rows.append(gap)
        last = i
        tag, old_no, new_no, body = lines[i]
        p = pos(lines[i])
        cls = {"+": "add", "-": "del", " ": "ctx"}[tag]
        if p and not marks[i]:
            cls += " other"
        if p in conflicts:
            cls += " dup"
        rows.append(
            f'<tr class="{cls}"><td class="ln">{old_no or ""}</td><td class="ln">{new_no or ""}</td>'
            f"<td class=\"code\"><span class=\"sign\">{esc(tag)}</span>{esc(body)}</td></tr>"
        )
    if last < len(lines) - 1:
        rows.append(gap)
    new = [l[2] for l, m in zip(lines, marks) if m and l[0] == "+"]
    old = [l[1] for l, m in zip(lines, marks) if m and l[0] == "-"]
    links = []
    if new:
        links.append(link_html(meta, f, "new", min(new), max(new), f"新 L{min(new)}–{max(new)}"))
    if old:
        links.append(link_html(meta, f, "old", min(old), max(old), f"旧 L{min(old)}–{max(old)}"))
    head = f'<span class="path">{esc(f["path"])}</span>'
    if h["header"]:
        head += f' <span class="fn">{esc(h["header"])}</span>'
    return (
        f'<figure class="hunk"><figcaption>{head}<span class="links">{" ".join(x for x in links if x)}</span></figcaption>'
        f'<div class="scroll"><table>{"".join(rows)}</table></div></figure>'
    )


BINARY_KIND_JA = {"added": "バイナリの追加", "deleted": "バイナリの削除"}
FILE_KIND_JA = {
    "mode-only": "モードだけの変更",
    "renamed": "名前だけの変更",
    "added": "空のファイルの追加",
    "deleted": "空のファイルの削除",
    "modified": "行を持たない変更",
}


def excerpts(files: list[dict], focus: set, conflicts: set, meta: dict) -> str:
    out = []
    for f in files:
        if (f["path"], "file", 0) in focus:
            if f["binary"]:
                label = BINARY_KIND_JA.get(f["kind"], "バイナリの変更")
            else:
                label = FILE_KIND_JA.get(f["kind"], "行を持たない変更")
            renamed = f" ← {esc(f['old_path'])}" if f["old_path"] else ""
            out.append(
                f'<div class="filechange"><span class="path">{esc(f["path"])}</span>{renamed}'
                f' <span class="muted">{label}</span> {link_html(meta, f, "file")}</div>'
            )
        for h in f["hunks"]:
            if any(p in focus for p in hunk_positions(f, h)):
                out.append(excerpt(f, h, focus, conflicts, meta))
    return "".join(out)


def hunk_positions(f: dict, h: dict):
    for tag, old_no, new_no, _ in h["lines"]:
        if tag == "+":
            yield (f["path"], "new", new_no)
        elif tag == "-":
            yield (f["path"], "old", old_no)


def verdict_chip(s: dict) -> str:
    n = len(s.get("concerns", []))
    if n:
        return f'<span class="chip concern">懸念あり {n}</span>'
    return '<span class="chip lgtm">LGTM</span>' if s["importance"] == "core" else ""


def verdict(sections: list[dict], complete: bool) -> str:
    items = [
        f'<li><a href="#s{i}">{esc(s["title"])}</a>{prose(c)}</li>'
        for i, s in enumerate(sections)
        for c in s.get("concerns", [])
    ]
    if items:
        return f'<span class="chip concern">懸念あり {len(items)}</span><ul class="concerns">{"".join(items)}</ul>'
    if not complete:
        return '<span class="chip warnchip">判定できない</span><p class="note">説明もれ検査が通っていない。</p>'
    return '<span class="chip lgtm">LGTM</span><p class="note">どの節にも懸念は無い。</p>'


def render(meta: dict, files: list[dict], ledger: list[tuple], review: dict) -> str:
    sections = sorted_sections(review)
    cov = coverage(ledger, sections)
    conflicts = set(cov["conflicts"])
    anchor = {}
    for i, s in enumerate(sections):
        anchor.setdefault(s["title"], i)

    toc, body = [], []
    for i, (s, sc) in enumerate(zip(sections, cov["sections"])):
        imp = s["importance"]
        toc.append(
            f'<li><a href="#s{i}"><span class="chip {imp}">{IMPORTANCE_JA[imp]}</span>{verdict_chip(s)}{esc(s["title"])}</a></li>'
        )
        concerns = "".join(f"<li>{prose(c)}</li>" for c in s.get("concerns", []))
        rels, primary_seen = [], False
        for rel in s.get("relations", []):
            j = anchor.get(rel["to"])
            if j == i:
                continue
            primary = rel.get("primary", False) and not primary_seen
            primary_seen = primary_seen or primary
            target = f'<a href="#s{j}">{esc(rel["to"])}</a>' if j is not None else f'{esc(rel["to"])} <span class="muted">(見つからない節)</span>'
            rels.append(f'<li class="{"primary" if primary else ""}">→ {target}{prose(rel["reason"], "inline")}</li>')
        notes = []
        if not sc["positions"]:
            notes.append('<p class="warn">この節は台帳に無い位置だけを指している。</p>')
        if sc["unknown"]:
            notes.append(f'<p class="warn">台帳に無い位置を指している: <code>{esc(describe_gaps(sc["unknown"]).strip())}</code></p>')
        body.append(
            f'<section class="sec {imp}" id="s{i}"><header><span class="chip {imp}">{IMPORTANCE_JA[imp]}</span>'
            f'{verdict_chip(s)}<h3>{esc(s["title"])}</h3></header>'
            f'<div class="why"><span class="label">理由</span>{prose(s["reason"])}</div>'
            + (f'<ul class="concerns">{concerns}</ul>' if concerns else "")
            + f"{prose(s['body'])}"
            + (f'<ul class="rels">{"".join(rels)}</ul>' if rels else "")
            + "".join(notes)
            + excerpts(files, sc["positions"], conflicts, meta)
            + "</section>"
        )

    if cov["unclassified"]:
        body.append(
            '<section class="sec orphan" id="unexplained"><header><span class="chip warnchip">説明なし</span>'
            "<h3>どの節も説明していない変更</h3></header>"
            + excerpts(files, set(cov["unclassified"]), conflicts, meta)
            + "</section>"
        )

    overview = "".join(
        f'<div class="ov"><dt>{OVERVIEW_JA[k]}</dt><dd>{prose(review["overview"][k])}</dd></div>' for k in OVERVIEW_FIELDS
    )
    impacts = "".join(
        f'<article class="impact"><header><h3>{esc(im["feature"])}</h3>'
        f'<span class="conf {im["confidence"]}">{"事実" if im["confidence"] == "fact" else "推測"}</span></header>'
        f'<dl><dt>変わること</dt><dd>{prose(im["change"])}</dd><dt>確かめ方</dt><dd>{prose(im["verify"])}</dd>'
        + (f'<dt>残る穴</dt><dd>{prose(im["gap"])}</dd>' if nonblank(im.get("gap")) else "")
        + "</dl></article>"
        for im in review.get("impacts", [])
    )

    banner = []
    if not is_complete(cov):
        parts = []
        if cov["unclassified"]:
            parts.append(f'<a href="#unexplained">説明なし {len(cov["unclassified"])} 件</a>')
        if cov["conflicts"]:
            parts.append(f"2 節が取り合い {len(cov['conflicts'])} 件")
        if cov["unknown"]:
            parts.append(f"台帳に無い位置 {len(cov['unknown'])} 区間")
        banner.append(f'<p class="banner">説明もれ検査が通っていない: {" / ".join(parts)}</p>')
    if not meta.get("github"):
        banner.append(f'<p class="note">{esc(meta.get("github_note") or "GitHub のリポジトリと判別できない")}ので、コードへのリンクは無い。</p>')

    dirty_note = "（未コミットの変更を含む）" if meta["dirty"] else ""
    title = f"{meta['branch']} のレビュー"
    return PAGE.format(
        title=esc(title),
        branch=esc(meta["branch"]),
        base_ref=esc(meta["base_ref"]),
        base=esc(meta["base_oid"][:10]),
        head=esc(meta["head_oid"][:10]),
        dirty=dirty_note,
        total=cov["total"],
        files=len(files),
        banner="".join(banner),
        verdict=verdict(sections, is_complete(cov)),
        overview=overview,
        toc="".join(toc),
        sections="".join(body),
        impacts=impacts or '<p class="muted">影響の記述は無い。</p>',
    )


PAGE = """<title>{title}</title>
<meta name="claude:viewport-fit-sides" content="cover">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=BIZ+UDPGothic:wght@400;700&family=JetBrains+Mono:wght@400;600&display=swap">
<style>
:root {{
  --bg: #f6f7f9; --surface: #ffffff; --fg: #1d2330; --muted: #5d6676; --line: #dde1e8;
  --accent: #2b55c7; --core: #b3261e; --ripple: #b06a00; --follow: #2f7d74; --minor: #7a8291;
  --add-bg: #e6f4ea; --add-fg: #135c2a; --del-bg: #fbe9e9; --del-fg: #8f1d1d; --focus: #fff4c2; --warn: #9a4b00;
  --sans: "BIZ UDPGothic", "Hiragino Sans", "Yu Gothic", system-ui, sans-serif;
  --mono: "JetBrains Mono", ui-monospace, "SFMono-Regular", Menlo, monospace;
}}
@media (prefers-color-scheme: dark) {{ :root:not([data-theme="light"]) {{
  --bg: #12151b; --surface: #1a1e26; --fg: #e3e7ee; --muted: #9aa3b2; --line: #2c323d;
  --accent: #8aa8ff; --core: #ff8a80; --ripple: #f0b35a; --follow: #6cc6b9; --minor: #9aa3b2;
  --add-bg: #15301f; --add-fg: #9ee0b0; --del-bg: #3a1a1c; --del-fg: #ffb3b3; --focus: #3a3418; --warn: #f0b35a;
  color-scheme: dark; }} }}
:root[data-theme="dark"] {{
  --bg: #12151b; --surface: #1a1e26; --fg: #e3e7ee; --muted: #9aa3b2; --line: #2c323d;
  --accent: #8aa8ff; --core: #ff8a80; --ripple: #f0b35a; --follow: #6cc6b9; --minor: #9aa3b2;
  --add-bg: #15301f; --add-fg: #9ee0b0; --del-bg: #3a1a1c; --del-fg: #ffb3b3; --focus: #3a3418; --warn: #f0b35a;
  color-scheme: dark; }}
body {{ background: var(--bg); color: var(--fg); font-family: var(--sans); line-height: 1.7; }}
main {{ max-width: 960px; margin: 0 auto; padding-block: 24px 64px;
  padding-left: max(16px, env(safe-area-inset-left, 0px)); padding-right: max(16px, env(safe-area-inset-right, 0px));
  display: grid; gap: 32px; }}
h1 {{ font-size: 1.5rem; margin: 0; text-wrap: balance; }}
h2 {{ font-size: 1.1rem; margin: 0 0 12px; letter-spacing: .04em; color: var(--muted); }}
h3 {{ font-size: 1.05rem; margin: 0; text-wrap: balance; }}
a {{ color: var(--accent); }}
code, .path, .fn, .ln, .code {{ font-family: var(--mono); font-size: .85em; }}
.meta {{ color: var(--muted); font-size: .9rem; display: flex; flex-wrap: wrap; gap: 4px 16px; }}
.banner {{ margin: 0; padding: 10px 14px; border: 1px solid var(--warn); color: var(--warn); border-radius: 6px; }}
.note, .muted {{ color: var(--muted); }}
.note {{ margin: 0; font-size: .9rem; }}
.warn {{ color: var(--warn); font-size: .9rem; margin: 0; }}
dl {{ margin: 0; }}
.ov {{ display: grid; grid-template-columns: 6em minmax(0, 1fr); gap: 12px; padding: 10px 0; border-top: 1px solid var(--line); }}
.ov dt {{ color: var(--muted); font-size: .85rem; letter-spacing: .08em; padding-top: 2px; }}
.ov dd {{ margin: 0; min-width: 0; }}
.toc {{ list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; }}
.toc a {{ display: flex; gap: 10px; align-items: baseline; text-decoration: none; color: var(--fg); }}
.toc a:hover h3, .toc a:hover {{ color: var(--accent); }}
.chip {{ flex: none; font-size: .72rem; font-weight: 700; letter-spacing: .06em; padding: 1px 8px; border-radius: 999px; border: 1px solid currentColor; }}
.chip.core {{ color: var(--core); }} .chip.ripple {{ color: var(--ripple); }} .chip.follow {{ color: var(--follow); }}
.chip.minor {{ color: var(--minor); }} .chip.warnchip {{ color: var(--warn); }}
.chip.lgtm {{ color: var(--bg); background: var(--follow); border-color: var(--follow); }}
.chip.concern {{ color: var(--bg); background: var(--core); border-color: var(--core); }}
.verdict {{ display: grid; gap: 8px; justify-items: start; }}
.verdict .chip {{ font-size: .95rem; padding: 2px 14px; }}
.concerns {{ margin: 0; padding-left: 1.4em; display: grid; gap: 6px; }}
.verdict .concerns {{ justify-self: stretch; }}
.verdict .concerns a {{ font-weight: 700; margin-right: .5em; }}
.sec .concerns {{ color: var(--core); }}
.sec {{ background: var(--surface); border: 1px solid var(--line); border-left: 4px solid var(--minor); border-radius: 6px;
  padding: 16px; display: grid; gap: 12px; min-width: 0; scroll-margin-top: 16px; }}
.sec.core {{ border-left-color: var(--core); }} .sec.ripple {{ border-left-color: var(--ripple); }}
.sec.follow {{ border-left-color: var(--follow); }} .sec.orphan {{ border-left-color: var(--warn); }}
.sec header {{ display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; }}
.sections {{ display: grid; gap: 20px; }}
.why {{ display: grid; grid-template-columns: 3em minmax(0, 1fr); gap: 8px; color: var(--muted); font-size: .92rem; }}
.label {{ font-size: .8rem; letter-spacing: .08em; padding-top: 2px; }}
.md {{ white-space: pre-wrap; min-width: 0; overflow-wrap: anywhere; }}
.md.rendered {{ white-space: normal; }}
.md.rendered > :first-child {{ margin-top: 0; }} .md.rendered > :last-child {{ margin-bottom: 0; }}
.md.inline {{ display: inline; }} .md.inline.rendered p {{ display: inline; }}
.md pre {{ overflow-x: auto; padding: 8px; background: var(--bg); border-radius: 4px; }}
.rels {{ list-style: none; margin: 0; padding: 0; font-size: .9rem; display: grid; gap: 4px; }}
.rels .primary {{ font-weight: 700; }}
.rels .md {{ margin-left: .5em; color: var(--muted); font-weight: 400; }}
.hunk {{ margin: 0; border: 1px solid var(--line); border-radius: 4px; overflow: hidden; min-width: 0; }}
.hunk figcaption, .filechange {{ display: flex; flex-wrap: wrap; gap: 4px 12px; align-items: baseline; padding: 6px 10px;
  background: var(--bg); font-size: .85rem; border-bottom: 1px solid var(--line); }}
.filechange {{ border: 1px solid var(--line); border-radius: 4px; }}
.links {{ margin-left: auto; display: flex; gap: 12px; }}
.gh {{ white-space: nowrap; }} .nolink {{ color: var(--muted); font-size: .8rem; }}
.fn {{ color: var(--muted); }}
.scroll {{ overflow-x: auto; }}
table {{ border-collapse: collapse; width: 100%; font-size: .8rem; line-height: 1.5; }}
td {{ padding: 0 8px; vertical-align: top; }}
.ln {{ color: var(--muted); text-align: right; user-select: none; width: 1%; white-space: nowrap; font-variant-numeric: tabular-nums; }}
.code {{ white-space: pre; }}
.sign {{ user-select: none; display: inline-block; width: 1.2em; color: var(--muted); }}
tr.add {{ background: var(--add-bg); color: var(--add-fg); }}
tr.del {{ background: var(--del-bg); color: var(--del-fg); }}
tr.other {{ opacity: .45; }}
tr.dup td.code {{ box-shadow: inset 3px 0 0 var(--warn); }}
tr.gap td {{ color: var(--muted); text-align: center; }}
.impacts {{ display: grid; gap: 12px; }}
.impact {{ background: var(--surface); border: 1px solid var(--line); border-radius: 6px; padding: 14px 16px; display: grid; gap: 8px; }}
.impact header {{ display: flex; gap: 10px; align-items: baseline; justify-content: space-between; flex-wrap: wrap; }}
.impact dl {{ display: grid; grid-template-columns: 6em minmax(0, 1fr); gap: 6px 12px; }}
.impact dt {{ color: var(--muted); font-size: .85rem; }} .impact dd {{ margin: 0; min-width: 0; }}
.conf {{ font-size: .75rem; color: var(--muted); border: 1px solid var(--line); padding: 0 8px; border-radius: 999px; }}
.conf.fact {{ color: var(--follow); border-color: var(--follow); }}
@media (max-width: 520px) {{ .ov, .impact dl, .why {{ grid-template-columns: minmax(0, 1fr); gap: 2px; }} .links {{ margin-left: 0; }} }}
</style>
<main>
<header style="display:grid;gap:8px">
<h1>{branch} のレビュー</h1>
<div class="meta"><span>{base_ref} (<code>{base}</code>) から作業ツリーまで{dirty}</span><span>HEAD <code>{head}</code></span><span>{files} ファイル / 変更位置 {total}</span></div>
{banner}
</header>
<section class="verdict"><h2>判定</h2>{verdict}</section>
<section><h2>概要</h2><dl>{overview}</dl></section>
<nav><h2>読む順</h2><ol class="toc">{toc}</ol></nav>
<section><h2>節ごとの変更</h2><div class="sections">{sections}</div></section>
<section><h2>機能への影響</h2><div class="impacts">{impacts}</div></section>
</main>
<script src="https://cdnjs.cloudflare.com/ajax/libs/marked/12.0.2/marked.min.js" integrity="sha384-/TQbtLCAerC3jgaim+N78RZSDYV7ryeoBCVqTuzRrFec2akfBkHS7ACQ3PQhvMVi" crossorigin="anonymous"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/dompurify/3.1.6/purify.min.js" integrity="sha384-+VfUPEb0PdtChMwmBcBmykRMDd+v6D/oFmB3rZM/puCMDYcIvF968OimRh4KQY9a" crossorigin="anonymous"></script>
<script>
if (window.marked && window.DOMPurify) {{
  for (const el of document.querySelectorAll("[data-md]")) {{
    el.innerHTML = DOMPurify.sanitize(marked.parse(el.textContent));
    el.classList.add("rendered");
  }}
}}
</script>
"""


# ---- コマンド ----


def cmd_prepare(args) -> int:
    root = repo_root(os.getcwd())
    head_oid = commit_of(root, "HEAD")
    base_ref = args.base or guess_base(root)
    base_oid = git_text(root, "merge-base", commit_of(root, base_ref), head_oid)
    text = diff_text(root, base_oid)
    if not text.strip():
        print(f"{base_ref} ({base_oid[:10]}) から作業ツリーまでに差分が無い", file=sys.stderr)
        return 3
    url = origin_url(root)
    repo = github_repo(url) if url else None
    meta = {
        "root": root,
        "branch": git_text(root, "rev-parse", "--abbrev-ref", "HEAD"),
        "base_ref": base_ref,
        "base_oid": base_oid,
        "head_oid": head_oid,
        "github": repo,
        "github_note": None if repo else ("origin が無い" if not url else "origin が GitHub のリポジトリと判別できない"),
        "head_pushed": bool(repo) and on_origin(root, head_oid),
        "base_pushed": bool(repo) and on_origin(root, base_oid),
        "dirty": dirty_paths(root),
    }
    d = tempfile.mkdtemp(prefix="revidere-")
    with open(os.path.join(d, "diff.patch"), "w", encoding="utf-8", newline="") as fh:
        fh.write(text)
    write_json(os.path.join(d, "meta.json"), meta)
    files = parse_diff(text)
    print(f"作業 dir: {d}")
    print(f"範囲: {base_ref} ({base_oid[:10]}) から作業ツリーまで。`git diff-index -p -M {base_oid[:10]}` で読める")
    print()
    print(ledger_summary(files), end="")
    return 0


def cmd_check(args) -> int:
    _, _, ledger = load_workdir(args.dir)
    first = os.path.join(args.dir, "review.json")
    if args.retry:
        return check_retry(args.dir, ledger)
    review = load_review(first)
    cov = coverage(ledger, sorted_sections(review))
    print(coverage_report(cov))
    return 0 if is_complete(cov) else 1


def check_retry(d: str, ledger: list[tuple]) -> int:
    """直した答えが最初の答えより説明もれを減らしたときだけ採る。"""
    first_path, retry_path = os.path.join(d, "review.json"), os.path.join(d, "review.retry.json")
    first = load_review(first_path)
    first_cov = coverage(ledger, sorted_sections(first))
    try:
        retry = load_review(retry_path)
    except Failure as e:
        print(f"直した答えを採らない ({e})。最初の答えのまま進む\n")
        print(coverage_report(first_cov))
        return 0 if is_complete(first_cov) else 1
    retry_cov = coverage(ledger, sorted_sections(retry))
    if len(retry_cov["unclassified"]) < len(first_cov["unclassified"]) or (
        len(retry_cov["unclassified"]) == len(first_cov["unclassified"]) and is_complete(retry_cov)
    ):
        os.replace(first_path, os.path.join(d, "review.first.json"))
        os.replace(retry_path, first_path)
        print("直した答えを採った\n")
        print(coverage_report(retry_cov))
        return 0 if is_complete(retry_cov) else 1
    print("直した答えは説明もれを減らさなかった。最初の答えのまま進む\n")
    print(coverage_report(first_cov))
    return 0 if is_complete(first_cov) else 1


def cmd_render(args) -> int:
    meta, files, ledger = load_workdir(args.dir)
    review = load_review(os.path.join(args.dir, "review.json"))
    out = os.path.join(args.dir, "review.html")
    page = render(meta, files, ledger, review)
    with open(out, "w", encoding="utf-8") as fh:
        fh.write(page)
    print(out)
    return 0


ARTIFACT_URL = "https://claude.ai/"


def cmd_published(args) -> int:
    if not args.url.startswith(ARTIFACT_URL):
        raise Failure(f"Artifact の URL ではない ({ARTIFACT_URL} で始まらない): {args.url}")
    path = os.path.join(args.dir, "meta.json")
    try:
        meta = read_json(path)
    except (OSError, ValueError) as e:
        raise Failure(f"{path} を読めない: {e}")
    meta["artifact_url"] = args.url
    # conductor が毎秒読むので、書きかけの meta.json を見せない。
    write_json(path + ".tmp", meta)
    os.replace(path + ".tmp", path)
    return 0


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="revidere.py")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("prepare")
    p.add_argument("--base")
    p.set_defaults(run=cmd_prepare)
    c = sub.add_parser("check")
    c.add_argument("dir")
    c.add_argument("--retry", action="store_true")
    c.set_defaults(run=cmd_check)
    r = sub.add_parser("render")
    r.add_argument("dir")
    r.set_defaults(run=cmd_render)
    u = sub.add_parser("published")
    u.add_argument("dir")
    u.add_argument("url")
    u.set_defaults(run=cmd_published)
    args = ap.parse_args(argv)
    try:
        return args.run(args)
    except Failure as e:
        print(f"revidere: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
