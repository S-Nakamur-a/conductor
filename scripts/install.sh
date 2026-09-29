#!/bin/sh
# linked worktree から入れたバイナリは worktree 名で分ける。行き先が 1 つだと、複数の
# worktree で実装が終わったとき最後の install が黙って勝ち、どれを動かしているのか
# 分からなくなる。
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

if [ "$(git rev-parse --git-dir)" = "$(git rev-parse --git-common-dir)" ]; then
    cargo install --path .
    exit 0
fi

# cargo install に名前は指定できないので、置き場を借りてから名前を付けて移す。
# ~/.cargo/bin へ直に入れると、移す前に本物の conductor を潰す。
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
mkdir -p "$cargo_home/bin"
# rename で済ませたいので同じファイルシステムに置く。走っているバイナリにも被せられる。
stage=$(mktemp -d "$cargo_home/install-stage.XXXXXX")
trap 'rm -rf "$stage"' EXIT
cargo install --path . --root "$stage"

dest="$cargo_home/bin/conductor-$(basename "$repo")"
mv "$stage/bin/conductor" "$dest"
echo "installed $dest"
