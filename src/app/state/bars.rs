//! 画面上端の worktree モニタストリップ (worktree bar) の状態。

use crate::ui::worktree_bar::{WtbarAction, WtbarHit};

/// worktree ストリップの横スクロール位置とマウス当たり判定。
///
/// hits は描画のたびに記録され、マウスハンドラが読む — 描画とイベント処理の
/// あいだでレイアウトを受け渡す唯一の経路なので、両者を同じ構造体に置いている。
#[derive(Default)]
pub struct WtbarState {
    /// 直近の描画が記録したクリック領域 (worktree 選択 / 削除 / 追加)。
    pub hits: Vec<WtbarHit>,
    /// 先頭に表示している worktree チップの添字 (横スクロール位置)。
    /// ホイールと矢印でページングされ、描画のたびに再クランプされる。
    pub scroll: usize,
    /// 次回の描画で、選択中の worktree のチップが見えるところまで
    /// scroll をパンさせる。選択が変わったときに立てるので、ジャンプ
    /// すれば必ずそのチップが露出しつつ、通常のスクロールは邪魔しない。
    pub reveal_selected: bool,
    /// いまマウスが乗っているアクション (直近の Moved を hits に
    /// 突き合わせた結果)。チップと [x] のホバー背景を駆動する。
    pub hover: Option<WtbarAction>,
}
