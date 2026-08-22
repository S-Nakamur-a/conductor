//! ビューアパネル — diff ハイライトとレビューコメント付きのファイル内容表示。
//!
//! 中央カラムに選択中ファイルの内容を表示する。diff_state 上で変更のあった行は
//! インラインでハイライトされる。レビューコメントはインラインバッジとして表示される。
//!
//! 描画の責務ごとに分割されている: [file_view] はプレーン/注釈付きファイル内容
//! （パネルのデフォルトモード）、[diff_view] は unified-diff モード、[summary_view]
//! はブランチの変更概要疑似ファイル、[markdown_view] は markdown ファイルを文章として
//! 描画する（Raw/Rendered ヘッダー切り替えも含む）、[media_view] は画像/動画、
//! [comment_thread] はインラインのレビューコメントスレッドと新規コメント作成ボックス、
//! [syntax] は syntax/diff の注釈ヘルパー、[span_utils] は汎用の Span 操作、
//! [search_box] はパネル内検索入力、[tab_row] は開いているファイルのタブ行。

mod code_line;
mod comment_thread;
mod diff_line;
mod diff_view;
mod file_view;
mod markdown_view;
mod media_view;
mod search_box;
mod span_utils;
mod summary_view;
mod syntax;
mod tab_row;

pub use file_view::render;
pub(crate) use markdown_view::toggle_segments;
// revidere の 2 列ビューも syntect のトークンを描くので、タブの展開は同じ
// 実装を使う。1 行の中で列を引き継ぐのが要点なので、単純な置換では代われない。
pub(crate) use syntax::expand_tabs_at;

/// インラインスレッドのアクション行の共有定義。
///
/// レンダラー（[comment_thread::build_inline_thread_lines]）と event/mouse.rs の
/// マウスヒットテストは、各アクションの位置について一致していなければならない。
/// どちらもこれらの定数からレイアウトを導出するので、ラベルを変更してもクリック対象が
/// 気づかぬうちに壊れることはない。
pub(crate) mod thread_actions {
    pub const REPLY: &str = "\u{21a9} reply"; // ↩ reply
    pub const RESOLVE: &str = "\u{2713} resolve"; // ✓ resolve
    pub const UNRESOLVE: &str = "\u{21ba} unresolve"; // ↺ unresolve
    pub const DELETE: &str = "\u{2717} delete"; // ✗ delete
    pub const ASK_CLAUDE: &str = "\u{2728} ask claude"; // ✨ ask claude
    /// アクション間の間隔（列数）。
    pub const GAP: usize = 2;

    fn w(s: &str) -> usize {
        unicode_width::UnicodeWidthStr::width(s)
    }

    /// status（resolve/unresolve）スロットのパディング先の幅。現在の status に
    /// 関わらず delete アクションが常に同じ列から始まるようにする。
    pub fn status_slot_width() -> usize {
        w(RESOLVE).max(w(UNRESOLVE))
    }

    /// この列（アクション行コンテンツ開始位置からの相対位置）より左側のクリックは
    /// "reply" に当たる。
    pub fn reply_end() -> usize {
        w(REPLY) + GAP
    }

    /// reply_end()..resolve_end() の範囲のクリックは "resolve"/"unresolve" に、
    /// それ以降は "delete"（さらに右端なら "ask claude"）に当たる。
    pub fn resolve_end() -> usize {
        reply_end() + status_slot_width() + GAP
    }

    /// 右寄せの "ask claude" ボタンの表示幅。パネル右端に対するヒットテストに使う。
    pub fn ask_claude_width() -> usize {
        w(ASK_CLAUDE)
    }
}
