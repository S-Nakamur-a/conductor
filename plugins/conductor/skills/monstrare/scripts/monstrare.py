#!/usr/bin/env python3
"""monstrare: ブランチの変更を、主題ごとの図で読むページにする道具。

    monstrare.py prepare [--base REF]   作業 dir を作り、差分と変更ファイルの一覧を置く
    monstrare.py check DIR              DIR/story.json の形と、全ファイルがどれかの主題に入っているか
    monstrare.py render DIR             DIR/story.html を書く

git・差分・抜粋・リンクは同じプラグインの revidere.py を借りる。標準ライブラリだけで Python 3.9 から動く。
"""

from __future__ import annotations

import argparse
import os
import sys

# skill の dir がリンクでも revidere に届くよう、実体のパスから辿る。
sys.path.insert(0, os.path.join(os.path.dirname(os.path.realpath(__file__)), "..", "..", "revidere", "scripts"))
import revidere as rv  # noqa: E402

Failure = rv.Failure
esc, prose, nonblank = rv.esc, rv.prose, rv.nonblank

HOW = ["読んだ", "走らせた", "測った", "確かめられなかった"]


# ---- 変更ファイル ----


def counts(f: dict) -> tuple[int, int]:
    tags = [line[0] for h in f["hunks"] for line in h["lines"]]
    return tags.count("+"), tags.count("-")


def changed_paths(files: list[dict]) -> list[str]:
    # 種類だけ変わったファイルは同じパスで削除と追加の 2 件になるので、パスで 1 つにまとめる。
    return list(dict.fromkeys(f["path"] for f in files))


def file_list(files: list[dict]) -> str:
    out = []
    for f in files:
        add, dele = counts(f)
        size = "-" if f["binary"] or not f["hunks"] else f"+{add} -{dele}"
        out.append(f"{f['path']} [{'binary' if f['binary'] else f['kind']}] {size}")
        if f["old_path"]:
            out.append(f"  (renamed from {f['old_path']})")
    return "\n".join(out) + "\n"


# ---- story.json ----


def strings(v, where: str) -> list[str]:
    if not isinstance(v, list) or not all(nonblank(x) for x in v):
        raise Failure(f"{where} が空でない文字列の配列でない")
    return v


def figure(v, where: str) -> None:
    if not isinstance(v, dict) or not nonblank(v.get("svg")) or not nonblank(v.get("caption")):
        raise Failure(f"{where} に svg と caption が無い")


def note_items(v, where: str) -> None:
    if not isinstance(v, list):
        raise Failure(f"{where} が配列でない")
    for i, c in enumerate(v):
        if not isinstance(c, dict) or not nonblank(c.get("text")):
            raise Failure(f"{where}[{i}].text が空")
        if c.get("how") not in HOW:
            raise Failure(f"{where}[{i}].how が {HOW} のどれでもない: {c.get('how')!r}")


def validate(s) -> None:
    if not isinstance(s, dict):
        raise Failure("story.json の一番外が JSON オブジェクトでない")
    for key in ("title", "summary"):
        if not nonblank(s.get(key)):
            raise Failure(f"{key} が空")
    figure(s.get("overview"), "overview")
    themes = s.get("themes")
    if not isinstance(themes, list) or not themes:
        raise Failure("themes が空")
    for i, t in enumerate(themes):
        where = f"themes[{i}]"
        if not isinstance(t, dict) or not nonblank(t.get("title")):
            raise Failure(f"{where}.title が空")
        where = f"{where}（{t['title']}）"
        if not isinstance(t.get("body"), str):
            raise Failure(f"{where} に body が無い")
        if not isinstance(t.get("kind", ""), str):
            raise Failure(f"{where}.kind が文字列でない")
        figures = t.get("figures", [])
        if not isinstance(figures, list):
            raise Failure(f"{where}.figures が配列でない")
        for j, fig in enumerate(figures):
            figure(fig, f"{where}.figures[{j}]")
        if "before_after" in t:
            ba = t["before_after"]
            if not isinstance(ba, dict):
                raise Failure(f"{where}.before_after がオブジェクトでない")
            strings(ba.get("before"), f"{where}.before_after.before")
            strings(ba.get("after"), f"{where}.before_after.after")
        excerpts = t.get("excerpts", [])
        if not isinstance(excerpts, list):
            raise Failure(f"{where}.excerpts が配列でない")
        for j, rg in enumerate(excerpts):
            rv.validate_range(rg, f"{where}.excerpts[{j}]")
            if rg["side"] == "file":
                raise Failure(f"{where}.excerpts[{j}] の side は new か old (抜粋は行を指す)")
        strings(t.get("files"), f"{where}.files")
    note_items(s.get("notes", []), "notes")


def renamed_hint(path: str, files: list[dict]) -> str:
    for f in files:
        if f["old_path"] == path:
            return f"。{path} は {f['path']} への rename なので、新しいパスで書く"
    return ""


def theme_positions(t: dict, files: list[dict], index: dict) -> set:
    """抜粋が指す変更行。変更行を 1 行も含まない範囲は、読む人に何も見せないので受け付けない。"""
    found: set = set()
    for j, rg in enumerate(t.get("excerpts", [])):
        hits, _ = rv.range_hits(rg, index)
        if not hits:
            end = rg.get("end") or rg["start"]
            raise Failure(
                f"「{t['title']}」の excerpts[{j}] ({rg['path']} {rg['side']} {rg['start']}-{end}) に変更行が無い"
                + renamed_hint(rg["path"], files)
            )
        found.update(hits)
    return found


def load_story(path: str, files: list[dict]) -> dict:
    try:
        s = rv.read_json(path)
    except FileNotFoundError:
        raise Failure(f"{path} が無い")
    except ValueError as e:
        raise Failure(f"{path} が JSON として読めない: {e}")
    validate(s)
    known = set(changed_paths(files))
    for t in s["themes"]:
        for p in t["files"]:
            if p not in known:
                raise Failure(f"「{t['title']}」の files にある {p} は変更されていない" + renamed_hint(p, files))
    return s


def unthemed(story: dict, files: list[dict]) -> list[str]:
    covered = {p for t in story["themes"] for p in t["files"]}
    return [p for p in changed_paths(files) if p not in covered]


def load(d: str) -> tuple[dict, list[dict], list[dict], dict]:
    """story と、主題ごとの抜粋の変更行。render の前にも毎回ここを通す。"""
    meta, files, ledger = rv.load_workdir(d)
    story = load_story(os.path.join(d, "story.json"), files)
    index = rv.ledger_index(ledger)
    focus = [theme_positions(t, files, index) for t in story["themes"]]
    return meta, files, focus, story


# ---- HTML ----


def notes_html(story: dict) -> str:
    notes = story.get("notes", [])
    if not notes:
        return ""
    items = "".join(f'<li>{prose(n["text"], "inline")}<span class="how">{esc(n["how"])}</span></li>' for n in notes)
    return f'<section class="card"><div class="head"><h2>知っておくとよいこと {len(notes)}</h2></div><ul class="notes">{items}</ul></section>'


def figure_html(fig: dict) -> str:
    # svg は Claude が描いたものをそのまま埋める (描き方の規則は SKILL.md)。
    return f'<figure class="pic"><div class="fig">{fig["svg"]}</div><figcaption>{prose(fig["caption"])}</figcaption></figure>'


def before_after_html(ba: dict) -> str:
    def side(cls, label, items):
        lis = "".join(f"<li>{prose(x, 'inline')}</li>" for x in items)
        return f'<div class="{cls}"><span class="tag">{label}</span><ul>{lis}</ul></div>'

    return f'<div class="pair">{side("before", "前", ba["before"])}{side("after", "後", ba["after"])}</div>'


def map_html(files: list[dict], story: dict, missing: list[str]) -> str:
    themes_of: dict = {}
    for i, t in enumerate(story["themes"], 1):
        for p in t["files"]:
            themes_of.setdefault(p, []).append(i)
    sizes: dict = {}
    for f in files:
        add, dele = counts(f)
        a, d = sizes.get(f["path"], (0, 0))
        sizes[f["path"]] = (a + add, d + dele)
    widest = max([a + d for a, d in sizes.values()] + [1])
    rows = []
    for p in sorted(changed_paths(files), key=lambda p: p in missing):
        add, dele = sizes[p]
        marks = "".join(f'<a class="mark" href="#t{i}">{i:02d}</a>' for i in themes_of.get(p, []))
        if p in missing:
            marks = '<span class="mark none">主題なし</span>'
        rows.append(
            f'<div class="row"><span class="path">{esc(p)}</span><span class="marks">{marks}</span>'
            f'<span class="bar"><i class="a" style="width:{100 * add / widest:.1f}%"></i>'
            f'<i class="d" style="width:{100 * dele / widest:.1f}%"></i></span>'
            f'<span class="n">+{add} −{dele}</span></div>'
        )
    return "".join(rows)


def render(meta: dict, files: list[dict], focus: list[set], story: dict) -> str:
    missing = unthemed(story, files)
    themes = []
    for i, (t, positions) in enumerate(zip(story["themes"], focus), 1):
        kind = f'<span class="kind">{esc(t["kind"])}</span>' if nonblank(t.get("kind")) else ""
        names = "".join(f'<span class="path">{esc(p)}</span>' for p in t["files"])
        themes.append(
            f'<section class="card" id="t{i}"><div class="head"><span class="num">{i:02d}</span>'
            f'<h2>{esc(t["title"])}</h2>{kind}</div>'
            + prose(t["body"])
            + "".join(figure_html(fig) for fig in t.get("figures", []))
            + (before_after_html(t["before_after"]) if "before_after" in t else "")
            + rv.excerpts(files, positions, set(), meta)
            + f'<div class="files">{names}</div></section>'
        )
    banner = []
    if missing:
        banner.append(f'<p class="banner"><a href="#map">どの主題にも入っていないファイル {len(missing)} 件</a></p>')
    if not meta.get("github"):
        banner.append(f'<p class="note">{esc(meta.get("github_note") or "GitHub のリポジトリと判別できない")}ので、コードへのリンクは無い。</p>')
    add = sum(counts(f)[0] for f in files)
    dele = sum(counts(f)[1] for f in files)
    return PAGE.format(
        title=esc(story["title"]),
        branch=esc(meta["branch"]),
        base_ref=esc(meta["base_ref"]),
        base=esc(meta["base_oid"][:10]),
        head=esc(meta["head_oid"][:10]),
        dirty="（未コミットの変更を含む）" if meta["dirty"] else "",
        size=f"{len(changed_paths(files))} ファイル · +{add} −{dele}",
        summary=prose(story["summary"]),
        banner="".join(banner),
        overview=figure_html(story["overview"]),
        themes="".join(themes),
        notes=notes_html(story),
        map=map_html(files, story, missing),
        shared_css=rv.SHARED_CSS,
        figure_css=rv.FIGURE_CSS,
        md_script=rv.MD_SCRIPT,
    )


PAGE = """<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=BIZ+UDGothic:wght@400;700&family=BIZ+UDPGothic:wght@400;700&display=swap">
<style>
{shared_css}
main {{ gap: 28px; }}
h1 {{ font-size: 30px; }}
.lede {{ font-size: 17px; max-width: 64ch; }}
.card {{ background: var(--surface); border-radius: var(--r-lg); box-shadow: var(--shadow); padding: 26px; display: grid; gap: 16px; min-width: 0; scroll-margin-top: 16px; }}
.head {{ display: flex; gap: 12px; align-items: center; flex-wrap: wrap; }}
.head h2 {{ font-size: 20px; font-weight: 700; }}
.num {{ font-variant-numeric: tabular-nums; font-size: 13px; font-weight: 600; color: var(--accent-strong); background: var(--accent-soft); padding: 2px 8px; border-radius: var(--r-sm); letter-spacing: 0; }}
.kind {{ font-size: 12px; color: var(--muted); border: 1px solid var(--line); border-radius: 999px; padding: 1px 10px; }}
.pair {{ display: grid; grid-template-columns: 1fr 1fr; gap: 14px; }}
.pair > div {{ border-radius: var(--r-md); padding: 14px 16px; display: grid; gap: 8px; align-content: start; }}
.before {{ background: var(--sunken); }}
.after {{ background: var(--surface); border: 1.5px solid var(--ok); }}
.pair ul, ul.notes {{ margin: 0; padding-left: 1.2em; display: grid; gap: 6px; }}
.tag {{ justify-self: start; font-size: 11.5px; font-weight: 700; padding: 1px 10px; border-radius: 999px; }}
.before .tag {{ background: var(--line-soft); color: var(--muted); }}
.after .tag {{ background: var(--ok-bg); color: var(--ok-fg); }}
.files {{ display: flex; flex-wrap: wrap; gap: 6px; }}
.files .path {{ background: var(--sunken); color: var(--muted); padding: 1px 8px; border-radius: 4px; font-size: 12px; }}
.map {{ display: grid; gap: 8px; font-size: 13px; }}
.map .row {{ display: grid; grid-template-columns: minmax(0, 18em) 7em minmax(0, 1fr) 7em; gap: 12px; align-items: center; }}
.map .path {{ overflow-wrap: anywhere; }}
.marks {{ display: flex; gap: 4px; flex-wrap: wrap; }}
.mark {{ font-variant-numeric: tabular-nums; font-size: 11.5px; font-weight: 600; background: var(--accent-soft); color: var(--accent-strong); border-radius: 4px; padding: 0 6px; }}
.mark:hover {{ text-decoration: none; background: var(--accent-tint); }}
.mark.none {{ background: var(--warn-bg); color: var(--warn-fg); font-family: var(--sans); }}
.bar {{ height: 8px; border-radius: 4px; display: flex; overflow: hidden; background: var(--line-soft); }}
.bar i {{ display: block; height: 100%; }} .bar .a {{ background: var(--new); }} .bar .d {{ background: var(--gone); }}
.n {{ color: var(--muted); text-align: right; font-variant-numeric: tabular-nums; font-size: 12px; }}
@media (max-width: 640px) {{ .pair {{ grid-template-columns: 1fr; }} .map .row {{ grid-template-columns: minmax(0, 1fr) auto; }} .map .bar {{ grid-column: 1 / -1; }} }}
{figure_css}
.how {{ font-size: 11px; font-weight: 600; color: var(--muted); background: var(--line-soft); border-radius: 4px; padding: 1px 7px; margin-left: 8px; white-space: nowrap; letter-spacing: .04em; }}
</style>
</head>
<body>
<main>
<header style="display:grid;gap:10px">
<div class="meta"><span>{branch}</span><span>{base_ref} (<code>{base}</code>) から作業ツリーまで{dirty}</span><span>HEAD <code>{head}</code></span><span>{size}</span></div>
<h1>{title}</h1>
<div class="lede">{summary}</div>
{banner}
</header>
<section class="card" id="t0"><div class="head"><span class="num">00</span><h2>全体</h2></div>{overview}</section>
{themes}
{notes}
<section class="card" id="map"><div class="head"><h2>どのファイルがどの主題か</h2></div><div class="map">{map}</div></section>
</main>
{md_script}
</body>
</html>
"""


# ---- コマンド ----


def cmd_prepare(args) -> int:
    root, base_ref, base_oid, head_oid = rv.resolve_range(args.base)
    text = rv.diff_text(root, base_oid)
    if not text.strip():
        print(f"{base_ref} ({base_oid[:10]}) から作業ツリーまでに差分が無い", file=sys.stderr)
        return 3
    d = rv.write_workdir("monstrare-", rv.branch_meta(root, base_ref, base_oid, head_oid), text)
    print(f"作業 dir: {d}")
    print(f"範囲: {base_ref} ({base_oid[:10]}) から作業ツリーまで。`git diff-index -p -M {base_oid[:10]}` で読める")
    print()
    print(file_list(rv.parse_diff(text)), end="")
    return 0


def cmd_check(args) -> int:
    _, files, _, story = load(args.dir)
    missing = unthemed(story, files)
    total = len(changed_paths(files))
    print(f"変更ファイル {total} 件のうち、主題に入ったもの {total - len(missing)} 件")
    if missing:
        print(f"\nどの主題にも入っていないファイル ({len(missing)} 件):")
        print("".join(f"{p}\n" for p in missing), end="")
        return 1
    return 0


def cmd_render(args) -> int:
    meta, files, focus, story = load(args.dir)
    page = render(meta, files, focus, story)
    out = os.path.join(args.dir, "story.html")
    with open(out, "w", encoding="utf-8") as fh:
        fh.write(page)
    print(out)
    return 0


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="monstrare.py")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("prepare")
    p.add_argument("--base")
    p.set_defaults(run=cmd_prepare)
    for name, run in (("check", cmd_check), ("render", cmd_render)):
        c = sub.add_parser(name)
        c.add_argument("dir")
        c.set_defaults(run=run)
    args = ap.parse_args(argv)
    try:
        return args.run(args)
    except (Failure, OSError, ValueError) as e:
        # check の exit 1 は「主題に入っていないファイルがある」だけに使う。
        print(f"monstrare: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
