#!/usr/bin/env python3
"""crates/ の版に worktree 固有の接尾辞を付け外しする。

版は -C metadata に入るので、全 worktree で同じ版のクレートは共有 target-dir
(~/.cargo/config.toml の target-dir) で同名の成果物になり、後からビルドした
worktree のものに黙って差し替わる。cargo は差し替わった成果物をそのまま走らせて
exit 0 を返すので、他人のテストが自分のテストとして緑になる。

root の conductor は触らない。依存の版は下流のハッシュに伝播するので、メンバーを
分ければ root の成果物名も分かれ、conductor -V は本物の版のままでいられる。
"""

import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SUFFIX = re.compile(r"-wt\.[0-9A-Za-z.-]+$")
PACKAGE_VERSION = re.compile(
    r"(?P<head>^\[package\]$.*?^version = \")(?P<version>[^\"]+)(?P<tail>\")",
    re.MULTILINE | re.DOTALL,
)


def label() -> str:
    """semver の prerelease に使える worktree 名。"""
    name = re.sub(r"[^0-9A-Za-z-]+", "-", REPO.name).strip("-")
    return name or "wt"


def manifests() -> list[Path]:
    return sorted(REPO.glob("crates/*/Cargo.toml"))


def rewrite(path: Path, to_version) -> str | None:
    text = path.read_text()
    match = PACKAGE_VERSION.search(text)
    if match is None:
        raise SystemExit(f"{path}: [package] の version が読めない")
    current = match.group("version")
    wanted = to_version(current)
    if wanted == current:
        return None
    path.write_text(
        text[: match.start("version")] + wanted + text[match.end("version") :]
    )
    return f"{path.parent.name}: {current} -> {wanted}"


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in ("stamp", "reset"):
        raise SystemExit("usage: wt-version.py stamp|reset")
    if sys.argv[1] == "stamp":
        mark = f"-wt.{label()}"
        to_version = lambda v: v if SUFFIX.search(v) else v + mark  # noqa: E731
    else:
        to_version = lambda v: SUFFIX.sub("", v)  # noqa: E731

    changed = [line for path in manifests() if (line := rewrite(path, to_version))]
    print("\n".join(changed) if changed else "版は既にその形になっている")
    # Cargo.lock を版に追従させる。ビルドはしない。刻み済みで呼ばれても、
    # 前回 lock の同期だけ失敗している場合があるので毎回走らせる。
    subprocess.run(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=REPO,
        check=True,
        stdout=subprocess.DEVNULL,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
