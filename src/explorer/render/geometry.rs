//! [super::render] が返す値の型。
//!
//! Explorer の3つの一覧は行の高さが固定なので、画面上の当たり判定はほぼ
//! すべて area から直接導ける純粋関数（[super::comments::ask_claude_all_cols]
//! など）で済む。描画してみるまで決まらないのは検索欄の端末カーソル位置
//! だけ — 他パネルのオーバーレイが出ているかどうかは Explorer の外の話で、
//! 実際に置くかどうかの判断も含めて呼び出し側に委ねる。

/// [super::render] 1回分の呼び出しの結果。
#[derive(Default)]
pub struct Geometry {
    /// 検索欄が出ていれば、そこに置くべき端末カーソルの位置。
    pub search_cursor: Option<(u16, u16)>,
}
