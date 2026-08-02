//! メニューバーのインタラクション状態と、キーボード/マウス両ハンドラが共有する
//! 純粋なナビゲーションヘルパー。
//!
//! ナビゲーションヘルパーが App のメソッドではなく &[MenuItem] に対する
//! フリー関数になっているのは、区切りのスキップやラップアラウンドのルールを
//! 端末や App を立ち上げずに単体テストできるようにするためである。

use ratatui::layout::Rect;

use super::model::MenuItem;

/// メニューバーのインタラクションが今どの状態にあるか。
///
/// 2状態ではなく3状態あるのは、F10 がメニューを確定させずにバーへ
/// フォーカスするだけの動作をするからである。これは GTK/Windows の慣習と同じで、
/// その後は矢印キーでタイトルを閲覧し、Down/Enter でドロップダウンを開く。
/// Bar を Open に統合してしまうと、F10 がユーザの求めていないドロップダウン
/// を強制的に開くことになる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuFocus {
    /// メニューバーは描画されるが不活性で、キーは通常どおりアプリに渡る。
    #[default]
    Closed,
    /// バーがキーボードフォーカスを持ち index がハイライトされているが、
    /// ドロップダウンはまだ開いていない。
    Bar { index: usize },
    /// index のドロップダウンが開いており selected がハイライトされている。
    Open {
        index: usize,
        selected: usize,
        scroll: usize,
    },
}

impl MenuFocus {
    /// メニューが入力を消費している状態かどうか。true のときはイベント
    /// ディスパッチャがすべてのキーをメニューハンドラに渡し、パネルには届かない。
    pub fn is_active(self) -> bool {
        !matches!(self, MenuFocus::Closed)
    }

    /// いずれかのアクティブ状態でハイライトされているトップレベルメニュー。
    pub fn active_index(self) -> Option<usize> {
        match self {
            MenuFocus::Closed => None,
            MenuFocus::Bar { index } => Some(index),
            MenuFocus::Open { index, .. } => Some(index),
        }
    }

    /// ドロップダウンが開いているメニューがあればそれ。
    pub fn open_index(self) -> Option<usize> {
        match self {
            MenuFocus::Open { index, .. } => Some(index),
            _ => None,
        }
    }
}

/// メニューバー行上のクリック可能なトップレベルタイトル。x0 は含み、x1 は
/// 含まない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarHit {
    pub x0: u16,
    pub x1: u16,
    /// [MENUS](super::model::MENUS) へのインデックス。
    pub menu: usize,
}

/// 開いたドロップダウン内のクリック可能な行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemHit {
    /// 画面上の絶対行。
    pub y: u16,
    /// 開いているメニューの items へのインデックス。
    pub item: usize,
    /// この行のコマンドが現在実行不可の場合は false。グレーアウトして描画され、
    /// クリックしても何も起きない。
    pub enabled: bool,
}

/// App が保持するメニューバー状態。
#[derive(Default)]
pub struct MenuState {
    pub focus: MenuFocus,
    /// 直近のバー描画で記録されたトップレベルタイトルのヒット領域。
    pub bar_hits: Vec<BarHit>,
    /// 直近のドロップダウン描画で記録された行のヒット領域。ドロップダウンが
    /// 開いていない間は空。
    pub item_hits: Vec<ItemHit>,
    /// 開いているドロップダウンの矩形(枠を含む)。範囲外クリックの判定に使う。
    /// 閉じている間はサイズ0。
    pub dropdown_area: Rect,
    /// マウス下にあるトップレベルタイトル。ホバーハイライト用。
    pub hover: Option<usize>,
}

impl MenuState {
    /// 記録済みのドロップダウン領域をすべて破棄する。古い矩形がクリックを
    /// 吸い込み続けないよう、ドロップダウンが閉じるたびに呼ぶ。
    pub fn clear_dropdown_regions(&mut self) {
        self.item_hits.clear();
        self.dropdown_area = Rect::default();
    }

    /// 何も開かずにバーへキーボードフォーカスを与える。
    pub fn focus_bar(&mut self, index: usize) {
        self.focus = MenuFocus::Bar { index };
        self.clear_dropdown_regions();
    }

    /// index のドロップダウンを、最初の選択可能行をハイライトした状態で開く。
    pub fn open(&mut self, index: usize, items: &[MenuItem]) {
        self.focus = MenuFocus::Open {
            index,
            selected: first_selectable(items),
            scroll: 0,
        };
        self.clear_dropdown_regions();
    }

    /// メニューを完全に離れ、入力をアプリに返す。
    ///
    /// コマンドを実行する前に必ずこれを呼ぶこと。複数のコマンドは独自の
    /// オーバーレイを開くため、実行後に閉じるとそのオーバーレイの状態まで
    /// 一緒に壊してしまう。
    pub fn close(&mut self) {
        self.focus = MenuFocus::Closed;
        self.clear_dropdown_regions();
    }

    /// selected が visible 行のウィンドウ内に収まるよう scroll を調整する。
    pub fn scroll_selection_into_view(&mut self, visible: usize) {
        let MenuFocus::Open {
            selected, scroll, ..
        } = &mut self.focus
        else {
            return;
        };
        if visible == 0 {
            return;
        }
        if *selected < *scroll {
            *scroll = *selected;
        } else if *selected >= *scroll + visible {
            *scroll = *selected + 1 - visible;
        }
    }

    /// バー行上で col の位置にあるトップレベルタイトル(あれば)。
    pub fn bar_hit_at(&self, col: u16) -> Option<usize> {
        self.bar_hits
            .iter()
            .find(|h| col >= h.x0 && col < h.x1)
            .map(|h| h.menu)
    }

    /// 画面上の絶対行 row にあるドロップダウン行(あれば)と、それが
    /// 実行可能かどうか。
    pub fn item_hit_at(&self, row: u16) -> Option<ItemHit> {
        self.item_hits.iter().find(|h| h.y == row).copied()
    }

    /// (col, row) が開いているドロップダウンの矩形内に収まるかどうか。
    pub fn in_dropdown(&self, col: u16, row: u16) -> bool {
        let a = self.dropdown_area;
        a.width > 0
            && a.height > 0
            && col >= a.x
            && col < a.x + a.width
            && row >= a.y
            && row < a.y + a.height
    }
}

// 純粋なナビゲーションヘルパー

/// items 内で最初に選択可能な行。なければ 0。
///
/// すべての行が区切りであるメニューは実行時の条件ではなくテーブル記述側の
/// ミスなので、呼び出し側全員がアンラップしなければならない Option を返す
/// のではなく、ここで 0 に落としておく。
pub fn first_selectable(items: &[MenuItem]) -> usize {
    items.iter().position(MenuItem::is_selectable).unwrap_or(0)
}

/// items 内で最後に選択可能な行。なければ 0。
pub fn last_selectable(items: &[MenuItem]) -> usize {
    items
        .iter()
        .rposition(MenuItem::is_selectable)
        .unwrap_or(0)
}

/// from から dir 方向(+1 で下、-1 で上)へ1行選択を進める。区切りは
/// スキップし、両端ではラップアラウンドする。
///
/// 無効な行もあえて選択可能なままにしてある。グレーアウトは「今は使えない」を
/// 意味するだけであり、スキップしてしまうと行の存在自体を隠すことになって
/// しまう。それは無効状態の目的とは逆である。
pub fn step_selection(items: &[MenuItem], from: usize, dir: i32) -> usize {
    let n = items.len();
    if n == 0 {
        return 0;
    }
    let mut idx = from.min(n - 1);
    // 最大でも n ステップ: どの行にも到達できる回数であり、選択可能な行が
    // 1つもないメニューでは諦めて from を返す。
    for _ in 0..n {
        idx = if dir >= 0 {
            (idx + 1) % n
        } else {
            (idx + n - 1) % n
        };
        if items[idx].is_selectable() {
            return idx;
        }
    }
    from
}

/// ラベルが ch で始まる(大文字小文字を区別しない)次の行を、from から
/// 前方かつラップして探す。メニュー内の "New …" 系エントリを n で
/// 渡り歩けるようにするタイプアヘッド機能。
pub fn find_by_initial(items: &[MenuItem], from: usize, ch: char) -> Option<usize> {
    let n = items.len();
    if n == 0 {
        return None;
    }
    let target = ch.to_ascii_lowercase();
    (1..=n)
        .map(|off| (from + off) % n)
        .find(|&idx| match &items[idx] {
            MenuItem::Command { label, .. } => label
                .chars()
                .next()
                .is_some_and(|c| c.to_ascii_lowercase() == target),
            MenuItem::Separator => false,
        })
}

/// ハイライトされているトップレベルメニューを1つ進める。ラップする。
pub fn step_menu(menu_count: usize, from: usize, dir: i32) -> usize {
    if menu_count == 0 {
        return 0;
    }
    if dir >= 0 {
        (from + 1) % menu_count
    } else {
        (from + menu_count - 1) % menu_count
    }
}
