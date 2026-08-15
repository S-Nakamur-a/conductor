//! revidere の状態: いま読み込まれている成果物と、解析の実行中のもの。

use crate::app::RevidereRuns;

/// 成果物と解析の状態。
#[derive(Default)]
pub struct RevidereState {
    /// 選択中の worktree の成果物。無い (まだ解析していない) が既定で、
    /// そのときは Viewer が素の diff を描く。
    /// [crate::app::App::refresh_reviews] が読み直す。
    pub current: Option<Box<crate::revidere::Review>>,
    /// 成果物が「在るのに読めなかった」ときの理由。読めた時と無い時は None。
    /// ステータスに出しても消えないよう、状態として持つ。
    pub load_error: Option<String>,
    /// 実行中の解析。ブランチごとに高々 1 本なので worktree 同士が
    /// 待ち合わせにならない。
    pub runs: RevidereRuns,
    /// いまどの区間のレビューを出しているか。読み込みも解析もこれに従う。
    ///
    /// 2 つを同時に持たない。スクロール位置も組み立て済みの列も区間ごとに
    /// 二重になるので、切り替えたら捨てて読み直す。
    pub scope: revidere::Scope,
    /// 2 列ビューの左列 (項目一覧) の選択位置。
    pub selected: usize,
    /// 右列 (diff) の垂直スクロールオフセット。
    pub diff_scroll: usize,
    /// 概要の 1 列表示に切り替わっているか。
    ///
    /// 開いた直後はこちら。概要は項目より先に読むものだが、読むのは最初の
    /// 一度で、そのあとずっと画面を取り続けるものではない (GitHub が PR の
    /// 説明と Files changed を分けているのと同じ切り分け)。
    pub show_overview: bool,
    /// 概要の垂直スクロールオフセット。diff と分けて持つのは、行き来しても
    /// それぞれの読みかけの位置が残るようにするため。
    pub overview_scroll: usize,
    /// 最後に読んだ成果物の (パス, 更新時刻)。ここが変わっていなければ
    /// 読み直しを丸ごと飛ばす — [crate::revidere::load] は git diff を取り直す
    /// ので、コメントが 1 件書かれるたびに走らせるには重い。
    pub loaded_from: Option<(std::path::PathBuf, std::time::SystemTime)>,
    /// 項目ごとの、右列での先頭行。本文の折り返しが幅に依存するので描画中に
    /// 書き込まれ、n/N のジャンプがこれを読む (diff ペインの screen_entry_map
    /// と同じ作り)。描画前は空。
    pub section_rows: Vec<usize>,
    /// 組み立て済みの右列。syntect と折り返しは 1 フレームに収まる仕事では
    /// ないので、幅・テーマ・成果物が変わったときだけ作り直す。
    pub diff_cache: Option<crate::ui::revidere_view::DiffRender>,
    /// 成果物の版。[Self::replace] のたびに進み、右列のキャッシュキーに入る。
    /// 中身を比較する代わりの安い指紋。
    pub epoch: u64,
    /// 左列のいま画面に出ている行 → 項目の番号。クリックした行がどの項目かを
    /// 引くのに使う。折り返しで 1 項目が複数行になるので、単純な割り算では
    /// 引けない。
    pub list_rows: Vec<usize>,
    /// 左列と右列の画面上の位置。マウスのヒットテストに使う。このビューは
    /// アコーディオンのカラムと重ならないので、レイアウトキャッシュ側の
    /// ジオメトリでは当たらない。
    pub list_area: ratatui::layout::Rect,
    pub diff_area: ratatui::layout::Rect,
    /// Changed files パネル右上の状態チップにマウスが乗っているか。
    /// 当たり判定は描画側 ([crate::ui::explorer_panel::revidere_badge_cols]) が
    /// 持っていて、ここに残るのは光らせるかどうかだけ。
    pub badge_hover: bool,
}

impl RevidereState {
    /// 2 列ビューに出せる成果物があるか。
    pub fn has_review(&self) -> bool {
        self.current.is_some()
    }

    /// 成果物を差し替え、そこに紐づく選択とスクロールを畳む。
    ///
    /// 選択だけ残すと、項目の数が減ったときに存在しない項目を指したままになる。
    pub fn replace(&mut self, review: Option<Box<crate::revidere::Review>>) {
        self.current = review;
        self.selected = 0;
        self.diff_scroll = 0;
        // 別の成果物は別の概要。開いた直後と同じく概要から読ませる。
        self.show_overview = true;
        self.overview_scroll = 0;
        self.section_rows.clear();
        self.list_rows.clear();
        self.loaded_from = None;
        // 右列のキャッシュは中身の指紋を持たないので、ここで版を進めて
        // 無効化する。捨て忘れると別の worktree の diff が残る。
        self.diff_cache = None;
        self.epoch = self.epoch.wrapping_add(1);
    }
}
