#!/usr/bin/env bash
# 構造の判定器。パネルが1ディレクトリに収まっているか (panels) と、
# レイヤーディレクトリが閉じているか (closure) を見る。
#
# closure が本体。panels は「移したパネル」を名指しで確かめるだけなので、
# 新しく足したパネルは名指ししない限り素通りする。閉じた集合として
# 検査すれば、既定が縦になる。
set -uo pipefail
cd "$(dirname "$0")/.."

MANIFEST=scripts/shared-layers.txt
LAYERS="src/ui src/event src/app"

fail=0
note() { printf '  %s\n' "$1"; }
bad()  { printf '  NG  %s\n' "$1"; fail=1; }

check_panel() {
  local panel="$1"; shift
  local dir="src/$panel"
  printf '[%s]\n' "$panel"

  [ -d "$dir" ] || { bad "$dir が無い"; return; }

  for role in "$@"; do
    if compgen -G "$dir/$role" > /dev/null; then
      note "OK  $dir/$role"
    else
      bad "$dir/$role が無い"
    fi
  done

  local leftovers
  leftovers=$(ls -1d src/ui/${panel}* src/event/${panel}* \
                     src/event/mouse/${panel}* src/app/${panel}* src/app/state/${panel}* 2>/dev/null || true)
  if [ -n "$leftovers" ]; then
    while IFS= read -r f; do [ -n "$f" ] && bad "旧レイヤーに残存: $f"; done <<< "$leftovers"
  else
    note "OK  旧レイヤーに残存なし"
  fi
}

# マニフェストの節を読む。パスと理由がタブ以降に分かれている前提。
manifest_section() {
  awk -v want="$1" '
    /^\[/ { section = $0; next }
    /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
    section == want { print }
  ' "$MANIFEST"
}

check_closure() {
  printf '[closure] %s\n' "$LAYERS"

  [ -f "$MANIFEST" ] || { bad "$MANIFEST が無い"; return; }

  local shared pending declared actual
  shared=$(manifest_section '[共有]')
  pending=$(manifest_section '[移行待ち]')

  # 理由の無い行は認めない。ここが「新しいものを置くときに立ち止まる」仕掛け。
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    if [ "$(printf '%s' "$line" | awk -F'\t' '{print NF}')" -lt 2 ] \
       || [ -z "$(printf '%s' "$line" | cut -f2- | tr -d '[:space:]')" ]; then
      bad "理由が書かれていない: ${line%%$'\t'*}"
    fi
  done <<< "$shared
$pending"

  declared=$(printf '%s\n%s\n' "$shared" "$pending" | cut -f1 | grep -v '^$' | sort)
  actual=$(for l in $LAYERS; do
             [ -d "$l" ] || continue
             find "$l" -mindepth 1 -maxdepth 1 \( -name '*.rs' -o -type d \) \
               | sed 's|$||' | while read -r p; do
                   [ -d "$p" ] && echo "$p/" || echo "$p"
                 done
           done | sort)

  local undeclared stale
  undeclared=$(comm -13 <(echo "$declared") <(echo "$actual"))
  stale=$(comm -23 <(echo "$declared") <(echo "$actual"))

  if [ -n "$undeclared" ]; then
    while IFS= read -r p; do
      [ -n "$p" ] && bad "マニフェストに無い: $p — パネル固有ならそのパネルのディレクトリへ。共有なら理由を添えて登録する"
    done <<< "$undeclared"
  fi
  if [ -n "$stale" ]; then
    while IFS= read -r p; do
      [ -n "$p" ] && bad "実体が無いのにマニフェストに残っている: $p"
    done <<< "$stale"
  fi

  local n_shared n_pending
  n_shared=$(echo "$shared" | grep -c '[^[:space:]]' || true)
  n_pending=$(echo "$pending" | grep -c '[^[:space:]]' || true)
  [ -z "$undeclared$stale" ] && note "OK  レイヤーは閉じている"
  note "共有 $n_shared 件 / 移行待ち $n_pending 件 (移行待ちが 0 になったら垂直化は完了)"
}

panels_all() {
  check_panel menu      'mod.rs' 'render.rs' 'input.rs' 'mouse.rs'
  check_panel reflow    'mod.rs' 'render*'   'input.rs'
  check_panel revidere  'mod.rs' 'render.rs' 'input.rs' 'state.rs'
  check_panel explorer  'mod.rs' 'render*'   'input*'   'mouse.rs'
  check_panel viewer    'mod.rs' 'render*'   'input*'   'mouse.rs'
  check_panel worktree  'mod.rs' 'render*'   'input*'   'mouse.rs'
  check_panel terminal  'mod.rs' 'render*'   'input*'
}

case "${1:-all}" in
  menu|reflow|revidere|explorer|viewer|worktree|terminal)
    case "$1" in
      menu)     check_panel menu     'mod.rs' 'render.rs' 'input.rs' 'mouse.rs' ;;
      reflow)   check_panel reflow   'mod.rs' 'render*'   'input.rs' ;;
      revidere) check_panel revidere 'mod.rs' 'render.rs' 'input.rs' 'state.rs' ;;
      explorer) check_panel explorer 'mod.rs' 'render*'   'input*'   'mouse.rs' ;;
      viewer)   check_panel viewer   'mod.rs' 'render*'   'input*'   'mouse.rs' ;;
      worktree) check_panel worktree 'mod.rs' 'render*'   'input*'   'mouse.rs' ;;
      terminal) check_panel terminal 'mod.rs' 'render*'   'input*' ;;
    esac ;;
  panels)  panels_all ;;
  closure) check_closure ;;
  all)     panels_all; check_closure ;;
  *) echo "usage: $0 [panels|closure|all|<panel>]" >&2; exit 2 ;;
esac

exit $fail
