//! [super::render] が返す値の型。
//!
//! 当たり判定は area から導ける純粋関数で済み、描画してみるまで決まらない
//! のは検索欄の端末カーソル位置だけ。実際に置くかどうかは全画面オーバーレイ
//! の有無次第で、それは Explorer の外の話なので呼び出し側に委ねる。

/// [super::render] 1回分の呼び出しの結果。
#[derive(Default)]
pub struct Geometry {
    /// 検索欄が出ていれば、そこに置くべき端末カーソルの位置。
    pub search_cursor: Option<(u16, u16)>,
}
