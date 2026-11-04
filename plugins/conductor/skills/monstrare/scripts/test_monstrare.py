"""monstrare.py のテスト。差分は一時リポジトリで本物の git に出させる (道具は revidere のテストから借りる)。"""

from __future__ import annotations

import copy
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "monstrare.py")
REVIDERE_SCRIPTS = os.path.join(HERE, "..", "..", "revidere", "scripts")
sys.path.insert(0, REVIDERE_SCRIPTS)
import test_revidere as trv  # noqa: E402

spec = importlib.util.spec_from_file_location("monstrare", SCRIPT)
mo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mo)

SVG = '<svg viewBox="0 0 100 40" role="img" aria-label="x"><rect class="box" x="1" y="1" width="50" height="20"/></svg>'


class Repo(trv.Repo):
    def run(self, *args, cwd=None, script=SCRIPT):
        p = subprocess.run([sys.executable, script, *args], cwd=cwd or self.path, env=self.env, capture_output=True)
        return p.returncode, p.stdout.decode(), p.stderr.decode()


def theme(title, files, **extra):
    return dict({"title": title, "body": "本文", "figures": [{"svg": SVG, "caption": "図"}], "files": files}, **extra)


def story_of(themes, **extra):
    return dict({"title": "見出し", "summary": "要約", "overview": {"svg": SVG, "caption": "全体"}, "themes": themes}, **extra)


class MonstrareTest(unittest.TestCase):
    def setUp(self):
        self.r = Repo()
        self.r.write("keep.txt", "k\n")
        self.r.write("a.py", "".join(f"line{i}\n" for i in range(1, 21)))
        self.r.commit("base")
        self.r.branch()

    def tearDown(self):
        self.r.close()

    def prepare(self, *args):
        code, out, err = self.r.run("prepare", *args)
        self.assertEqual(code, 0, err)
        return out.splitlines()[0].split(": ", 1)[1], out

    def put(self, d, story):
        with open(os.path.join(d, "story.json"), "w", encoding="utf-8") as fh:
            json.dump(story, fh, ensure_ascii=False)

    def check(self, d, story):
        self.put(d, story)
        return self.r.run("check", d)

    def render(self, d, story):
        self.put(d, story)
        code, out, err = self.r.run("render", d)
        self.assertEqual(code, 0, err)
        self.assertEqual(os.path.basename(out.strip()), "story.html")
        with open(out.strip(), encoding="utf-8") as fh:
            return fh.read()

    def edit_a(self):
        lines = [f"line{i}\n" for i in range(1, 21)]
        lines[4] = "changed5\n"
        lines[14] = "changed15\n"
        self.r.write("a.py", "".join(lines))


class Prepare(MonstrareTest):
    def test_差分が無ければexit3で文を出す(self):
        code, _, err = self.r.run("prepare")
        self.assertEqual(code, 3)
        self.assertIn("差分が無い", err)

    def test_ベースが解決できなければexit2(self):
        self.r.write("keep.txt", "k2\n")
        code, _, err = self.r.run("prepare", "--base", "no-such")
        self.assertEqual(code, 2)
        self.assertIn("monstrare:", err)

    def test_作業dirの接頭辞はrevidereの印に数えられない名前(self):
        self.r.write("keep.txt", "k2\n")
        d, _ = self.prepare()
        self.assertTrue(os.path.basename(d).startswith("monstrare-"))
        self.assertTrue(os.path.isfile(os.path.join(d, "meta.json")))

    def test_一覧に行数と状態が出て未追跡と空とバイナリは行数を持たない(self):
        self.edit_a()
        self.r.write("new.txt", "n1\nn2\n")
        self.r.write("empty.txt", "")
        self.r.write("bin.dat", b"\x00\x01\x02")
        _, out = self.prepare()
        self.assertIn("a.py [modified] +2 -2", out)
        self.assertIn("new.txt [added] +2 -0", out)
        self.assertIn("empty.txt [added] -", out)
        self.assertIn("bin.dat [binary] -", out)

    def test_改名は新しいパスで出て元の名前を添える(self):
        self.r.git("mv", "a.py", "b.py")
        self.r.commit("mv")
        _, out = self.prepare()
        self.assertIn("b.py [renamed] -", out)
        self.assertIn("(renamed from a.py)", out)

    def test_モードだけの変更は行数を持たず主題に入れられる(self):
        os.chmod(os.path.join(self.r.path, "a.py"), 0o755)
        d, out = self.prepare()
        self.assertIn("a.py [mode-only] -", out)
        code, _, err = self.check(d, story_of([theme("A", ["a.py"])]))
        self.assertEqual(code, 0, err)

    def test_利用者のindexと作業ツリーに書かない(self):
        self.edit_a()
        index = os.path.join(self.r.path, ".git", "index")
        status = self.r.git("status", "--porcelain")
        os.utime(index, (1, 1))
        self.prepare()
        self.assertEqual(os.stat(index).st_mtime, 1)
        self.assertEqual(self.r.git("status", "--porcelain"), status)

    def test_skillのdirがリンクでもrevidereに届く(self):
        self.r.write("keep.txt", "k2\n")
        with tempfile.TemporaryDirectory() as t:
            link = os.path.join(t, "monstrare")
            os.symlink(os.path.dirname(HERE), link)
            code, _, err = self.r.run("prepare", script=os.path.join(link, "scripts", "monstrare.py"))
        self.assertEqual(code, 0, err)


class Check(MonstrareTest):
    def setUp(self):
        super().setUp()
        self.edit_a()
        self.r.write("new.txt", "n1\n")
        self.d, _ = self.prepare()

    def test_全ファイルが主題に入ればexit0(self):
        code, out, _ = self.check(self.d, story_of([theme("A", ["a.py"]), theme("B", ["new.txt"])]))
        self.assertEqual(code, 0, out)

    def test_同じファイルが2つの主題に入ってよい(self):
        code, _, _ = self.check(self.d, story_of([theme("A", ["a.py", "new.txt"]), theme("B", ["a.py"])]))
        self.assertEqual(code, 0)

    def test_主題に入っていないファイルはexit1で名指す(self):
        code, out, _ = self.check(self.d, story_of([theme("A", ["a.py"])]))
        self.assertEqual(code, 1)
        self.assertIn("new.txt", out)

    def test_変更されていないパスを主題に入れるとexit2(self):
        code, _, err = self.check(self.d, story_of([theme("A", ["a.py", "new.txt", "keep.txt"])]))
        self.assertEqual(code, 2)
        self.assertIn("keep.txt は変更されていない", err)

    def test_形が壊れているとexit2で欄を名指す(self):
        ok = story_of([theme("A", ["a.py", "new.txt"])])
        cases = {
            "themes が空": lambda s: s.update(themes=[]),
            "overview に svg と caption が無い": lambda s: s.pop("overview"),
            "summary が空": lambda s: s.update(summary=" "),
            "files が空でない文字列の配列でない": lambda s: s["themes"][0].update(files="a.py"),
            "figures[0] に svg と caption が無い": lambda s: s["themes"][0].update(figures=[{"svg": SVG}]),
            "before_after.after が空でない文字列の配列でない": lambda s: s["themes"][0].update(before_after={"before": ["x"]}),
            "concerns.blocking が配列でない": lambda s: s.update(concerns={"blocking": None}),
            "how が": lambda s: s.update(concerns={"nonblocking": [{"text": "t", "how": "たぶん"}]}),
            "行範囲が壊れている": lambda s: s["themes"][0].update(excerpts=[{"path": "a.py", "side": "new", "start": "5"}]),
        }
        for want, mutate in cases.items():
            with self.subTest(want):
                s = copy.deepcopy(ok)
                mutate(s)
                code, _, err = self.check(self.d, s)
                self.assertEqual(code, 2, err)
                self.assertIn(want, err)

    def test_図とconcernsは省いてよくhowは4つの値を受ける(self):
        s = story_of([{"title": "A", "body": "", "files": ["a.py", "new.txt"]}])
        s["concerns"] = {"nonblocking": [{"text": "未計測", "how": h} for h in mo.HOW]}
        code, _, err = self.check(self.d, s)
        self.assertEqual(code, 0, err)

    def test_抜粋が変更行を含まなければexit2(self):
        for rg in ({"path": "a.py", "side": "new", "start": 1, "end": 3}, {"path": "keep.txt", "side": "new", "start": 1}):
            with self.subTest(rg=rg):
                code, _, err = self.check(self.d, story_of([theme("A", ["a.py", "new.txt"], excerpts=[rg])]))
                self.assertEqual(code, 2, err)
                self.assertIn("変更行が無い", err)

    def test_抜粋はfileを指せず開始が終わりより後でも止まる(self):
        for rg in ({"path": "a.py", "side": "file"}, {"path": "a.py", "side": "new", "start": 9, "end": 3}):
            with self.subTest(rg=rg):
                code, _, _ = self.check(self.d, story_of([theme("A", ["a.py", "new.txt"], excerpts=[rg])]))
                self.assertEqual(code, 2)

    def test_無いdirはexit2で漏れのexit1にしない(self):
        code, _, err = self.r.run("check", os.path.join(self.d, "nope"))
        self.assertEqual(code, 2, err)

    def test_story_jsonが読めなければexit2(self):
        with open(os.path.join(self.d, "story.json"), "w") as fh:
            fh.write("{")
        code, _, err = self.r.run("check", self.d)
        self.assertEqual(code, 2)
        self.assertIn("JSON として読めない", err)


class Renamed(MonstrareTest):
    def test_改名したファイルは新しいパスで指し旧いパスには案内を出す(self):
        self.r.git("mv", "a.py", "b.py")
        self.edit_a_at("b.py")
        d, _ = self.prepare()
        code, _, err = self.check(d, story_of([theme("A", ["a.py"])]))
        self.assertEqual(code, 2)
        self.assertIn("b.py への rename", err)
        old = {"path": "b.py", "side": "old", "start": 5}
        code, _, err = self.check(d, story_of([theme("A", ["b.py"], excerpts=[old])]))
        self.assertEqual(code, 0, err)
        code, _, err = self.check(d, story_of([theme("A", ["b.py"], excerpts=[dict(old, path="a.py")])]))
        self.assertEqual(code, 2)
        self.assertIn("b.py への rename", err)

    def edit_a_at(self, path):
        lines = [f"line{i}\n" for i in range(1, 21)]
        lines[4] = "changed5\n"
        self.r.write(path, "".join(lines))


class TypeChange(MonstrareTest):
    def test_種類だけ変わったファイルは1件で数え旧い側も抜粋できる(self):
        os.remove(os.path.join(self.r.path, "a.py"))
        os.symlink("keep.txt", os.path.join(self.r.path, "a.py"))
        d, out = self.prepare()
        self.assertEqual(out.count("a.py ["), 2)
        s = story_of([theme("A", ["a.py"], excerpts=[{"path": "a.py", "side": "old", "start": 1}])])
        code, _, err = self.check(d, s)
        self.assertEqual(code, 0, err)
        page = self.render(d, s)
        self.assertEqual(page.count('<span class="path">a.py</span><span class="marks">'), 1)


class Render(MonstrareTest):
    def setUp(self):
        super().setUp()
        self.edit_a()
        self.r.write("new.txt", "n1\n")
        self.d, _ = self.prepare()

    def test_図はそのまま埋まり文はエスケープされる(self):
        s = story_of([theme("<主題>", ["a.py", "new.txt"])], title="Vec<T> & x")
        page = self.render(self.d, s)
        self.assertIn(SVG, page)
        self.assertIn("<title>Vec&lt;T&gt; &amp; x</title>", page)
        self.assertIn("&lt;主題&gt;", page)
        self.assertTrue(page.startswith("<!DOCTYPE html>"))

    def test_抜粋は指した変更行を差分から載せる(self):
        rg = {"path": "a.py", "side": "new", "start": 5}
        page = self.render(self.d, story_of([theme("A", ["a.py", "new.txt"], excerpts=[rg])]))
        self.assertIn("changed5", page)
        self.assertIn("line5", page)
        self.assertNotIn("changed15", page)

    def test_止めるものが無ければそう出し止めない欄は中身があるときだけ(self):
        page = self.render(self.d, story_of([theme("A", ["a.py", "new.txt"])]))
        self.assertIn("止めるものは無い", page)
        self.assertNotIn("止めないが", page)
        s = story_of([theme("A", ["a.py", "new.txt"])], concerns={
            "blocking": [{"text": "落ちる", "how": "走らせた"}],
            "nonblocking": [{"text": "遅いかも", "how": "確かめられなかった"}],
        })
        page = self.render(self.d, s)
        self.assertIn("LGTM を止めるもの 1", page)
        self.assertIn("止めないが、知っておくとよいもの 1", page)
        self.assertNotIn("止めるものは無い", page)
        self.assertIn("確かめられなかった", page)

    def test_主題に入っていないファイルは冒頭と地図と懸念の欄に出る(self):
        page = self.render(self.d, story_of([theme("A", ["a.py"])]))
        self.assertIn("どの主題にも入っていないファイル 1 件", page)
        self.assertIn("主題なし", page)
        self.assertIn("は読んでいない", page)

    def test_地図は主題の番号を並べる(self):
        page = self.render(self.d, story_of([theme("A", ["a.py"]), theme("B", ["a.py", "new.txt"])]))
        self.assertIn('<a class="mark" href="#t1">01</a><a class="mark" href="#t2">02</a>', page)

    def test_renderも毎回検査し壊れた答えは書かない(self):
        self.put(self.d, story_of([]))
        code, _, err = self.r.run("render", self.d)
        self.assertEqual(code, 2)
        self.assertFalse(os.path.exists(os.path.join(self.d, "story.html")))

    def test_GitHubでないoriginならリンクの代わりに理由を出す(self):
        page = self.render(self.d, story_of([theme("A", ["a.py", "new.txt"])]))
        self.assertIn("origin が無い", page)

    def test_GitHubのoriginで未pushならリンクの代わりに未pushと出す(self):
        self.r.git("remote", "add", "origin", "git@github.com:o/r.git")
        self.r.git("add", "-A")
        self.r.git("commit", "-q", "-m", "c")
        d, _ = self.prepare()
        rg = {"path": "a.py", "side": "new", "start": 5}
        page = self.render(d, story_of([theme("A", ["a.py", "new.txt"], excerpts=[rg])]))
        self.assertIn('<span class="nolink">未 push</span>', page)

    def test_主題に入っていないファイルは地図の末尾に出る(self):
        page = self.render(self.d, story_of([theme("A", ["new.txt"])]))
        self.assertLess(page.index('<span class="path">new.txt</span><span class="marks">'),
                        page.index('<span class="path">a.py</span><span class="marks">'))

    def test_図の規則は借りた抜粋の枠に当たらない(self):
        rg = {"path": "a.py", "side": "new", "start": 5}
        page = self.render(self.d, story_of([theme("A", ["a.py", "new.txt"], excerpts=[rg])]))
        self.assertIn('<figure class="hunk">', page)
        self.assertIn('<figure class="pic">', page)
        self.assertNotRegex(page, r"\n(figure|figcaption) \{")

    def test_GitHubのoriginでpush済みなら抜粋にpermalinkが付く(self):
        self.r.git("remote", "add", "origin", "git@github.com:o/r.git")
        self.r.git("add", "-A")
        self.r.git("commit", "-q", "-m", "c")
        self.r.git("update-ref", "refs/remotes/origin/feature", "HEAD")
        self.r.git("update-ref", "refs/remotes/origin/main", "main")
        head = self.r.git("rev-parse", "HEAD").strip()
        d, _ = self.prepare()
        rg = {"path": "a.py", "side": "new", "start": 5}
        page = self.render(d, story_of([theme("A", ["a.py", "new.txt"], excerpts=[rg])]))
        self.assertIn(f"https://github.com/o/r/blob/{head}/a.py?plain=1#L5", page)

    def test_ダークの2か所にも図の色のトークンがある(self):
        page = self.render(self.d, story_of([theme("A", ["a.py", "new.txt"])]))
        blocks = page.split("@media (prefers-color-scheme: dark)")[1].split(':root[data-theme="dark"]')
        for block in blocks:
            for token in ("--new:", "--gone:", "--hold:", "--tint:", "--add-bg:"):
                self.assertIn(token, block.split("}")[0])

    def test_バイナリだけの変更でも地図が描ける(self):
        r = self.r
        r.git("add", "-A")
        r.git("commit", "-q", "-m", "c")
        r.git("switch", "-q", "-c", "bin")
        r.write("bin.dat", b"\x00\x01")
        d, _ = self.prepare("--base", "feature")
        page = self.render(d, story_of([theme("A", ["bin.dat"])]))
        self.assertIn("bin.dat", page)


if __name__ == "__main__":
    unittest.main()
