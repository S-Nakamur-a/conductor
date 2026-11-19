"""delineare.py のテスト。prepare は一時リポジトリで本物の git に走らせる (道具は revidere のテストから借りる)。"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "delineare.py")
EXAMPLE = os.path.join(HERE, "example.plan.json")
sys.path.insert(0, os.path.join(HERE, "..", "..", "revidere", "scripts"))
import test_revidere as trv  # noqa: E402

SVG = '<svg viewBox="0 0 100 40" role="img" aria-label="x"><rect class="box" x="1" y="1" width="50" height="20"/></svg>'


def example() -> dict:
    with open(EXAMPLE, encoding="utf-8") as fh:
        return json.load(fh)


def minimal() -> dict:
    return {
        "title": "t",
        "date": "2026-11-19",
        "scope": "s",
        "premises": ["p"],
        "verified": [{"text": "v", "how": "読んだ"}],
        "make": ["m"],
        "skip": [{"text": "k", "why": "w"}],
        "steps": [{"title": "st", "verify": "vf"}],
    }


class Repo(trv.Repo):
    def __init__(self):
        super().__init__()
        self.env["GIT_CEILING_DIRECTORIES"] = self.tmp.name

    def run(self, *args, cwd=None, script=SCRIPT):
        p = subprocess.run([sys.executable, script, *args], cwd=cwd or self.path, env=self.env, capture_output=True)
        return p.returncode, p.stdout.decode(), p.stderr.decode()


class DelineareTest(unittest.TestCase):
    def setUp(self):
        self.r = Repo()

    def tearDown(self):
        self.r.close()

    def prepare(self, cwd=None, **kw):
        code, out, err = self.r.run("prepare", cwd=cwd, **kw)
        self.assertEqual(code, 0, err)
        return out.splitlines()[0].split(": ", 1)[1]

    def meta(self, d):
        with open(os.path.join(d, "meta.json"), encoding="utf-8") as fh:
            return json.load(fh)

    def put(self, d, plan):
        with open(os.path.join(d, "plan.json"), "w", encoding="utf-8") as fh:
            json.dump(plan, fh, ensure_ascii=False)

    def check(self, plan):
        d = self.prepare()
        self.put(d, plan)
        return self.r.run("check", d)

    def render(self, plan):
        d = self.prepare()
        self.put(d, plan)
        code, out, err = self.r.run("render", d)
        self.assertEqual(code, 0, err)
        self.assertEqual(out.strip(), os.path.join(d, "plan.html"))
        with open(out.strip(), encoding="utf-8") as fh:
            return fh.read()


class Prepare(DelineareTest):
    def committed(self):
        self.r.write("keep.txt", "k\n")
        self.r.commit("base")

    def test_GitHubのoriginならrepoはowner_nameでbranchとHEADも残る(self):
        self.committed()
        self.r.git("remote", "add", "origin", "git@github.com:o/r.git")
        d = self.prepare()
        meta = self.meta(d)
        self.assertEqual(meta["repo"], "o/r")
        self.assertEqual(meta["branch"], "main")
        self.assertEqual(meta["head_oid"], self.r.git("rev-parse", "HEAD").strip())
        self.assertEqual(meta["root"], self.r.path)

    def test_GitHubでないoriginか無いならrepoはリポジトリのdir名(self):
        self.committed()
        self.assertEqual(self.meta(self.prepare())["repo"], "repo")
        self.r.git("remote", "add", "origin", "git@gitlab.com:o/r.git")
        self.assertEqual(self.meta(self.prepare())["repo"], "repo")

    def test_コミットが無いリポジトリでもbranchとHEADを空にして作る(self):
        meta = self.meta(self.prepare())
        self.assertIsNone(meta["head_oid"])
        self.assertEqual(meta["repo"], "repo")

    def test_gitの外ならmetaは空で作業dirは作る(self):
        outside = os.path.join(self.r.tmp.name, "plain")
        os.makedirs(outside)
        self.assertEqual(self.meta(self.prepare(cwd=outside)), {})

    def test_作業dirは名前もページもrevidereの印に数えられない(self):
        self.committed()
        d = self.prepare()
        self.put(d, example())
        code, _, err = self.r.run("render", d)
        self.assertEqual(code, 0, err)
        self.assertTrue(os.path.basename(d).startswith("delineare-"))
        self.assertEqual(sorted(os.listdir(d)), ["meta.json", "plan.html", "plan.json"])

    def test_skillのdirがリンクでもrevidereに届く(self):
        with tempfile.TemporaryDirectory() as t:
            link = os.path.join(t, "delineare")
            os.symlink(os.path.dirname(HERE), link)
            code, _, err = self.r.run("prepare", script=os.path.join(link, "scripts", "delineare.py"))
        self.assertEqual(code, 0, err)


class Check(DelineareTest):
    def test_見本のplanは通る(self):
        code, out, err = self.check(example())
        self.assertEqual(code, 0, err)
        self.assertIn("通った", out)

    def test_planが無いか読めなければexit2(self):
        d = self.prepare()
        code, _, err = self.r.run("check", d)
        self.assertEqual(code, 2)
        self.assertIn("delineare:", err)
        self.assertIn("が無い", err)
        with open(os.path.join(d, "plan.json"), "w") as fh:
            fh.write("{")
        code, _, err = self.r.run("check", d)
        self.assertEqual(code, 2)
        self.assertIn("JSON として読めない", err)

    def test_欠けた欄や形の違う欄は名指しでexit2(self):
        cases = {
            "title が空": lambda p: p.update(title=" "),
            "date が空": lambda p: p.pop("date"),
            "scope が空": lambda p: p.update(scope=""),
            "status が": lambda p: p.update(status="later"),
            "status が ": lambda p: p.update(status=["draft"]),
            "premises が": lambda p: p.update(premises=[""]),
            "verified[0].how": lambda p: p["verified"][0].update(how="見た"),
            "skip[0] に text と why が無い": lambda p: p["skip"][0].pop("why"),
            "steps が空": lambda p: p.update(steps=[]),
            "に verify が無い": lambda p: p["steps"][0].pop("verify"),
            "figures[0] に svg と caption が無い": lambda p: p.update(figures=[{"svg": SVG}]),
            "size[0] に label と value が無い": lambda p: p.update(size=[{"label": "x"}]),
            "一番外が JSON オブジェクトでない": lambda p: None,
        }
        for want, spoil in cases.items():
            with self.subTest(want=want):
                plan = minimal()
                spoil(plan)
                code, _, err = self.check([] if want.startswith("一番外") else plan)
                self.assertEqual(code, 2, err)
                self.assertIn("delineare: ", err)
                self.assertIn(want.strip(), err)

    def test_sizeの値は0でもfalseでも通る(self):
        for value in (0, False):
            with self.subTest(value=value):
                plan = minimal()
                plan["size"] = [{"label": "x", "value": value}]
                code, _, err = self.check(plan)
                self.assertEqual(code, 0, err)

    def test_必須の配列が空でも通る(self):
        plan = minimal()
        for key in ("premises", "verified", "make", "skip"):
            plan[key] = []
        code, _, err = self.check(plan)
        self.assertEqual(code, 0, err)


class Render(DelineareTest):
    def test_見本のplanは全部の塊と手順の確かめ方が出る(self):
        page = self.render(dict(example(), figures=[{"svg": SVG, "caption": "全体の図"}]))
        for heading in ("どこまでやるか", "前提", "確かめたこと", "作るもの", "作らないもの", "やること", "決めていないこと", "全体の形"):
            self.assertIn(heading, page)
        self.assertIn('<dl class="size">', page)
        self.assertIn('<span class="dot">4</span>', page)
        self.assertEqual(page.count("<b>確かめ方</b>"), 4)
        self.assertIn(SVG, page)

    def test_省略できる欄を省くかnullにすると描くときもその塊が出ない(self):
        plan = minimal()
        plan["open"] = None
        plan["steps"][0]["files"] = None
        d = self.prepare()
        self.put(d, plan)
        self.assertEqual(self.r.run("check", d)[0], 0)
        page = self.render(plan)
        for absent in ("決めていないこと", '<dl class="size">', "全体の形", '<div class="files">'):
            self.assertNotIn(absent, page)

    def test_必須の配列が空なら件数0の塊になる(self):
        plan = minimal()
        plan["make"] = []
        self.assertIn('作るもの<span class="cnt">0</span>', self.render(plan))

    def test_文はエスケープされる(self):
        plan = minimal()
        plan["title"] = "<script>x</script>"
        plan["steps"][0]["files"] = ["<b>.py"]
        page = self.render(plan)
        self.assertNotIn("<script>x</script>", page)
        self.assertIn("&lt;script&gt;x&lt;/script&gt;", page)
        self.assertIn("&lt;b&gt;.py", page)

    def test_statusごとに札の色と日本語が変わる(self):
        for status, ja in (("draft", "未着手"), ("doing", "進行中"), ("done", "完了"), ("dropped", "取りやめ")):
            with self.subTest(status=status):
                page = self.render(dict(minimal(), status=status))
                self.assertIn(f'<span class="pill {status}">{ja}</span>', page)

    def test_確かめられなかったことは確かめた印を付けず件数にも入れない(self):
        plan = minimal()
        plan["verified"].append({"text": "試せなかった", "how": "確かめられなかった"})
        page = self.render(plan)
        self.assertIn('確かめたこと<span class="cnt">1</span>', page)
        unsure = page.split("試せなかった")[0].rsplit("<li>", 1)[1]
        self.assertIn('class="mk unsure"', unsure)
        self.assertNotIn("✓", unsure)

    def test_図と札の見た目がstyleに入り波括弧が残らない(self):
        page = self.render(minimal())
        style = page.split("<style>")[1].split("</style>")[0]
        for rule in (":root {", ".fig svg .new {", ".how {", ".mk.unsure {"):
            self.assertIn(rule, style)
        self.assertNotIn("{{", style)
        self.assertNotIn("prefers-color-scheme: dark", page)

    def test_metaが無ければexit2(self):
        d = self.prepare()
        self.put(d, minimal())
        os.remove(os.path.join(d, "meta.json"))
        code, _, err = self.r.run("render", d)
        self.assertEqual(code, 2)
        self.assertIn("delineare:", err)


if __name__ == "__main__":
    unittest.main()
