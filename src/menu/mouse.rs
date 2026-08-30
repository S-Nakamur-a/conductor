//! メニューバーとそのドロップダウンのマウス処理。
//!
//! この2つの入口は
//! [handle_mouse_event](crate::event::mouse::handle_mouse_event) 内で他のバー用
//! ハンドラより先に実行される。この順序には意味がある。handle_title_bar_click は
//! main_area より上の全ての行を無条件に自分のものとして true を返すため、これより
//! 後にメニューバーのクリック処理を置くと絶対に呼ばれない。
//!
//! クリックが何を意味するかの判断は [classify_menu_click] が担う。記録済みの
//! ヒット領域に対する純粋関数であり、
//! [classify_margin_click](crate::viewer::mouse::classify_margin_click)
//! と同じ形。理由も同じで、興味深いルール（トグル・閉じる・無反応行）を App や
//! ターミナルを立ち上げずにテストできるようにするため。

use crate::app::App;
use crate::menu::MenuFocus;
use crate::menu::model::MENUS;
use crate::menu::state::MenuState;

/// 指定した位置への左クリックがメニューに対して何をすべきかを表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuClick {
    /// menu の item を実行する。
    Activate { menu: usize, item: usize },
    /// menu のドロップダウンを開く。
    Open(usize),
    /// メニューから完全に抜ける。
    Close,
    /// 消費はするが何も起きない — 無効化された行、セパレータ、ドロップダウン自体の枠。
    /// 惜しいクリックで即座に閉じてしまわず、メニューを開いたままにする。
    Inert,
    /// メニューへのクリックではない。残りのディスパッチャに渡す。
    Pass,
}

/// index にあるメニューの項目一覧。インデックスが古い場合は空スライス。
fn items_of(index: usize) -> &'static [crate::menu::MenuItem] {
    MENUS.get(index).map(|m| m.items).unwrap_or(&[])
}

/// (col, row) へのクリックが何を意味するかを決定する。bar_row はメニューバーの
/// 画面上の行で、バーが描画されていない場合は None。
pub(crate) fn classify_menu_click(
    state: &MenuState,
    bar_row: Option<u16>,
    col: u16,
    row: u16,
) -> MenuClick {
    // 開いているドロップダウンの内側。
    if state.in_dropdown(col, row) {
        return match (state.focus.open_index(), state.item_hit_at(row)) {
            (Some(menu), Some(hit)) if hit.enabled => MenuClick::Activate {
                menu,
                item: hit.item,
            },
            _ => MenuClick::Inert,
        };
    }

    // バーの行そのもの。
    if bar_row == Some(row) {
        return match state.bar_hit_at(col) {
            // 開いているメニューをクリックすると閉じる。既に開いているものを
            // 開き直すのではなく、同じ対象へのクリックがトグルとして働く。
            Some(idx) if state.focus.open_index() == Some(idx) => MenuClick::Close,
            Some(idx) => MenuClick::Open(idx),
            // バーの空白部分: 開くものは何もなく、開いているメニューがあれば閉じる。
            None => MenuClick::Close,
        };
    }

    // それ以外の場所: メニューが開いていれば閉じる。開いていなければこのイベントの
    // 対象外。閉じるためのクリックは意図的に飲み込む — メニューを閉じる操作が、
    // その下にあったものまで押してしまってはいけない。
    if state.focus.is_active() {
        MenuClick::Close
    } else {
        MenuClick::Pass
    }
}

/// メニューバーと開いているドロップダウンへの左クリックを処理する。クリックを
/// 消費した場合は true を返す。
pub(crate) fn handle_menu_click(app: &mut App, col: u16, row: u16) -> bool {
    let bar = app.layout.cache.menubar_area;
    let bar_row = (bar.height > 0).then_some(bar.y);

    match classify_menu_click(&app.menu, bar_row, col, row) {
        MenuClick::Activate { menu, item } => {
            super::input::activate_item(app, menu, item);
            true
        }
        MenuClick::Open(idx) => {
            app.menu.open(idx, items_of(idx));
            true
        }
        MenuClick::Close => {
            app.menu.close();
            true
        }
        MenuClick::Inert => true,
        MenuClick::Pass => false,
    }
}

/// メニューバーのホバーを追跡する。このマウス移動をメニューが専有する場合は true を
/// 返し、呼び出し側は他パネルのホバー処理をスキップする（ドロップダウンの下にあるものが
/// 光ってはいけない）。
pub(crate) fn handle_menu_hover(app: &mut App, col: u16, row: u16) -> bool {
    let bar = app.layout.cache.menubar_area;
    let on_bar = bar.height > 0 && row == bar.y;

    // カーソル下のタイトルをハイライトする。行から外れると None になるが、
    // これは同時に「マウスがバーから離れた」ことのクリアも兼ねている。
    app.menu.hover = if on_bar {
        app.menu.bar_hit_at(col)
    } else {
        None
    };

    match app.menu.focus {
        MenuFocus::Closed => false,

        // メニューが開いている状態でバー上をなぞると表示中のメニューが切り替わる —
        // これによりメニューバーは「クリックで開いて閉じて」を繰り返すのではなく、
        // ブラウズできるものになる。
        MenuFocus::Open { index, .. } => {
            if on_bar {
                if let Some(idx) = app.menu.bar_hit_at(col)
                    && idx != index
                {
                    app.menu.open(idx, items_of(idx));
                }
            } else if app.menu.in_dropdown(col, row) {
                // 行にホバーすると選択が移動する。これによりキーボードとポインタで
                // 「現在の項目」という概念を共有できる。
                if let Some(hit) = app.menu.item_hit_at(row)
                    && let MenuFocus::Open {
                        ref mut selected, ..
                    } = app.menu.focus
                {
                    *selected = hit.item;
                }
            }
            true
        }

        // F10でバーにフォーカスしているが何も開いていない状態。ホバーはハイライトのみ
        // 行い、開くには依然としてクリックまたはDown/Enterが必要。これにより画面上部で
        // マウスを動かしただけでユーザが望んでいないメニューが開くことはない。
        MenuFocus::Bar { .. } => on_bar,
    }
}
