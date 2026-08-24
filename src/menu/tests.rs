//! メニューテーブルの不変条件と、純粋なナビゲーションのルール。
//!
//! every_command_is_reachable が要となるテストである。「すべての操作は
//! メニューから到達できる」を、コマンド追加のたびに陳腐化しかねない主張ではなく
//! 検証済みの性質にしているのはこのテストである。

use super::model::{INTENTIONALLY_UNLISTED, MENUS, MenuItem};
use super::state::{find_by_initial, first_selectable, last_selectable, step_menu, step_selection};
use crate::command_palette::{COMMANDS, CommandId};

/// メニュー内のすべてのコマンドをフラットにしたもの。
fn menu_command_ids() -> Vec<CommandId> {
    MENUS
        .iter()
        .flat_map(|m| m.items.iter())
        .filter_map(MenuItem::command)
        .collect()
}

#[test]
fn every_command_is_reachable() {
    let listed = menu_command_ids();
    let excused: Vec<CommandId> = INTENTIONALLY_UNLISTED.iter().map(|(id, _)| *id).collect();

    let missing: Vec<_> = COMMANDS
        .iter()
        .filter(|c| !listed.contains(&c.id) && !excused.contains(&c.id))
        .map(|c| c.label)
        .collect();

    assert!(
        missing.is_empty(),
        "these palette commands are on no menu and are not in INTENTIONALLY_UNLISTED: {missing:?}\n\
         Add each one to a menu in model.rs, or list it in INTENTIONALLY_UNLISTED with a reason."
    );
}

#[test]
fn unlisted_commands_are_actually_unlisted() {
    let listed = menu_command_ids();
    for (id, reason) in INTENTIONALLY_UNLISTED {
        assert!(
            !listed.contains(id),
            "{id:?} is on a menu but is also excused in INTENTIONALLY_UNLISTED ({reason})"
        );
    }
}

#[test]
fn no_command_appears_on_two_menus() {
    let listed = menu_command_ids();
    for (i, id) in listed.iter().enumerate() {
        assert!(
            !listed[i + 1..].contains(id),
            "{id:?} appears on more than one menu — a duplicated row makes the \
             menu bar ambiguous about where an operation lives"
        );
    }
}

#[test]
fn every_menu_command_exists_in_the_palette_table() {
    // メニューは execute_palette_command 経由でコマンドを実行するため、
    // COMMANDS に存在しない id のメニュー項目は、ショートカットのヒントも
    // パレット側の対応項目も持たない行になってしまう。それは2つのテーブルが
    // ずれてしまった兆候である。
    for id in menu_command_ids() {
        assert!(
            COMMANDS.iter().any(|c| c.id == id),
            "{id:?} is on a menu but missing from the palette COMMANDS table"
        );
    }
}

#[test]
fn menus_are_non_empty_and_have_selectable_rows() {
    for menu in MENUS {
        assert!(!menu.items.is_empty(), "menu {:?} is empty", menu.title);
        assert!(
            menu.items.iter().any(MenuItem::is_selectable),
            "menu {:?} has no selectable row",
            menu.title
        );
    }
}

#[test]
fn menus_have_no_leading_trailing_or_doubled_separators() {
    for menu in MENUS {
        let sel: Vec<bool> = menu.items.iter().map(MenuItem::is_selectable).collect();
        assert!(
            sel.first() == Some(&true),
            "menu {:?} starts with a separator",
            menu.title
        );
        assert!(
            sel.last() == Some(&true),
            "menu {:?} ends with a separator",
            menu.title
        );
        assert!(
            !sel.windows(2).any(|w| !w[0] && !w[1]),
            "menu {:?} has two separators in a row",
            menu.title
        );
    }
}

// ナビゲーション

fn sample() -> Vec<MenuItem> {
    vec![
        MenuItem::Command {
            id: CommandId::Quit,
            label: "Alpha",
        },
        MenuItem::Separator,
        MenuItem::Command {
            id: CommandId::OpenRepo,
            label: "Beta",
        },
        MenuItem::Command {
            id: CommandId::SwitchRepo,
            label: "Alto",
        },
    ]
}

#[test]
fn step_selection_skips_separators() {
    let items = sample();
    // 0 → (1にある区切りをスキップ) → 2
    assert_eq!(step_selection(&items, 0, 1), 2);
    // 逆方向も同様
    assert_eq!(step_selection(&items, 2, -1), 0);
}

#[test]
fn step_selection_wraps_at_both_ends() {
    let items = sample();
    assert_eq!(
        step_selection(&items, 3, 1),
        0,
        "past the end wraps to first"
    );
    assert_eq!(
        step_selection(&items, 0, -1),
        3,
        "before the start wraps to last"
    );
}

#[test]
fn step_selection_is_a_no_op_without_selectable_rows() {
    let items = vec![MenuItem::Separator, MenuItem::Separator];
    assert_eq!(step_selection(&items, 0, 1), 0);
    assert_eq!(step_selection(&items, 1, -1), 1);
}

#[test]
fn step_selection_handles_an_empty_menu() {
    assert_eq!(step_selection(&[], 0, 1), 0);
}

#[test]
fn step_selection_clamps_an_out_of_range_start() {
    // 古いインデックス(選択中にメニューテーブルが変わった場合)でも panic しないこと。
    let items = sample();
    assert!(step_selection(&items, 99, 1) < items.len());
}

#[test]
fn first_and_last_selectable_find_the_edges() {
    let items = sample();
    assert_eq!(first_selectable(&items), 0);
    assert_eq!(last_selectable(&items), 3);

    let leading_sep = vec![
        MenuItem::Separator,
        MenuItem::Command {
            id: CommandId::Quit,
            label: "Only",
        },
    ];
    assert_eq!(first_selectable(&leading_sep), 1);
    assert_eq!(last_selectable(&leading_sep), 1);
}

#[test]
fn find_by_initial_matches_case_insensitively_and_wraps() {
    let items = sample();
    // "Alpha"(0)から見ると次の 'a' は "Alto"(3) — 自分自身ではない。
    assert_eq!(find_by_initial(&items, 0, 'a'), Some(3));
    // "Alto"(3)からはラップして "Alpha"(0) に戻る。
    assert_eq!(find_by_initial(&items, 3, 'A'), Some(0));
    assert_eq!(find_by_initial(&items, 0, 'b'), Some(2));
    assert_eq!(find_by_initial(&items, 0, 'z'), None);
}

#[test]
fn find_by_initial_never_lands_on_a_separator() {
    let items = sample();
    for ch in ['a', 'b', 'z'] {
        if let Some(idx) = find_by_initial(&items, 0, ch) {
            assert!(items[idx].is_selectable());
        }
    }
}

#[test]
fn step_menu_wraps_both_ways() {
    assert_eq!(step_menu(3, 2, 1), 0);
    assert_eq!(step_menu(3, 0, -1), 2);
    assert_eq!(step_menu(0, 0, 1), 0, "no menus must not divide by zero");
}
