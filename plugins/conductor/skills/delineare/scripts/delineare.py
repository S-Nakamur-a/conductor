#!/usr/bin/env python3
"""delineare: 実装計画を、範囲・前提・作る/作らない・手順の決まった形で読むページにする道具。

    delineare.py prepare            作業 dir を作る (リポジトリの中なら branch と HEAD を残す)
    delineare.py check DIR          DIR/plan.json の形を確かめる
    delineare.py render DIR         DIR/plan.html を書く

見た目は同じプラグインの revidere の SHARED_CSS と FIGURE_CSS を借りる。標準ライブラリだけで Python 3.9 から動く。
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.realpath(__file__)), "..", "..", "revidere", "scripts"))
import revidere as rv  # noqa: E402

Failure = rv.Failure
esc, prose, nonblank = rv.esc, rv.prose, rv.nonblank

UNVERIFIED = "確かめられなかった"
HOW = ["読んだ", "走らせた", "測った", UNVERIFIED]
STATUS = {"draft": "未着手", "doing": "進行中", "done": "完了", "dropped": "取りやめ"}


# ---- plan.json ----


def strings(v, where: str, required: bool = True) -> list:
    if v is None and not required:
        return []
    if not isinstance(v, list) or not all(nonblank(x) for x in v):
        raise Failure(f"{where} が空でない文字列の配列でない")
    return v


def validate(p) -> None:
    if not isinstance(p, dict):
        raise Failure("plan.json の一番外が JSON オブジェクトでない")
    for key in ("title", "date", "scope"):
        if not nonblank(p.get(key)):
            raise Failure(f"{key} が空")
    status = p.get("status", "draft")
    if not isinstance(status, str) or status not in STATUS:
        raise Failure(f"status が {list(STATUS)} のどれでもない: {status!r}")
    strings(p.get("premises"), "premises")
    verified = p.get("verified")
    if not isinstance(verified, list):
        raise Failure("verified が配列でない")
    for i, v in enumerate(verified):
        if not isinstance(v, dict) or not nonblank(v.get("text")):
            raise Failure(f"verified[{i}].text が空")
        if v.get("how") not in HOW:
            raise Failure(f"verified[{i}].how が {HOW} のどれでもない: {v.get('how')!r}")
    strings(p.get("make"), "make")
    skip = p.get("skip")
    if not isinstance(skip, list):
        raise Failure("skip が配列でない")
    for i, s in enumerate(skip):
        if not isinstance(s, dict) or not nonblank(s.get("text")) or not nonblank(s.get("why")):
            raise Failure(f"skip[{i}] に text と why が無い")
    size = p.get("size", [])
    if not isinstance(size, list):
        raise Failure("size が配列でない")
    for i, s in enumerate(size):
        if not isinstance(s, dict) or not nonblank(s.get("label")) or s.get("value") in (None, ""):
            raise Failure(f"size[{i}] に label と value が無い")
    steps = p.get("steps")
    if not isinstance(steps, list) or not steps:
        raise Failure("steps が空")
    for i, s in enumerate(steps):
        where = f"steps[{i}]"
        if not isinstance(s, dict) or not nonblank(s.get("title")):
            raise Failure(f"{where}.title が空")
        if not nonblank(s.get("verify")):
            raise Failure(f"{where}（{s['title']}）に verify が無い")
        if not isinstance(s.get("body", ""), str):
            raise Failure(f"{where}.body が文字列でない")
        strings(s.get("files"), f"{where}.files", required=False)
    figures = p.get("figures", [])
    if not isinstance(figures, list):
        raise Failure("figures が配列でない")
    for i, f in enumerate(figures):
        if not isinstance(f, dict) or not nonblank(f.get("svg")) or not nonblank(f.get("caption")):
            raise Failure(f"figures[{i}] に svg と caption が無い")
    strings(p.get("open"), "open", required=False)


def load_plan(path: str) -> dict:
    try:
        p = rv.read_json(path)
    except FileNotFoundError:
        raise Failure(f"{path} が無い")
    except ValueError as e:
        raise Failure(f"{path} が JSON として読めない: {e}")
    validate(p)
    return p


# ---- HTML ----


def li(mk: str, sym: str, text: str, extra: str = "") -> str:
    return f'<li><span class="mk {mk}">{sym}</span><span>{prose(text, "inline")}{extra}</span></li>'


def paths(items: list[str]) -> str:
    return "".join('<span class="path">' + esc(x) + "</span>" for x in items)


def panel(title: str, items: list[str], count: int | None = None) -> str:
    n = len(items) if count is None else count
    return f'<section class="panel"><h3>{esc(title)}<span class="cnt">{n}</span></h3><ul class="list">{"".join(items)}</ul></section>'


def render(meta: dict, p: dict) -> str:
    status = p.get("status", "draft")
    where = " · ".join(esc(x) for x in (meta.get("repo"), meta.get("branch")) if x)
    size = "".join(
        f'<div><dt>{esc(s["label"])}</dt><dd>{esc(s["value"])}'
        + (f'<small>{esc(s["unit"])}</small>' if nonblank(s.get("unit")) else "")
        + "</dd></div>"
        for s in p.get("size", [])
    )
    figures = "".join(
        f'<figure class="pic"><div class="fig">{f["svg"]}</div><figcaption>{prose(f["caption"])}</figcaption></figure>'
        for f in p.get("figures", [])
    )
    premises = [li("pre", "・", x) for x in p["premises"]]
    verified = [
        li("unsure", "?", v["text"], f'<span class="how">{esc(v["how"])}</span>')
        if v["how"] == UNVERIFIED
        else li("fact", "✓", v["text"], f'<span class="how">{esc(v["how"])}</span>')
        for v in p["verified"]
    ]
    make = [li("make", "+", x) for x in p["make"]]
    skip = [li("skip", "×", s["text"], f'<span class="why">{esc(s["why"])}</span>') for s in p["skip"]]
    steps = "".join(
        f'<li class="step"><div class="rail"><span class="dot">{i}</span><span class="line"></span></div><div class="body">'
        f'<h3>{esc(s["title"])}</h3>'
        + (prose(s["body"]) if nonblank(s.get("body")) else "")
        + (f'<div class="files">{paths(s.get("files", []))}</div>' if s.get("files") else "")
        + f'<div class="check"><b>確かめ方</b>{prose(s["verify"], "inline")}</div></div></li>'
        for i, s in enumerate(p["steps"], 1)
    )
    open_items = [li("open", "?", x) for x in p.get("open") or []]
    return PAGE.format(
        title=esc(p["title"]),
        status=status,
        status_ja=STATUS[status],
        date=esc(p["date"]),
        where=f"<span>{where}</span>" if where else "",
        scope=prose(p["scope"]),
        size=f'<dl class="size">{size}</dl>' if size else "",
        figures=f'<section><h2 class="sh">全体の形</h2>{figures}</section>' if figures else "",
        premises=panel("前提", premises),
        verified=panel("確かめたこと", verified, sum(v["how"] != UNVERIFIED for v in p["verified"])),
        make=panel("作るもの", make),
        skip=panel("作らないもの", skip),
        steps=steps,
        open=panel("決めていないこと", open_items) if open_items else "",
        shared_css=rv.SHARED_CSS,
        figure_css=rv.FIGURE_CSS,
        md_script=rv.MD_SCRIPT,
    )


PAGE = """<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>実装計画: {title}</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=BIZ+UDGothic:wght@400;700&family=BIZ+UDPGothic:wght@400;700&display=swap">
<style>
{shared_css}
main {{ gap: 28px; }}
h1 {{ font-size: 30px; }}
.sh {{ font-size: 13px; font-weight: 600; letter-spacing: .08em; color: var(--muted); margin: 0 0 12px; }}
.eyebrow {{ display: flex; gap: 10px; align-items: center; flex-wrap: wrap; font-size: 13px; color: var(--muted); }}
.pill {{ display: inline-flex; align-items: center; height: 22px; padding: 0 10px; border-radius: 999px; font-size: 11.5px; font-weight: 600; letter-spacing: .04em; }}
.pill.plan {{ background: var(--accent); color: #fff; }}
.pill.draft {{ background: var(--warn-bg); color: var(--warn-fg); }}
.scope {{ background: var(--accent-soft); border-radius: var(--r-lg); padding: 18px 22px; display: grid; gap: 4px; }}
.scope .label, .panel .label {{ font-size: 12px; font-weight: 600; letter-spacing: .06em; color: var(--accent-strong); }}
.scope p {{ margin: 0; font-size: 17px; font-weight: 500; text-wrap: pretty; }}
.size {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 12px; margin: 0; }}
.size div {{ background: var(--surface); border-radius: var(--r-lg); box-shadow: var(--shadow); padding: 14px 18px; }}
.size dt {{ font-size: 12.5px; color: var(--muted); }}
.size dd {{ margin: 0; font-variant-numeric: tabular-nums; font-size: 26px; font-weight: 600; letter-spacing: 0; line-height: 1.3; }}
.size dd small {{ font-family: var(--sans); font-size: 13px; color: var(--muted); font-weight: 500; margin-left: 3px; }}
.grid2 {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 16px; }}
.panel {{ background: var(--surface); border-radius: var(--r-lg); box-shadow: var(--shadow); padding: 20px 22px; display: grid; gap: 12px; align-content: start; min-width: 0; }}
.panel h3 {{ display: flex; align-items: center; gap: 8px; }}
.cnt {{ font-variant-numeric: tabular-nums; font-size: 12px; color: var(--faint); font-weight: 500; }}
.list {{ list-style: none; margin: 0; padding: 0; display: grid; gap: 10px; }}
.list li {{ display: grid; grid-template-columns: 22px minmax(0, 1fr); gap: 10px; font-size: 14px; line-height: 1.7; }}
.mk {{ width: 22px; height: 22px; border-radius: 6px; display: grid; place-items: center; font-size: 12px; font-weight: 700; margin-top: 1px; font-variant-numeric: tabular-nums; }}
.mk.pre {{ background: var(--sunken); color: var(--muted); }}
.mk.fact {{ background: var(--ok-bg); color: var(--ok-fg); }}
.mk.make {{ background: var(--accent-soft); color: var(--accent); }}
.mk.skip {{ background: var(--line-soft); color: var(--faint); }}
.mk.unsure {{ background: var(--line-soft); color: var(--muted); }}
.mk.open {{ background: var(--warn-bg); color: var(--warn-fg); }}
.list .why {{ display: block; color: var(--muted); font-size: 12.5px; }}
.steps {{ list-style: none; margin: 0; padding: 0; display: grid; }}
.step {{ display: grid; grid-template-columns: 36px minmax(0, 1fr); column-gap: 16px; }}
.step .rail {{ display: flex; flex-direction: column; align-items: center; }}
.step .dot {{ width: 36px; height: 36px; border-radius: 18px; background: var(--accent); color: #fff; display: grid; place-items: center; font-variant-numeric: tabular-nums; font-weight: 600; font-size: 14px; flex: none; }}
.step .line {{ width: 2px; flex: 1; background: var(--accent-tint); margin: 4px 0; min-height: 16px; }}
.step:last-child .line {{ display: none; }}
.step .body {{ padding: 5px 0 24px; display: grid; gap: 6px; min-width: 0; }}
.step:last-child .body {{ padding-bottom: 0; }}
.step p {{ margin: 0; color: var(--muted); font-size: 14px; }}
.step .files {{ display: flex; flex-wrap: wrap; gap: 6px; }}
.step .files .path {{ background: var(--sunken); color: var(--muted); padding: 1px 8px; border-radius: 4px; font-size: 12px; }}
.check {{ display: flex; gap: 8px; align-items: baseline; font-size: 13px; color: var(--ok-fg); }}
.check b {{ font-weight: 600; flex: none; }}
{figure_css}
.how {{ font-size: 11px; font-weight: 600; color: var(--muted); background: var(--line-soft); border-radius: 4px; padding: 1px 7px; margin-left: 8px; white-space: nowrap; letter-spacing: .04em; }}
.pill.doing {{ background: var(--accent-tint); color: var(--accent-strong); }}
.pill.done {{ background: var(--ok-bg); color: var(--ok-fg); }}
.pill.dropped {{ background: var(--line-soft); color: var(--faint); }}
.scope .md {{ font-size: 17px; font-weight: 500; }}
.step .body > .md {{ color: var(--muted); font-size: 14px; }}
</style>
</head>
<body>
<main>
<header style="display:grid;gap:12px">
<div class="eyebrow"><span class="pill plan">実装計画</span><span class="pill {status}">{status_ja}</span><span>{date}</span>{where}</div>
<h1>{title}</h1>
<div class="scope"><span class="label">どこまでやるか</span>{scope}</div>
</header>
{size}
{figures}
<div class="grid2">{premises}{verified}</div>
<div class="grid2">{make}{skip}</div>
<section class="panel"><h3>やること</h3><ol class="steps">{steps}</ol></section>
{open}
</main>
{md_script}
</body>
</html>
"""


# ---- コマンド ----


def git_out(*args: str) -> str | None:
    p = subprocess.run(["git", "--no-optional-locks", *args], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return p.stdout.decode("utf-8", "replace").strip() if p.returncode == 0 else None


def cmd_prepare(args) -> int:
    meta: dict = {}
    root = git_out("rev-parse", "--show-toplevel")
    if root:
        url = git_out("remote", "get-url", "origin")
        meta = {
            "root": root,
            "branch": git_out("rev-parse", "--abbrev-ref", "HEAD"),
            "head_oid": git_out("rev-parse", "HEAD"),
            "repo": (rv.github_repo(url) if url else None) or os.path.basename(root),
        }
    d = tempfile.mkdtemp(prefix="delineare-")
    rv.write_json(os.path.join(d, "meta.json"), meta)
    print(f"作業 dir: {d}")
    return 0


def cmd_check(args) -> int:
    load_plan(os.path.join(args.dir, "plan.json"))
    print("plan.json の形は通った")
    return 0


def cmd_render(args) -> int:
    meta = rv.read_json(os.path.join(args.dir, "meta.json"))
    p = load_plan(os.path.join(args.dir, "plan.json"))
    out = os.path.join(args.dir, "plan.html")
    with open(out, "w", encoding="utf-8") as fh:
        fh.write(render(meta, p))
    print(out)
    return 0


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="delineare.py")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("prepare").set_defaults(run=cmd_prepare)
    for name, run in (("check", cmd_check), ("render", cmd_render)):
        c = sub.add_parser(name)
        c.add_argument("dir")
        c.set_defaults(run=run)
    args = ap.parse_args(argv)
    try:
        return args.run(args)
    except (Failure, OSError, ValueError) as e:
        print(f"delineare: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
