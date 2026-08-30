#!/usr/bin/env bash
# パネルの state / render / input が 1 ディレクトリに同居しているかを判定する。
#
# 垂直分割の受け入れ条件そのもの。パネル固有のファイルが src/ui/ や src/event/ に
# 残っていれば失敗させる。
set -uo pipefail
cd "$(dirname "$0")/.."

fail=0
note() { printf '  %s\n' "$1"; }
bad()  { printf '  NG  %s\n' "$1"; fail=1; }

check_panel() {
  local panel="$1"; shift
  local dir="src/$panel"
  printf '[%s]\n' "$panel"

  [ -d "$dir" ] || { bad "$dir が無い"; return; }

  # 期待する役割ファイルが同居しているか
  for role in "$@"; do
    if compgen -G "$dir/$role" > /dev/null; then
      note "OK  $dir/$role"
    else
      bad "$dir/$role が無い"
    fi
  done

  # パネル固有のファイルが旧レイヤーに残っていないか
  local leftovers
  leftovers=$(ls -1 src/ui/${panel}* src/ui/${panel}_*/ src/event/${panel}* src/event/${panel}_*/ \
                    src/event/mouse/${panel}* src/app/${panel}* src/app/state/${panel}* 2>/dev/null || true)
  if [ -n "$leftovers" ]; then
    while IFS= read -r f; do [ -n "$f" ] && bad "旧レイヤーに残存: $f"; done <<< "$leftovers"
  else
    note "OK  旧レイヤーに残存なし"
  fi
}

case "${1:-all}" in
  menu)     check_panel menu     'mod.rs' 'render.rs' 'input.rs' 'mouse.rs' ;;
  reflow)   check_panel reflow   'mod.rs' 'render*'   'input.rs' ;;
  revidere) check_panel revidere 'mod.rs' 'render.rs' 'input.rs' 'state.rs' ;;
  all)
    check_panel menu     'mod.rs' 'render.rs' 'input.rs' 'mouse.rs'
    check_panel reflow   'mod.rs' 'render*'   'input.rs'
    check_panel revidere 'mod.rs' 'render.rs' 'input.rs' 'state.rs'
    ;;
  *) echo "usage: $0 [menu|reflow|revidere|all]" >&2; exit 2 ;;
esac

exit $fail
