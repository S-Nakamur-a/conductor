"""revidere.py のテスト。差分は一時リポジトリで本物の git に出させる。"""

from __future__ import annotations

import copy
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "revidere.py")
spec = importlib.util.spec_from_file_location("revidere", SCRIPT)
rv = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rv)

# conductor の入っている ~/.cargo/bin などを外し、git だけが見える PATH。
GIT_DIR = os.path.dirname(shutil.which("git"))
ENV_BASE = {
    "PATH": os.pathsep.join([GIT_DIR, "/usr/bin", "/bin"]),
    "GIT_CONFIG_NOSYSTEM": "1",
    "GIT_AUTHOR_NAME": "t",
    "GIT_AUTHOR_EMAIL": "t@example.com",
    "GIT_COMMITTER_NAME": "t",
    "GIT_COMMITTER_EMAIL": "t@example.com",
    "LANG": "C.UTF-8",
}


def read_bytes(path):
    with open(path, "rb") as fh:
        return fh.read()


class Repo:
    def __init__(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = os.path.realpath(os.path.join(self.tmp.name, "repo"))
        self.home = os.path.join(self.tmp.name, "home")
        os.makedirs(self.path)
        os.makedirs(self.home)
        self.env = dict(ENV_BASE, HOME=self.home, TMPDIR=os.path.join(self.tmp.name, "t"))
        os.makedirs(self.env["TMPDIR"])
        self.git("init", "-q", "-b", "main")

    def git(self, *args, check=True, cwd=None) -> str:
        p = subprocess.run(["git", *args], cwd=cwd or self.path, env=self.env, capture_output=True)
        if check and p.returncode != 0:
            raise AssertionError(f"git {args}: {p.stderr.decode()}")
        return p.stdout.decode("utf-8", "replace")

    def write(self, rel: str, content, mode=None):
        full = os.path.join(self.path, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "wb") as fh:
            fh.write(content if isinstance(content, bytes) else content.encode())
        if mode:
            os.chmod(full, mode)

    def commit(self, msg="c"):
        self.git("add", "-A")
        self.git("commit", "-q", "--allow-empty", "-m", msg)
        return self.git("rev-parse", "HEAD").strip()

    def branch(self, name="feature"):
        self.git("switch", "-q", "-c", name)

    def run(self, *args, cwd=None):
        p = subprocess.run(
            [sys.executable, SCRIPT, *args], cwd=cwd or self.path, env=self.env, capture_output=True
        )
        return p.returncode, p.stdout.decode(), p.stderr.decode()

    def prepare(self, *args, cwd=None):
        code, out, err = self.run("prepare", *args, cwd=cwd)
        if code != 0:
            raise AssertionError(f"prepare exit {code}: {err}")
        return out.splitlines()[0].split(": ", 1)[1], out

    def ledger(self, *args, cwd=None):
        d, _ = self.prepare(*args, cwd=cwd)
        _, files, ledger = rv.load_workdir(d)
        return files, ledger

    def close(self):
        self.tmp.cleanup()


def based():
    """main に 1 コミット、feature に切り替えた状態。"""
    r = Repo()
    r.write("keep.txt", "k\n")
    r.commit("base")
    r.branch()
    return r


class RepoTest(unittest.TestCase):
    def setUp(self):
        self.r = based()

    def tearDown(self):
        self.r.close()


class PrepareBase(unittest.TestCase):
    def test_ベースの推定はorigin_HEADを先に見る(self):
        r = based()
        try:
            base = r.git("rev-parse", "main").strip()
            r.git("update-ref", "refs/remotes/origin/trunk", base)
            r.git("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk")
            r.git("update-ref", "refs/remotes/origin/main", base)
            self.assertEqual(rv.guess_base(r.path), "origin/trunk")
        finally:
            r.close()

    def test_origin_HEADが消えたrefを指していたらorigin_mainへ落ちる(self):
        r = based()
        try:
            base = r.git("rev-parse", "main").strip()
            r.git("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/master")
            r.git("update-ref", "refs/remotes/origin/main", base)
            self.assertEqual(rv.guess_base(r.path), "origin/main")
        finally:
            r.close()

    def test_リモートが無ければローカルのmain(self):
        r = based()
        try:
            self.assertEqual(rv.guess_base(r.path), "main")
        finally:
            r.close()

    def test_origin_masterはローカルのmainより先(self):
        r = based()
        try:
            r.git("update-ref", "refs/remotes/origin/master", r.git("rev-parse", "main").strip())
            self.assertEqual(rv.guess_base(r.path), "origin/master")
        finally:
            r.close()

    def test_候補が無ければexit2(self):
        r = Repo()
        try:
            r.git("switch", "-q", "-c", "work")
            r.write("a", "a\n")
            r.commit()
            code, _, err = r.run("prepare")
            self.assertEqual(code, 2, err)
            self.assertIn("--base", err)
        finally:
            r.close()

    def test_存在しないbaseとオプションに見えるbaseはexit2(self):
        r = based()
        try:
            r.write("a", "a\n")
            for base in ["nope", "--all"]:
                code, _, err = r.run("prepare", f"--base={base}")
                self.assertEqual(code, 2, f"{base}: {err}")
        finally:
            r.close()

    def test_コミットが無ければexit2(self):
        r = Repo()
        try:
            r.write("a", "a\n")
            code, _, err = r.run("prepare", "--base=main")
            self.assertEqual(code, 2, err)
        finally:
            r.close()

    def test_差分が無ければexit3(self):
        r = based()
        try:
            code, _, err = r.run("prepare")
            self.assertEqual(code, 3, err)
            self.assertIn("差分が無い", err)
        finally:
            r.close()


class PrepareLedger(RepoTest):
    def test_削除行がハイフン2つで始まってもファイルの見出しに化けない(self):
        self.r.git("switch", "-q", "main")
        self.r.write("q.sql", "select 1;\n-- old comment\nselect 2;\n")
        self.r.commit()
        self.r.git("switch", "-q", "feature")
        self.r.git("merge", "-q", "main")
        self.r.write("q.sql", "select 1;\n++ added\nselect 2;\n")
        files, ledger = self.r.ledger()
        self.assertEqual([(f["path"], f["kind"]) for f in files], [("q.sql", "modified")])
        self.assertEqual(ledger, [("q.sql", "new", 2), ("q.sql", "old", 2)])

    def test_コミット済み_index_作業ツリー_未追跡が全部入りignoreは入らない(self):
        self.r.write(".gitignore", "ignored.txt\n")
        self.r.write("committed.txt", "c\n")
        self.r.commit()
        self.r.write("staged.txt", "s\n")
        self.r.git("add", "staged.txt")
        self.r.write("keep.txt", "k\nunstaged\n")
        self.r.write("new.txt", "n1\nn2\n")
        self.r.write("ignored.txt", "x\n")
        _, ledger = self.r.ledger()
        paths = {p for p, _, _ in ledger}
        self.assertEqual(paths, {".gitignore", "committed.txt", "staged.txt", "keep.txt", "new.txt"})
        self.assertIn(("new.txt", "new", 2), ledger)
        self.assertIn(("keep.txt", "new", 2), ledger)

    def test_削除_改名_改名と編集_バイナリ_モード_空ファイル(self):
        self.r.git("switch", "-q", "main")
        self.r.write("gone.txt", "g1\ng2\n")
        self.r.write("moved.txt", "same\n" * 5)
        self.r.write("edited.txt", "".join(f"line {i}\n" for i in range(20)))
        self.r.write("logo.bin", b"\x00\x01\x02")
        self.r.write("exec.bin", b"\x00\x04")
        self.r.write("run.sh", "echo\n")
        self.r.commit()
        self.r.git("switch", "-q", "feature")
        self.r.git("merge", "-q", "main")
        self.r.git("rm", "-q", "gone.txt")
        self.r.git("mv", "moved.txt", "moved2.txt")
        self.r.git("mv", "edited.txt", "edited2.txt")
        self.r.write("edited2.txt", "".join(f"line {i}\n" for i in range(20)).replace("line 3\n", "LINE 3\n"))
        self.r.write("logo.bin", b"\x00\x01\x03")
        self.r.write("exec.bin", b"\x00\x05", mode=0o755)
        os.chmod(os.path.join(self.r.path, "run.sh"), 0o755)
        self.r.write("empty.txt", "")
        self.r.git("add", "-A")
        files, ledger = self.r.ledger()
        kinds = {f["path"]: (f["kind"], f["old_path"], f["binary"]) for f in files}
        self.assertEqual(kinds["gone.txt"], ("deleted", None, False))
        self.assertEqual(kinds["moved2.txt"], ("renamed", "moved.txt", False))
        self.assertEqual(kinds["edited2.txt"], ("renamed", "edited.txt", False))
        self.assertEqual(kinds["logo.bin"], ("modified", None, True))
        self.assertEqual(kinds["exec.bin"], ("modified", None, True), "中身とモードの同時変更")
        self.assertEqual(kinds["run.sh"], ("mode-only", None, False))
        self.assertEqual(kinds["empty.txt"], ("added", None, False))
        for p in ["moved2.txt", "logo.bin", "exec.bin", "run.sh", "empty.txt"]:
            self.assertIn((p, "file", 0), ledger)
        self.assertEqual([x for x in ledger if x[0] == "gone.txt"], [("gone.txt", "old", 1), ("gone.txt", "old", 2)])
        self.assertEqual(
            [x for x in ledger if x[0] == "edited2.txt"], [("edited2.txt", "new", 4), ("edited2.txt", "old", 4)]
        )

    def test_台帳の要約は改名元と行の範囲を畳んで出す(self):
        self.r.write("a.txt", "1\n2\n3\n5\n")
        _, out = self.r.prepare()
        self.assertIn("a.txt [added]\n  new: 1-4\n", out)

    def test_変わったパスはエスケープされずに生のまま入る(self):
        names = ["日本語.txt", "with space.txt", 'a "q".txt', "t\tb.txt", "-dash.txt", "trailing "]
        for n in names:
            self.r.write(n, "x\n")
        committed = ["committed space.txt", "committed trailing "]
        for n in committed:
            self.r.write(n, "y\n")
        self.r.commit()
        for n in committed:
            self.r.write(n, "y\nz\n")
        self.r.write("untracked trailing ", "u\n")
        files, _ = self.r.ledger()
        self.assertEqual(sorted(f["path"] for f in files), sorted(names + committed + ["untracked trailing "]))

    def test_未追跡のハイフン始まりと記号のパスも入る(self):
        for n in ["-foo", 'b\\s.txt']:
            self.r.write(n, "x\n")
        files, _ = self.r.ledger()
        self.assertEqual(sorted(f["path"] for f in files), sorted(["-foo", "b\\s.txt"]))

    def test_改行以外の制御文字や行末なしで行番号がずれない(self):
        self.r.write("ff.txt", "a\fb\x1cc\nd\re\nlast")
        _, ledger = self.r.ledger()
        self.assertEqual(ledger, [("ff.txt", "new", 1), ("ff.txt", "new", 2), ("ff.txt", "new", 3)])

    def test_UTF8でない中身でも落ちない(self):
        self.r.write("sjis.txt", "日本語\n".encode("shift_jis"))
        _, ledger = self.r.ledger()
        self.assertEqual(ledger, [("sjis.txt", "new", 1)])

    def test_未追跡の入れ子リポジトリは載せずに続ける(self):
        nested = os.path.join(self.r.path, "nested")
        os.makedirs(nested)
        self.r.git("init", "-q", cwd=nested)
        self.r.write("nested/x", "x\n")
        self.r.write("top.txt", "t\n")
        files, _ = self.r.ledger()
        self.assertEqual([f["path"] for f in files], ["top.txt"])

    def test_未追跡のsymlinkはリンク先の1行(self):
        os.symlink("keep.txt", os.path.join(self.r.path, "link"))
        _, ledger = self.r.ledger()
        self.assertEqual(ledger, [("link", "new", 1)])

    def test_サブディレクトリから走らせても上の階層の未追跡が入る(self):
        self.r.write("sub/a.txt", "a\n")
        self.r.write("top.txt", "t\n")
        files, _ = self.r.ledger(cwd=os.path.join(self.r.path, "sub"))
        self.assertEqual(sorted(f["path"] for f in files), ["sub/a.txt", "top.txt"])

    def test_detached_HEADでも動く(self):
        self.r.write("a.txt", "a\n")
        self.r.commit()
        self.r.git("switch", "-q", "--detach")
        _, ledger = self.r.ledger()
        self.assertEqual(ledger, [("a.txt", "new", 1)])

    def test_利用者のdiff設定で見出しの接頭辞が変わっても同じ台帳(self):
        self.r.write("keep.txt", "k\nmore\n")
        self.r.git("mv", "keep.txt", "kept.txt")
        self.r.write("new.txt", "n\n")
        expected = self.r.ledger()[1]
        for key, value in [("diff.mnemonicPrefix", "true"), ("diff.noprefix", "true"), ("diff.srcPrefix", "x/")]:
            self.r.git("config", key, value)
            self.assertEqual(self.r.ledger()[1], expected, key)
            self.r.git("config", "--unset", key)

    def test_衝突中でも動き衝突マーカーが追加行に入る(self):
        self.r.write("keep.txt", "feature\n")
        self.r.commit()
        self.r.git("switch", "-q", "main")
        self.r.write("keep.txt", "main\n")
        self.r.commit()
        self.r.git("switch", "-q", "feature")
        self.r.git("merge", "-q", "main", check=False)
        d, _ = self.r.prepare("--base=main")
        with open(os.path.join(d, "diff.patch"), encoding="utf-8") as fh:
            self.assertIn("+<<<<<<<", fh.read())

    def test_prepareは利用者のindexを書かない(self):
        self.r.write("changed.txt", "c\n")
        index = os.path.join(self.r.path, ".git", "index")
        before = read_bytes(index)
        # 中身が同じで stat だけ変わったファイルは、普通の git diff なら index を書き戻させる。
        later = time.time() + 5
        os.utime(os.path.join(self.r.path, "keep.txt"), (later, later))
        self.r.prepare()
        self.assertEqual(read_bytes(index), before)

    def test_conductorが無いPATHで3つとも動く(self):
        self.assertIsNone(shutil.which("conductor", path=self.r.env["PATH"]))
        self.r.write("a.txt", "a\n")
        d, _ = self.r.prepare()
        write_review(d, [section("a", [rng("a.txt", "new", 1)])])
        self.assertEqual(self.r.run("check", d)[0], 0)
        self.assertEqual(self.r.run("render", d)[0], 0)

    def test_作業dirはリポジトリの外に毎回新しく作る(self):
        self.r.write("a.txt", "a\n")
        d1, _ = self.r.prepare()
        d2, _ = self.r.prepare()
        self.assertNotEqual(d1, d2)
        self.assertFalse(d1.startswith(self.r.path))
        self.assertEqual(self.r.git("status", "--porcelain"), "?? a.txt\n")


# ---- check ----


def rng(path, side, start=None, end=None):
    r = {"path": path, "side": side}
    if start is not None:
        r["start"] = start
    if end is not None:
        r["end"] = end
    return r


def section(title, ranges, importance="core", relations=None):
    s = {"title": title, "body": "b", "importance": importance, "reason": "r", "ranges": ranges}
    if relations is not None:
        s["relations"] = relations
    return s


def review_of(sections, impacts=None):
    return {
        "overview": {k: k for k in rv.OVERVIEW_FIELDS},
        "sections": sections,
        "impacts": impacts if impacts is not None else [],
    }


def write_review(d, sections, name="review.json", impacts=None):
    with open(os.path.join(d, name), "w", encoding="utf-8") as fh:
        json.dump(review_of(sections, impacts), fh, ensure_ascii=False)


LEDGER = [("a.rs", "new", 1), ("a.rs", "new", 2), ("a.rs", "old", 1), ("b.png", "file", 0)]


class Coverage(unittest.TestCase):
    def cov(self, sections):
        return rv.coverage(LEDGER, sections)

    def test_全位置がちょうど1節なら完全(self):
        c = self.cov([section("x", [rng("a.rs", "new", 1, 2), rng("a.rs", "old", 1)]), section("y", [rng("b.png", "file")])])
        self.assertTrue(rv.is_complete(c))
        self.assertEqual(c["classified"], 4)

    def test_3種類の破れを混ぜずに数える(self):
        c = self.cov([
            section("x", [rng("a.rs", "new", 1, 3)]),
            section("y", [rng("a.rs", "new", 2)]),
        ])
        self.assertEqual(c["unclassified"], [("a.rs", "old", 1), ("b.png", "file", 0)])
        self.assertEqual(c["conflicts"], [("a.rs", "new", 2)])
        self.assertEqual(c["unknown"], [("a.rs", "new", 3, 3)])

    def test_同じ節が範囲を重ねても取り合いにしない(self):
        c = self.cov([section("x", [rng("a.rs", "new", 1, 2), rng("a.rs", "new", 2)])])
        self.assertEqual(c["conflicts"], [])

    def test_台帳に無いファイル単位の位置は作り話(self):
        c = self.cov([section("x", [rng("c.png", "file")])])
        self.assertEqual(c["unknown"], [("c.png", "file", 0, 0)])

    def test_endが桁違いでもすぐ返る(self):
        t = time.time()
        c = self.cov([section("x", [rng("a.rs", "new", 1, 10**9)])])
        self.assertLess(time.time() - t, 1)
        self.assertEqual(c["unknown"], [("a.rs", "new", 3, 10**9)])

    def test_差し戻し用の書式は行を1つずつ並べる(self):
        self.assertEqual(
            rv.gap_summary([("b.png", "file", 0), ("a.rs", "new", 4), ("a.rs", "new", 5)]),
            "b.png file\na.rs new: 4, 5\n",
        )


class Validate(unittest.TestCase):
    def broken(self, mutate):
        r = review_of([section("x", [rng("a.rs", "new", 1, 2)])], [
            {"feature": "f", "change": "c", "verify": "v", "confidence": "fact"}
        ])
        mutate(r)
        with self.assertRaises(rv.Failure) as cm:
            rv.validate(r)
        return str(cm.exception)

    def test_壊れた答えはどの欄かを名指しして弾く(self):
        s = lambda r: r["sections"][0]
        rg = lambda r: s(r)["ranges"][0]
        cases = {
            "start が end より後": (lambda r: rg(r).update(start=3, end=2), "行範囲"),
            "start が無い": (lambda r: rg(r).pop("start"), "行範囲"),
            "start が true": (lambda r: rg(r).update(start=True), "行範囲"),
            "start が文字列": (lambda r: rg(r).update(start="3"), "行範囲"),
            "start が小数": (lambda r: rg(r).update(start=3.0), "行範囲"),
            "start が負": (lambda r: rg(r).update(start=-1), "行範囲"),
            "side が無効": (lambda r: rg(r).update(side="both"), "side"),
            "path が空": (lambda r: rg(r).update(path=" "), "path"),
            "sections が空": (lambda r: r.update(sections=[]), "sections が空"),
            "title が空白": (lambda r: s(r).update(title="  "), "title"),
            "reason が無い": (lambda r: s(r).pop("reason"), "reason"),
            "importance が無効": (lambda r: s(r).update(importance="high"), "importance"),
            "overview が空白": (lambda r: r["overview"].update(scope=" "), "overview.scope"),
            "feature が空": (lambda r: r["impacts"][0].update(feature=""), "feature"),
            "fact で change が空": (lambda r: r["impacts"][0].update(change=""), "fact"),
            "confidence が無効": (lambda r: r["impacts"][0].update(confidence="maybe"), "confidence"),
            "concerns が文字列": (lambda r: s(r).update(concerns="c"), "concerns"),
            "concerns に空白": (lambda r: s(r).update(concerns=["c", " "]), "concerns"),
        }
        for name, (mutate, needle) in cases.items():
            with self.subTest(name):
                self.assertIn(needle, self.broken(mutate))

    def test_推測なら変わることが空でも通る(self):
        r = review_of([section("x", [])], [{"feature": "f", "change": "", "verify": "v", "confidence": "guess"}])
        rv.validate(r)

    def test_主の関係が2つあっても弾かない(self):
        rels = [{"to": "y", "reason": "r", "primary": True}, {"to": "z", "reason": "r", "primary": True}]
        rv.validate(review_of([section("x", [], relations=rels)]))


class CheckCommand(RepoTest):
    def setUp(self):
        super().setUp()
        self.r.write("a.txt", "1\n2\n")
        self.d, _ = self.r.prepare()

    def test_完全ならexit0_もれがあればexit1で位置を出す(self):
        write_review(self.d, [section("x", [rng("a.txt", "new", 1, 2)])])
        self.assertEqual(self.r.run("check", self.d)[0], 0)
        write_review(self.d, [section("x", [rng("a.txt", "new", 1)])])
        code, out, _ = self.r.run("check", self.d)
        self.assertEqual(code, 1)
        self.assertIn("a.txt new: 2", out)

    def test_読めないJSONはexit2(self):
        with open(os.path.join(self.d, "review.json"), "w") as fh:
            fh.write("```json\n{}\n```")
        code, _, err = self.r.run("check", self.d)
        self.assertEqual(code, 2)
        self.assertIn("JSON", err)

    def test_直した答えは説明もれを減らしたときだけ採る(self):
        write_review(self.d, [section("first", [rng("a.txt", "new", 1)])])
        write_review(self.d, [section("worse", [])], "review.retry.json")
        self.assertEqual(self.r.run("check", self.d, "--retry")[0], 1)
        self.assertEqual(rv.load_review(os.path.join(self.d, "review.json"))["sections"][0]["title"], "first")

        write_review(self.d, [section("better", [rng("a.txt", "new", 1, 2)])], "review.retry.json")
        code, out, _ = self.r.run("check", self.d, "--retry")
        self.assertEqual(code, 0, out)
        self.assertEqual(rv.load_review(os.path.join(self.d, "review.json"))["sections"][0]["title"], "better")
        self.assertTrue(os.path.exists(os.path.join(self.d, "review.first.json")))

    def test_直した答えが読めなければ最初の答えで進む(self):
        write_review(self.d, [section("first", [rng("a.txt", "new", 1)])])
        with open(os.path.join(self.d, "review.retry.json"), "w") as fh:
            fh.write("{")
        code, out, _ = self.r.run("check", self.d, "--retry")
        self.assertEqual(code, 1)
        self.assertIn("最初の答えのまま", out)


class PublishedCommand(RepoTest):
    def setUp(self):
        super().setUp()
        self.r.write("a.txt", "1\n")
        self.d, _ = self.r.prepare()
        self.meta = os.path.join(self.d, "meta.json")

    def test_URLをmeta_jsonに書き足し_出し直しは上書きし_ほかの欄は残す(self):
        before = rv.read_json(self.meta)
        self.assertEqual(self.r.run("published", self.d, "https://claude.ai/artifact/a")[:2], (0, ""))
        self.assertEqual(self.r.run("published", self.d, "https://claude.ai/artifact/b")[0], 0)
        after = rv.read_json(self.meta)
        self.assertEqual(after.pop("artifact_url"), "https://claude.ai/artifact/b")
        self.assertEqual(after, before)
        self.assertFalse(os.path.exists(self.meta + ".tmp"), "一時ファイルは置き換えで消える")

    def test_ArtifactでないURLと読めないmetaはexit2で何も書かない(self):
        before = read_bytes(self.meta)
        code, _, err = self.r.run("published", self.d, "https://example.com/x")
        self.assertEqual(code, 2)
        self.assertIn("Artifact の URL ではない", err)
        self.assertEqual(read_bytes(self.meta), before)
        code, _, err = self.r.run("published", os.path.join(self.d, "missing"), "https://claude.ai/artifact/a")
        self.assertEqual(code, 2)
        self.assertIn("を読めない", err)


# ---- render ----


META = {
    "root": "/r",
    "branch": "feature",
    "base_ref": "origin/main",
    "base_oid": "b" * 40,
    "head_oid": "h" * 40,
    "github": "o/r",
    "github_note": None,
    "head_pushed": True,
    "base_pushed": True,
    "dirty": [],
}


def files_of(entries):
    return [dict({"old_path": None, "hunks": []}, **e) for e in entries]


class Permalink(unittest.TestCase):
    F = files_of([{"path": "src/a b#.md", "kind": "modified"}])[0]

    def link(self, side, start=0, end=0, f=None, **meta):
        return rv.link(dict(META, **meta), f or self.F, side, start, end)

    def test_push済みで未コミットでない新側はheadの行へ(self):
        self.assertEqual(
            self.link("new", 3, 5)[0], "https://github.com/o/r/blob/" + "h" * 40 + "/src/a%20b%23.md?plain=1#L3-L5"
        )
        self.assertTrue(self.link("new", 3, 3)[0].endswith("#L3"))

    def test_未コミットと未pushはリンクを張らずに理由を出す(self):
        self.assertEqual(self.link("new", 1, 1, dirty=["src/a b#.md"]), (None, "未コミット"))
        self.assertEqual(self.link("new", 1, 1, head_pushed=False), (None, "未 push"))
        self.assertEqual(self.link("file", dirty=["src/a b#.md"]), (None, "未コミット"))

    def test_旧側は改名前のパスでベースの行へ(self):
        f = files_of([{"path": "new.rs", "old_path": "old.rs", "kind": "renamed"}])[0]
        self.assertEqual(self.link("old", 2, 2, f=f)[0], "https://github.com/o/r/blob/" + "b" * 40 + "/old.rs?plain=1#L2")
        self.assertEqual(self.link("old", 2, 2, f=f, base_pushed=False), (None, "ベースが未 push"))

    def test_ファイル単位は行の印を付けない(self):
        self.assertTrue(self.link("file")[0].endswith("/src/a%20b%23.md"))

    def test_GitHubでなければリンクも理由も出さない(self):
        self.assertEqual(self.link("new", 1, 1, github=None), (None, ""))


class GithubUrl(unittest.TestCase):
    def test_URLの形からowner_repoだけを取り出す(self):
        for url in [
            "https://github.com/o/r.git",
            "https://github.com/o/r",
            "https://github.com/o/r/",
            "https://user:ghp_secret@github.com/o/r.git",
            "git@github.com:o/r.git",
            "ssh://git@github.com/o/r.git",
            "ssh://git@ssh.github.com:443/o/r.git",
            "https://GitHub.com/o/r.git",
        ]:
            self.assertEqual(rv.github_repo(url), "o/r", url)

    def test_GitHubと判別できないものはNone(self):
        for url in ["git@github-work:o/r.git", "https://gitlab.com/o/r.git", "/srv/repo.git", "https://github.com/o"]:
            self.assertIsNone(rv.github_repo(url), url)


class RenderPage(RepoTest):
    def test_説明のない変更も抜粋つきで最後に出る(self):
        self.r.write("a.txt", "explained\nleft out\n")
        d, _ = self.r.prepare()
        write_review(d, [section("x", [rng("a.txt", "new", 1)])])
        page = self.render(d)
        self.assertIn('id="unexplained"', page)
        self.assertIn("left out", page.split('id="unexplained"')[1])
        self.assertIn("説明なし 1 件", page)

    def test_台帳に無い位置だけを指す節も消さずに出す(self):
        self.r.write("a.txt", "a\n")
        d, _ = self.r.prepare()
        write_review(d, [section("real", [rng("a.txt", "new", 1)]), section("ghost", [rng("a.txt", "new", 9)])])
        page = self.render(d)
        self.assertIn("ghost", page)
        self.assertIn("台帳に無い位置だけを指している", page)

    def test_取り合った行は両方の節に出る(self):
        self.r.write("a.txt", "shared\n")
        d, _ = self.r.prepare()
        write_review(d, [section("x", [rng("a.txt", "new", 1)]), section("y", [rng("a.txt", "new", 1)])])
        page = self.render(d)
        self.assertEqual(page.count('class="add dup"'), 2)

    def test_差分と文のタグはエスケープする(self):
        self.r.write("a.txt", "</script><b>&\n")
        d, _ = self.r.prepare()
        s = section("<img src=x onerror=alert(1)>", [rng("a.txt", "new", 1)])
        s["body"] = "<script>alert(1)</script>"
        write_review(d, [s])
        page = self.render(d)
        self.assertNotIn("<img src=x", page)
        self.assertNotIn("<script>alert", page)
        self.assertNotIn("</script><b>", page)
        self.assertIn("&lt;/script&gt;&lt;b&gt;&amp;", page)

    def test_認証情報つきのoriginはmetaにもHTMLにも出ない(self):
        self.r.git("remote", "add", "origin", "https://user:ghp_secret@github.com/o/r.git")
        self.r.write("a.txt", "a\n")
        d, _ = self.r.prepare("--base=main")
        write_review(d, [section("x", [rng("a.txt", "new", 1)])])
        page = self.render(d)
        meta = read_bytes(os.path.join(d, "meta.json")).decode()
        for text in (page, meta):
            self.assertNotIn("ghp_secret", text)
        self.assertEqual(json.loads(meta)["github"], "o/r")

    def test_origin以外にだけGitHubがあればリンクは無い(self):
        self.r.git("remote", "add", "upstream", "https://github.com/o/r.git")
        self.r.write("a.txt", "a\n")
        d, _ = self.r.prepare("--base=main")
        write_review(d, [section("x", [rng("a.txt", "new", 1)])])
        page = self.render(d)
        self.assertNotIn("github.com/o/r/blob", page)
        self.assertIn("origin が無い", page)

    def test_push済みの判定はoriginのrefで見る(self):
        self.r.git("remote", "add", "origin", "git@github.com:o/r.git")
        self.r.write("a.txt", "a\n")
        head = self.r.commit()
        d, _ = self.r.prepare("--base=main")
        self.assertFalse(json.loads(read_bytes(os.path.join(d, "meta.json")))["head_pushed"])
        self.r.git("update-ref", "refs/remotes/origin/feature", head)
        later = time.time() + 5
        os.utime(os.path.join(self.r.path, "a.txt"), (later, later))
        d, _ = self.r.prepare("--base=main")
        write_review(d, [section("x", [rng("a.txt", "new", 1)])])
        self.assertIn(f"github.com/o/r/blob/{head}/a.txt?plain=1#L1", self.render(d))

    def test_削除したバイナリはベースへ_足したバイナリはheadへリンクする(self):
        self.r.git("remote", "add", "origin", "git@github.com:o/r.git")
        self.r.git("switch", "-q", "main")
        self.r.write("img.png", b"\x00\x01\x02")
        base = self.r.commit()
        self.r.git("update-ref", "refs/remotes/origin/main", base)
        self.r.git("switch", "-q", "feature")
        self.r.git("merge", "-q", "main")
        self.r.git("rm", "-q", "img.png")
        self.r.write("new.png", b"\x00\x09")
        head = self.r.commit()
        self.r.git("update-ref", "refs/remotes/origin/feature", head)
        d, out = self.r.prepare("--base=main")
        self.assertIn("img.png [binary]", out)
        write_review(d, [section("x", [rng("img.png", "file"), rng("new.png", "file")])])
        page = self.render(d)
        self.assertIn(f"github.com/o/r/blob/{base}/img.png", page)
        self.assertNotIn(f"blob/{head}/img.png", page)
        self.assertIn(f"github.com/o/r/blob/{head}/new.png", page)
        self.assertIn("バイナリの削除", page)
        self.assertIn("バイナリの追加", page)

    def test_prepareの後にコミットしてもrenderの結果は変わらない(self):
        self.r.write("a.txt", "a\n")
        d, _ = self.r.prepare()
        write_review(d, [section("x", [rng("a.txt", "new", 1)])])
        first = self.render(d)
        self.r.write("b.txt", "b\n")
        self.r.commit()
        self.assertEqual(self.render(d), first)

    def test_壊れたreview_jsonならHTMLを書かない(self):
        self.r.write("a.txt", "a\n")
        d, _ = self.r.prepare()
        write_review(d, [])
        code, _, err = self.r.run("render", d)
        self.assertEqual(code, 2, err)
        self.assertFalse(os.path.exists(os.path.join(d, "review.html")))

    def test_関係は同名なら先の節へ_自分自身と存在しない相手はリンクにしない(self):
        self.r.write("a.txt", "a\nb\nc\n")
        d, _ = self.r.prepare()
        write_review(d, [
            section("dup", [rng("a.txt", "new", 1)]),
            section("dup", [rng("a.txt", "new", 2)], importance="minor"),
            section("t", [rng("a.txt", "new", 3)], importance="follow", relations=[
                {"to": "dup", "reason": "r", "primary": True},
                {"to": "t", "reason": "self", "primary": True},
                {"to": "missing", "reason": "r", "primary": True},
            ]),
        ])
        page = self.render(d)
        rels = page.split('class="rels"')[1].split("</ul>")[0]
        self.assertIn('href="#s0"', rels)
        self.assertNotIn("self", rels)
        self.assertIn("(見つからない節)", rels)
        self.assertEqual(rels.count('class="primary"'), 1)

    def test_節は重要度順で長いファイルは節の行の前後だけ切り出す(self):
        self.r.write("big.txt", "".join(f"line {i}\n" for i in range(1, 101)))
        d, _ = self.r.prepare()
        write_review(d, [
            section("later", [rng("big.txt", "new", 1, 49)], importance="minor"),
            section("first", [rng("big.txt", "new", 50, 100)], importance="core"),
        ])
        page = self.render(d)
        self.assertLess(page.index(">first<"), page.index(">later<"))
        first = page.split('id="s0"')[1].split('id="s1"')[0]
        self.assertIn("line 47", first)
        self.assertNotIn("line 46", first)
        self.assertIn("…", first)

    def test_懸念が無く検査が通ればLGTM_本題以外には札を付けない(self):
        self.r.write("a.txt", "a\nb\n")
        d, _ = self.r.prepare()
        write_review(d, [section("x", [rng("a.txt", "new", 1)]), section("y", [rng("a.txt", "new", 2)], importance="minor")])
        page = self.render(d)
        verdict = page.split('class="verdict"')[1].split("</section>")[0]
        self.assertIn("LGTM", verdict)
        self.assertEqual(page.split('id="s0"')[1].split("</header>")[0].count("LGTM"), 1)
        self.assertNotIn("LGTM", page.split('id="s1"')[1].split("</header>")[0])

    def test_懸念はどの節のものも冒頭に節へのリンクつきで集まる(self):
        self.r.write("a.txt", "a\nb\n")
        d, _ = self.r.prepare()
        core = section("x", [rng("a.txt", "new", 1)])
        core["concerns"] = ["一つ目", "二つ目"]
        follow = section("y", [rng("a.txt", "new", 2)], importance="follow")
        follow["concerns"] = ["三つ目"]
        write_review(d, [core, follow])
        page = self.render(d)
        verdict = page.split('class="verdict"')[1].split("</section>")[0]
        self.assertIn("懸念あり 3", verdict)
        self.assertNotIn("LGTM", verdict)
        for text in ("一つ目", "二つ目", "三つ目"):
            self.assertIn(text, verdict)
        self.assertEqual(verdict.count('href="#s1"'), 1)
        self.assertIn("懸念あり 1", page.split('id="s1"')[1].split("</header>")[0])
        self.assertIn("懸念あり 2", page.split('id="s0"')[1].split("</header>")[0])
        self.assertNotIn("LGTM", page.split('id="s0"')[1].split("</header>")[0])

    def test_説明もれがあれば懸念が無くてもLGTMにしない(self):
        self.r.write("a.txt", "a\nb\n")
        d, _ = self.r.prepare()
        write_review(d, [section("x", [rng("a.txt", "new", 1)])])
        verdict = self.render(d).split('class="verdict"')[1].split("</section>")[0]
        self.assertNotIn("LGTM", verdict)
        self.assertIn("判定できない", verdict)

    def render(self, d):
        code, out, err = self.r.run("render", d)
        self.assertEqual(code, 0, err)
        with open(out.strip(), encoding="utf-8") as fh:
            return fh.read()


if __name__ == "__main__":
    unittest.main()
