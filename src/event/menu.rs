//! メニューバーがフォーカスされている間のキーボード処理。
//!
//! メニューがアクティブになるとすべてのキーを消費するので、ここでの文字
//! ジャンプショートカットは、どのパネルのバインドとも衝突せずに生の文字を
//! 使える。
//!
//! Esc は一度にすべて閉じるのではなく、1段階ずつ (ドロップダウン → バー →
//! アプリ) 巻き戻す。アプリの他の入れ子モーダルの挙動に合わせてあり、
//! 誤って Down を押しても分かりやすく戻れる。

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;
use crate::menu::model::{MENUS, MenuItem};
use crate::menu::state::{
    MenuFocus, find_by_initial, first_selectable, last_selectable, step_menu, step_selection,
};

/// index にあるメニューの項目。index が古くなっていれば空スライスを返す。
fn items_of(index: usize) -> &'static [MenuItem] {
    MENUS.get(index).map(|m| m.items).unwrap_or(&[])
}

/// タイトルが ch から始まる (大文字小文字を無視) 最初のメニューのインデックス。
fn menu_by_initial(ch: char) -> Option<usize> {
    let target = ch.to_ascii_lowercase();
    MENUS.iter().position(|m| {
        m.title
            .chars()
            .next()
            .is_some_and(|c| c.to_ascii_lowercase() == target)
    })
}

/// メニュー menu_idx の項目 item_idx にコマンドがあり、かつ現在利用可能なら
/// それを実行する。
fn activate(app: &mut App, menu_idx: usize, item_idx: usize) {
    let Some(id) = items_of(menu_idx).get(item_idx).and_then(MenuItem::command) else {
        return;
    };
    // グレーアウトした行は存在が見えるように選択可能なままにしてあるが、
    // 実行しても何も起きない — 無効化された GUI 項目をクリックするのと同じ。
    if !app.command_enabled(id) {
        return;
    }
    // 先にメニューを閉じる: OpenRepo のようなコマンドはオーバーレイを積むので、
    // 後から閉じるとそのコマンドが設定した状態ごと消えてしまう。
    app.menu.close();
    app.execute_palette_command(id);
}

/// ハイライトされた行をドロップダウンの可視ウィンドウ内に保つ。
fn rescroll(app: &mut App) {
    let visible = crate::ui::menu_bar::visible_rows(app, app.layout.cache.frame_area.height);
    app.menu.scroll_selection_into_view(visible);
}

/// [MenuFocus] がアクティブな間のキーを処理する。呼び出し側は、メニューが
/// 入力を握っていることをすでに確認済み。
pub(super) fn handle_menu_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    match app.menu.focus {
        MenuFocus::Closed => {}

        // バーにフォーカス、何も開いていない
        MenuFocus::Bar { index } => match key.code {
            KeyCode::Esc => app.menu.close(),
            KeyCode::Left => app.menu.focus_bar(step_menu(MENUS.len(), index, -1)),
            KeyCode::Right => app.menu.focus_bar(step_menu(MENUS.len(), index, 1)),
            KeyCode::Home => app.menu.focus_bar(0),
            KeyCode::End => app.menu.focus_bar(MENUS.len().saturating_sub(1)),
            KeyCode::Down | KeyCode::Enter | KeyCode::Char(' ') => {
                app.menu.open(index, items_of(index));
            }
            // 文字を押すとそのメニューへ直接ジャンプして開く — 行き先を
            // 知っている人にとって最速の経路。
            KeyCode::Char(c) => {
                if let Some(idx) = menu_by_initial(c) {
                    app.menu.open(idx, items_of(idx));
                }
            }
            _ => {}
        },

        // ドロップダウンが開いている
        MenuFocus::Open {
            index,
            selected,
            scroll,
        } => match key.code {
            KeyCode::Esc => app.menu.focus_bar(index),
            KeyCode::Left => {
                let idx = step_menu(MENUS.len(), index, -1);
                app.menu.open(idx, items_of(idx));
            }
            KeyCode::Right => {
                let idx = step_menu(MENUS.len(), index, 1);
                app.menu.open(idx, items_of(idx));
            }
            KeyCode::Up => {
                app.menu.focus = MenuFocus::Open {
                    index,
                    selected: step_selection(items_of(index), selected, -1),
                    scroll,
                };
                rescroll(app);
            }
            KeyCode::Down => {
                app.menu.focus = MenuFocus::Open {
                    index,
                    selected: step_selection(items_of(index), selected, 1),
                    scroll,
                };
                rescroll(app);
            }
            KeyCode::Home => {
                app.menu.focus = MenuFocus::Open {
                    index,
                    selected: first_selectable(items_of(index)),
                    scroll,
                };
                rescroll(app);
            }
            KeyCode::End => {
                app.menu.focus = MenuFocus::Open {
                    index,
                    selected: last_selectable(items_of(index)),
                    scroll,
                };
                rescroll(app);
            }
            KeyCode::Enter => activate(app, index, selected),
            KeyCode::Char(c) => {
                if let Some(idx) = find_by_initial(items_of(index), selected, c) {
                    app.menu.focus = MenuFocus::Open {
                        index,
                        selected: idx,
                        scroll,
                    };
                    rescroll(app);
                }
            }
            _ => {}
        },
    }
    None
}

/// マウス側からの実行。クリックハンドラと共有し、クリックされた行と Enter で
/// 選択された行がまったく同じ経路を通るようにする。
pub(in crate::event) fn activate_item(app: &mut App, menu_idx: usize, item_idx: usize) {
    activate(app, menu_idx, item_idx);
}
