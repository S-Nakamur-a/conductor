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
        notes = s.get("notes", [])
        if not isinstance(notes, list) or not all(nonblank(c) for c in notes):
            raise Failure(f"{where}（{s['title']}）の notes が空でない文字列の配列でない")
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


LANGUAGE_BY_NAME = {
    "Gemfile": "ruby", "Rakefile": "ruby", "config.ru": "ruby",
    "Dockerfile": "dockerfile", "Containerfile": "dockerfile",
    "Makefile": "makefile",
}
LANGUAGE_BY_EXT = {
    ".rb": "ruby", ".rake": "ruby", ".gemspec": "ruby",
    ".go": "go", ".rs": "rust", ".py": "python", ".sql": "sql",
    ".toml": "toml", ".yml": "yaml", ".yaml": "yaml", ".ini": "ini",
    ".dockerfile": "dockerfile",
    ".js": "javascript", ".mjs": "javascript", ".cjs": "javascript", ".jsx": "javascript",
    ".ts": "typescript", ".tsx": "typescript", ".json": "json",
    ".sh": "bash", ".bash": "bash", ".zsh": "bash",
    ".md": "markdown", ".css": "css", ".scss": "scss", ".html": "xml", ".xml": "xml",
    ".java": "java", ".kt": "kotlin", ".swift": "swift", ".c": "c", ".h": "c",
    ".cpp": "cpp", ".cc": "cpp", ".cs": "csharp", ".php": "php", ".lua": "lua",
}


def language_of(path: str) -> str | None:
    """highlight.js の言語名。分からなければ None (色を付けない)。"""
    name = path.rsplit("/", 1)[-1]
    if name in LANGUAGE_BY_NAME:
        return LANGUAGE_BY_NAME[name]
    if name.startswith("Dockerfile."):
        return "dockerfile"
    return LANGUAGE_BY_EXT.get(os.path.splitext(name)[1].lower())


def code_text(body: str) -> str:
    # HTML は CR を改行に読み替えるので、残すとブラウザで行を繋いで色を付けるときに行がずれる。
    return body.rstrip("\r").replace("\r", "␍")


def lang_attr(path: str) -> str:
    lang = language_of(path)
    return f' data-lang="{esc(lang)}"' if lang else ""


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
            f'<td class="code"><span class="sign">{esc(tag)}</span><span class="src">{esc(code_text(body))}</span></td></tr>'
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
        f'<div class="scroll"><table{lang_attr(f["path"])}>{"".join(rows)}</table></div></figure>'
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


def listed(sections: list[dict], key: str) -> list[str]:
    return [
        f'<li><a href="#s{i}">{esc(s["title"])}</a>{prose(c)}</li>'
        for i, s in enumerate(sections)
        for c in s.get(key, [])
    ]


def verdict(sections: list[dict], complete: bool) -> str:
    items, held = listed(sections, "concerns"), listed(sections, "notes")
    if items:
        head = f'<span class="chip concern">懸念あり {len(items)}</span><ul class="concerns">{"".join(items)}</ul>'
    elif not complete:
        head = '<span class="chip warnchip">判定できない</span><p class="note">説明もれ検査が通っていない。</p>'
    elif held:
        head = '<span class="chip lgtm">LGTM</span><p class="note">LGTM を止める懸念は無い。</p>'
    else:
        head = '<span class="chip lgtm">LGTM</span><p class="note">どの節にも懸念は無い。</p>'
    if not held:
        return head
    return head + f'<span class="chip held">止めないもの {len(held)}</span><ul class="notes">{"".join(held)}</ul>'


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
        held = "".join(f"<li>{prose(c)}</li>" for c in s.get("notes", []))
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
            + (f'<ul class="notes">{held}</ul>' if held else "")
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
    explained = cov["classified"]
    stats = (
        f'<div><dt>ファイル</dt><dd>{len(files)}</dd></div>'
        f'<div><dt>変更位置</dt><dd>{cov["total"]}</dd></div>'
        f'<div><dt>節</dt><dd>{len(sections)}</dd></div>'
        f'<div class="{"ok" if is_complete(cov) else "ng"}"><dt>説明済み</dt><dd>{explained}/{cov["total"]}</dd></div>'
    )
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
        stats=stats,
        verdict=verdict(sections, is_complete(cov)),
        overview=overview,
        toc="".join(toc),
        sections="".join(body),
        impacts=impacts or '<p class="muted">影響の記述は無い。</p>',
        shared_css=SHARED_CSS,
        md_script=MD_SCRIPT,
    )


SHARED_CSS = """\
:root {
  --bg: oklch(0.975 0.006 250); --surface: #ffffff; --sunken: oklch(0.965 0.007 250);
  --fg: oklch(0.21 0.02 258); --muted: oklch(0.46 0.024 252); --faint: oklch(0.58 0.02 250);
  --line: oklch(0.90 0.012 250); --line-soft: oklch(0.94 0.009 250); --line-strong: oklch(0.78 0.018 250);
  --accent: oklch(0.56 0.19 257); --accent-strong: oklch(0.42 0.17 260); --accent-soft: oklch(0.965 0.02 255); --accent-tint: oklch(0.91 0.05 255);
  --ok: oklch(0.55 0.14 158); --ok-bg: oklch(0.965 0.03 158); --ok-fg: oklch(0.42 0.11 158);
  --bad: oklch(0.56 0.2 25); --bad-bg: oklch(0.965 0.02 22); --bad-fg: oklch(0.47 0.17 25);
  --warn: oklch(0.72 0.15 70); --warn-bg: oklch(0.975 0.035 88); --warn-fg: oklch(0.50 0.12 60);
  --info-bg: oklch(0.97 0.02 225); --info-fg: oklch(0.45 0.10 230);
  --add-bg: oklch(0.965 0.035 158); --add-fg: oklch(0.38 0.10 158); --del-bg: oklch(0.965 0.025 22); --del-fg: oklch(0.45 0.15 25);
  --tint: var(--accent-soft); --new: var(--ok); --gone: var(--bad); --hold: var(--warn);
  --sans: "BIZ UDPGothic", "Hiragino Sans", "Yu Gothic", system-ui, sans-serif;
  --mono: "BIZ UDGothic", ui-monospace, "SFMono-Regular", Menlo, monospace;
  --code: "Hiragino Sans", "Yu Gothic", system-ui, sans-serif;
  --r-sm: 6px; --r-md: 10px; --r-lg: 14px;
  --shadow: 0 1px 2px oklch(0.3 0.04 258 / .06), 0 0 0 1px oklch(0.3 0.04 258 / .035);
  color-scheme: light;
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg); font-family: var(--sans); font-size: 15px; line-height: 1.8; letter-spacing: .02em; -webkit-font-smoothing: antialiased; }
main { max-width: 960px; margin: 0 auto; padding: 40px max(20px, env(safe-area-inset-right, 0px)) 80px max(20px, env(safe-area-inset-left, 0px)); display: grid; gap: 36px; }
h1, h2, h3 { margin: 0; text-wrap: balance; line-height: 1.4; }
h1 { font-size: 28px; font-weight: 700; }
h3 { font-size: 16px; font-weight: 600; }
a { color: var(--accent); text-decoration: none; }
a:hover { color: var(--accent-strong); text-decoration: underline; text-underline-offset: 3px; }
code, .path, .fn { font-family: var(--code); font-size: .92em; letter-spacing: 0; }
.ln, .code { font-family: var(--mono); font-size: .85em; letter-spacing: 0; }
.meta { color: var(--muted); font-size: 13px; display: flex; flex-wrap: wrap; gap: 4px 16px; }
.meta code { color: var(--fg); }
.banner { margin: 0; padding: 10px 14px; background: var(--warn-bg); color: var(--warn-fg); border-radius: var(--r-md); font-weight: 500; font-size: 14px; }
.banner a { color: inherit; text-decoration: underline; }
.note, .muted { color: var(--muted); }
.note { margin: 0; font-size: 13px; }
.warn { color: var(--warn-fg); font-size: 13px; margin: 0; }
.md { white-space: pre-wrap; min-width: 0; overflow-wrap: anywhere; }
.md.rendered { white-space: normal; }
.md.rendered > :first-child { margin-top: 0; } .md.rendered > :last-child { margin-bottom: 0; }
.md.inline { display: inline; } .md.inline.rendered p { display: inline; }
.md p { margin: 0 0 .6em; }
.md code { background: var(--sunken); padding: 1px 5px; border-radius: 4px; }
.md pre { overflow-x: auto; padding: 10px 12px; background: var(--sunken); border-radius: var(--r-sm); }
.hunk { margin: 0; border: 1px solid var(--line); border-radius: var(--r-md); overflow: hidden; min-width: 0; background: var(--surface); }
.hunk figcaption, .filechange { display: flex; flex-wrap: wrap; gap: 4px 12px; align-items: baseline; padding: 7px 12px; background: var(--sunken); font-size: 13px; border-bottom: 1px solid var(--line); }
.filechange { border: 1px solid var(--line); border-radius: var(--r-md); }
.links { margin-left: auto; display: flex; gap: 10px; }
.gh { white-space: nowrap; font-size: 12px; font-weight: 600; }
.nolink { color: var(--faint); font-size: 11.5px; background: var(--line-soft); padding: 0 7px; border-radius: 4px; }
.fn { color: var(--muted); }
.scroll { overflow-x: auto; }
table { border-collapse: collapse; width: 100%; font-size: 12.5px; line-height: 1.65; }
td { padding: 0 10px; vertical-align: top; }
.ln { color: var(--faint); text-align: right; user-select: none; width: 1%; white-space: nowrap; font-variant-numeric: tabular-nums; }
.code { white-space: pre; }
.sign { user-select: none; display: inline-block; width: 1.2em; color: var(--faint); }
tr.add { background: var(--add-bg); color: var(--add-fg); }
tr.del { background: var(--del-bg); color: var(--del-fg); }
tr.add .sign, tr.del .sign { color: inherit; }
tr.other { opacity: .45; }
tr.dup td.code { box-shadow: inset 3px 0 0 var(--warn); }
tr.gap td { color: var(--faint); text-align: center; background: var(--sunken); font-size: 11px; }
.hljs-keyword, .hljs-selector-tag, .hljs-meta .hljs-keyword { color: oklch(0.45 0.19 320); }
.hljs-string, .hljs-regexp, .hljs-symbol, .hljs-char { color: oklch(0.43 0.12 215); }
.hljs-comment, .hljs-quote { color: var(--faint); font-style: italic; }
.hljs-number, .hljs-literal { color: oklch(0.48 0.14 55); }
.hljs-title, .hljs-section { color: oklch(0.44 0.16 265); }
.hljs-type, .hljs-built_in, .hljs-title.class_ { color: oklch(0.44 0.11 175); }
.hljs-attr, .hljs-attribute, .hljs-property, .hljs-variable, .hljs-params { color: oklch(0.45 0.11 85); }
.hljs-meta { color: var(--muted); }
"""

FIGURE_CSS = """\
figure.pic { margin: 0; display: grid; gap: 10px; min-width: 0; }
.fig { overflow-x: auto; background: var(--sunken); border-radius: var(--r-md); padding: 20px; }
.fig svg { max-width: 100%; height: auto; display: block; color: var(--fg); margin: 0 auto; }
.fig svg text { font-family: var(--sans); font-size: 12.5px; fill: currentColor; letter-spacing: .02em; }
.fig svg .m { font-family: var(--code); font-variant-numeric: tabular-nums; font-size: 11.5px; letter-spacing: 0; }
.fig svg .sub { fill: var(--muted); font-size: 11px; }
.fig svg .box { fill: var(--surface); stroke: var(--line-strong); stroke-width: 1.2; }
.fig svg .soft { fill: var(--tint); stroke: var(--accent-tint); stroke-width: 1.2; }
.fig svg .edge { fill: none; stroke: var(--muted); stroke-width: 1.4; }
.fig svg .dash { stroke-dasharray: 5 4; }
.fig svg .new { stroke: var(--new); stroke-width: 1.6; } .fig svg .newt { fill: var(--ok-fg); }
.fig svg .gone { stroke: var(--gone); } .fig svg .gonet { fill: var(--bad-fg); }
.fig svg .hold { stroke: var(--hold); stroke-width: 1.6; } .fig svg .holdt { fill: var(--warn-fg); }
.fig svg .box.new { fill: var(--ok-bg); } .fig svg .box.gone { fill: var(--bad-bg); } .fig svg .box.hold { fill: var(--warn-bg); }
.fig svg .lane { stroke: var(--line-strong); stroke-dasharray: 3 4; }
.fig svg marker path { fill: var(--muted); } .fig svg marker path.newt { fill: var(--new); } .fig svg marker path.holdt { fill: var(--hold); }
.pic figcaption { color: var(--muted); font-size: 13.5px; max-width: 72ch; }
"""

MD_SCRIPT = """\
<script src="https://cdnjs.cloudflare.com/ajax/libs/marked/12.0.2/marked.min.js" integrity="sha384-/TQbtLCAerC3jgaim+N78RZSDYV7ryeoBCVqTuzRrFec2akfBkHS7ACQ3PQhvMVi" crossorigin="anonymous"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/dompurify/3.1.6/purify.min.js" integrity="sha384-+VfUPEb0PdtChMwmBcBmykRMDd+v6D/oFmB3rZM/puCMDYcIvF968OimRh4KQY9a" crossorigin="anonymous"></script>
<script>
if (window.marked && window.DOMPurify) {
  for (const el of document.querySelectorAll("[data-md]")) {
    el.innerHTML = DOMPurify.sanitize(marked.parse(el.textContent));
    el.classList.add("rendered");
  }
}
</script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/highlight.min.js" integrity="sha384-F/bZzf7p3Joyp5psL90p/p89AZJsndkSoGwRpXcZhleCWhd8SnRuoYo4d0yirjJp" crossorigin="anonymous"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/dockerfile.min.js" integrity="sha384-H3wUmCndJNBOuIIzcLSr2zLLZYs8PYuw6qKyENk7THEnYQJvOMDBu42P9XsJSRKw" crossorigin="anonymous"></script>
<script>
// 複数行の文字列やコメントの途中の行も塗るため、表の行を新側と旧側の列に繋いで 1 回で塗り、結果を行で切って戻す。
function revidereSplitLines(html) {
  const lines = [], open = [];
  let cur = "", m;
  const re = /(<span[^>]*>)|(<\\/span>)|(\\n)|([^<\\n]+)/g;
  while ((m = re.exec(html))) {
    if (m[1]) { open.push(m[1]); cur += m[1]; }
    else if (m[2]) { open.pop(); cur += m[2]; }
    else if (m[3]) { lines.push(cur + "</span>".repeat(open.length)); cur = open.join(""); }
    else cur += m[4];
  }
  lines.push(cur);
  return lines;
}
function revidereRuns(table) {
  const runs = [];
  let fresh = [], stale = [];
  const flush = () => { runs.push(fresh, stale); fresh = []; stale = []; };
  for (const tr of table.rows) {
    if (tr.classList.contains("gap")) { flush(); continue; }
    const el = tr.querySelector(".src");
    if (!el) continue;
    if (tr.classList.contains("del")) stale.push({ el, paint: true });
    else {
      fresh.push({ el, paint: true });
      if (tr.classList.contains("ctx")) stale.push({ el, paint: false });
    }
  }
  flush();
  return runs.filter((run) => run.length);
}
if (window.hljs) {
  for (const table of document.querySelectorAll("table[data-lang]")) {
    const language = table.dataset.lang;
    if (!hljs.getLanguage(language)) continue;
    try {
      const paint = (code) => revidereSplitLines(hljs.highlight(code, { language, ignoreIllegals: true }).value);
      for (const run of revidereRuns(table)) {
        // 末尾に足した空行がまだ span の中なら、表示範囲が文字列などの途中から始まったか途中で終わっていて
        // 繋いで塗ると中と外が反転する。
        let lines = paint(run.map((x) => x.el.textContent).join("\\n") + "\\n");
        if (lines.pop().startsWith("<span")) lines = run.map((x) => paint(x.el.textContent)[0]);
        if (lines.length !== run.length) continue;
        run.forEach((x, i) => { if (x.paint) x.el.innerHTML = lines[i]; });
      }
    } catch (e) {
      console.warn("revidere: 色を付けられない表がある", e);
    }
  }
}
</script>
"""


PAGE = """<title>{title}</title>
<meta name="claude:viewport-fit-sides" content="cover">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=BIZ+UDGothic:wght@400;700&family=BIZ+UDPGothic:wght@400;700&display=swap">
<style>
{shared_css}
main > section > h2, nav > h2, .verdict h2 {{ font-size: 13px; font-weight: 600; letter-spacing: .08em; color: var(--muted); margin: 0 0 12px; }}
.top {{ display: grid; gap: 12px; }}
.stats {{ display: flex; flex-wrap: wrap; gap: 10px; margin: 4px 0 0; }}
.stats div {{ display: grid; gap: 0; background: var(--surface); box-shadow: var(--shadow); border-radius: var(--r-md); padding: 8px 14px; min-width: 104px; }}
.stats dt {{ font-size: 11.5px; color: var(--muted); }}
.stats dd {{ margin: 0; font-family: var(--mono); font-size: 20px; font-weight: 600; letter-spacing: 0; }}
.stats .ok dd {{ color: var(--ok-fg); }} .stats .ng dd {{ color: var(--warn-fg); }}
dl {{ margin: 0; }}
section > dl {{ background: var(--surface); border-radius: var(--r-lg); box-shadow: var(--shadow); padding: 4px 22px; }}
.ov {{ display: grid; grid-template-columns: 6em minmax(0, 1fr); gap: 16px; padding: 12px 0; border-top: 1px solid var(--line-soft); }}
.ov:first-child {{ border-top: 0; }}
.ov dt {{ color: var(--accent-strong); font-size: 13px; font-weight: 600; letter-spacing: .06em; padding-top: 2px; }}
.ov dd {{ margin: 0; min-width: 0; }}
.toc {{ list-style: none; margin: 0; padding: 6px; display: grid; gap: 2px; background: var(--surface); border-radius: var(--r-lg); box-shadow: var(--shadow); counter-reset: toc; }}
.toc li {{ counter-increment: toc; }}
.toc a {{ display: flex; gap: 8px; align-items: center; padding: 8px 12px; border-radius: var(--r-sm); color: var(--fg); }}
.toc a::before {{ content: counter(toc, decimal-leading-zero); font-family: var(--mono); font-size: 12px; color: var(--faint); width: 2em; flex: none; }}
.toc a:hover {{ background: var(--sunken); color: var(--accent-strong); text-decoration: none; }}
.chip {{ flex: none; display: inline-flex; align-items: center; gap: 4px; height: 22px; font-size: 11.5px; font-weight: 600; letter-spacing: .04em; padding: 0 9px; border-radius: 999px; border: 1px solid transparent; line-height: 1; white-space: nowrap; }}
.chip.core {{ background: var(--accent); color: #fff; }}
.chip.ripple {{ background: var(--accent-tint); color: var(--accent-strong); }}
.chip.follow {{ color: var(--muted); border-color: var(--line-strong); }}
.chip.minor {{ background: var(--line-soft); color: var(--faint); }}
.chip.lgtm {{ background: var(--ok-bg); color: var(--ok-fg); }}
.chip.lgtm::before {{ content: "✓"; }}
.chip.concern {{ background: var(--bad); color: #fff; }}
.chip.concern::before {{ content: "!"; font-family: var(--mono); }}
.chip.warnchip {{ background: var(--warn-bg); color: var(--warn-fg); }}
.chip.held {{ background: var(--info-bg); color: var(--info-fg); }}
.verdict {{ display: grid; gap: 12px; justify-items: start; background: var(--surface); border-radius: var(--r-lg); box-shadow: var(--shadow); padding: 20px 22px; }}
.verdict h2 {{ margin: 0; }}
.verdict > .chip {{ height: 30px; font-size: 14px; padding: 0 14px; }}
.concerns {{ margin: 0; padding-left: 1.3em; display: grid; gap: 6px; }}
.concerns li::marker {{ color: var(--bad); }}
.verdict .concerns, .verdict .notes {{ justify-self: stretch; }}
.verdict .concerns a, .verdict .notes a {{ font-weight: 600; margin-right: .6em; }}
.sec .concerns {{ color: var(--bad-fg); background: var(--bad-bg); border-radius: var(--r-md); padding: 10px 14px 10px 2.1em; font-size: 14px; }}
.notes {{ margin: 0; padding-left: 1.3em; display: grid; gap: 6px; }}
.notes li::marker {{ color: var(--info-fg); }}
.sec .notes {{ color: var(--info-fg); background: var(--info-bg); border-radius: var(--r-md); padding: 10px 14px 10px 2.1em; font-size: 14px; }}
.sections {{ display: grid; gap: 18px; }}
.sec {{ background: var(--surface); border: 1px solid transparent; border-radius: var(--r-lg); box-shadow: var(--shadow); padding: 20px 22px; display: grid; gap: 14px; min-width: 0; scroll-margin-top: 16px; }}
.sec.core {{ border-color: var(--accent-tint); }}
.sec.orphan {{ border-color: var(--warn); }}
.sec header {{ display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }}
.sec header h3 {{ margin-left: 4px; }}
.why {{ display: grid; grid-template-columns: 3.2em minmax(0, 1fr); gap: 8px; color: var(--muted); font-size: 13.5px; background: var(--sunken); border-radius: var(--r-md); padding: 10px 14px; }}
.label {{ font-size: 12px; font-weight: 600; letter-spacing: .06em; padding-top: 1px; color: var(--faint); }}
.rels {{ list-style: none; margin: 0; padding: 0; font-size: 13.5px; display: grid; gap: 4px; }}
.rels .primary {{ font-weight: 600; }}
.rels .md {{ margin-left: .5em; color: var(--muted); font-weight: 400; }}
.impacts {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 14px; }}
.impact {{ background: var(--surface); border-radius: var(--r-lg); box-shadow: var(--shadow); padding: 18px 20px; display: grid; gap: 10px; align-content: start; }}
.impact header {{ display: flex; gap: 10px; align-items: center; justify-content: space-between; flex-wrap: wrap; }}
.impact dl {{ display: grid; grid-template-columns: 5.5em minmax(0, 1fr); gap: 6px 12px; font-size: 14px; }}
.impact dt {{ color: var(--muted); font-size: 12.5px; padding-top: 2px; }}
.impact dd {{ margin: 0; min-width: 0; }}
.conf {{ font-size: 11.5px; font-weight: 600; padding: 1px 9px; border-radius: 999px; color: var(--muted); border: 1px dashed var(--line-strong); }}
.conf.fact {{ background: var(--accent-soft); color: var(--accent-strong); border: 1px solid var(--accent-tint); }}
@media (max-width: 560px) {{ .ov, .impact dl, .why {{ grid-template-columns: minmax(0, 1fr); gap: 2px; }} .links {{ margin-left: 0; }} }}
</style>
<main>
<header class="top">
<h1>{branch} のレビュー</h1>
<div class="meta"><span>{base_ref} (<code>{base}</code>) から作業ツリーまで{dirty}</span><span>HEAD <code>{head}</code></span><span>{files} ファイル / 変更位置 {total}</span></div>
<dl class="stats">{stats}</dl>
{banner}
</header>
<section class="verdict"><h2>判定</h2>{verdict}</section>
<section><h2>概要</h2><dl>{overview}</dl></section>
<nav><h2>読む順</h2><ol class="toc">{toc}</ol></nav>
<section><h2>節ごとの変更</h2><div class="sections">{sections}</div></section>
<section><h2>機能への影響</h2><div class="impacts">{impacts}</div></section>
</main>
{md_script}
"""


# ---- コマンド ----


def resolve_range(base: str | None) -> tuple[str, str, str, str]:
    """(root, base_ref, base_oid, head_oid)。base_oid はベースとの共通祖先。"""
    root = repo_root(os.getcwd())
    head_oid = commit_of(root, "HEAD")
    base_ref = base or guess_base(root)
    base_oid = git_text(root, "merge-base", commit_of(root, base_ref), head_oid)
    return root, base_ref, base_oid, head_oid


def branch_meta(root: str, base_ref: str, base_oid: str, head_oid: str) -> dict:
    url = origin_url(root)
    repo = github_repo(url) if url else None
    return {
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


def write_workdir(prefix: str, meta: dict, text: str) -> str:
    d = tempfile.mkdtemp(prefix=prefix)
    with open(os.path.join(d, "diff.patch"), "w", encoding="utf-8", newline="") as fh:
        fh.write(text)
    write_json(os.path.join(d, "meta.json"), meta)
    return d


def cmd_prepare(args) -> int:
    root, base_ref, base_oid, head_oid = resolve_range(args.base)
    text = diff_text(root, base_oid)
    if not text.strip():
        print(f"{base_ref} ({base_oid[:10]}) から作業ツリーまでに差分が無い", file=sys.stderr)
        return 3
    d = write_workdir("revidere-", branch_meta(root, base_ref, base_oid, head_oid), text)
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
