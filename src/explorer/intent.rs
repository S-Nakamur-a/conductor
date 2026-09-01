//! Explorer がユーザの要求を外へ伝える形。
//!
//! パネルは他パネルの状態を書かない。書きたくなるものはどれも Explorer の処理が
//! 必要とする状態ではなく「ユーザが何を要求したか」なので、値として返して
//! [crate::app::App] に解釈させる。分割前は 13 のフィールドパスへ直接書いており、
//! Explorer が Viewer とレビュー状態の内部を知っていた。
//!
//! 選択中の行を指すだけのものは行番号を運ばない。App はパネルの持ち主なので
//! カーソルを読める。運ぶのは、行から中身への解決をパネル側でしか行えないもの
//! (返信の行から親コメントを引く等) だけ。

use crate::app::OpenAs;

/// Explorer が App に頼むこと。
pub enum Intent {
    /// ツリーの根からの相対パス。絶対パスに戻せるのは根を持つツリーだけなので、
    /// 意図の側では相対のまま運ぶ。
    ///
    /// ツリーが出すのはファイルの中身そのもので、diff ではない。同じファイルを
    /// diff で見ている最中でも素の表示へ戻す。
    OpenFile {
        path: String,
        how: OpenAs,
    },
    /// 変更ファイル一覧で選択中の行を Viewer で開く。
    OpenSelectedChange {
        how: OpenAs,
    },
    /// ブランチの変更サマリを Viewer 全体で開く。
    OpenSummary,
    /// Viewer を該当コメントの位置へ寄せて選択させる。フォーカスは移さない。
    ///
    /// 返信の行からは親コメントへ辿る必要があり、その解決はコメント一覧の
    /// 行構造を知っている側にしかできないので、ここだけ添字を運ぶ。
    RevealComment {
        comment: usize,
        /// フォーカスも Viewer へ移すか。ダブルクリックのときだけ真。
        focus_viewer: bool,
    },
    OpenSelectedCommentDetail,
    BeginReplyToSelected,
    ToggleCommentExpansion,
    EditSelectedComment,
    DeleteSelectedComment,
    ToggleCommentResolved,
    /// 変更ファイル一覧のセクションやディレクトリの開閉。
    /// 開閉の状態を持っているのが `diff_state` なので Explorer からは書けない。
    Section {
        op: SectionOp,
    },
    /// レビュー済みの印を付け外しする。永続化を伴う。
    ToggleSelectedViewed,
    CloseModal,
    /// 変更のあったファイル全部について Claude に尋ねる。
    AskClaudeAboutChanges,
    OpenFilenameSearch,
}

pub enum SectionOp {
    Toggle,
    Expand,
    Collapse,
}
