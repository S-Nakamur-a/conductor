// 成果物を「行から引ける」形にしたもの。読む側が最初に触る面。
//
// 成果物 (review.rs) は項目の側から範囲を持っているが、diff を描く側が欲しいのは
// 逆向きの「この行の持ち主は」で、毎回 sections を線形に走るのは描画のたびに
// 効く。ここで一度だけ索引を作る。

use crate::review::{Coverage, Impact, Importance, Overview, Position, Review, Section, Side};
use std::collections::HashMap;

impl Importance {
    /// 帯や見出しに使う色の目安。素の RGB なのは、描画ライブラリの型
    /// (ratatui の Color など) をここへ持ち込まないため。
    ///
    /// diff の緑/赤とぶつからないよう暖色と無彩色で組んである。
    pub const fn recommended_rgb(self) -> (u8, u8, u8) {
        match self {
            Importance::Core => (255, 130, 80),
            Importance::Ripple => (240, 174, 70),
            Importance::Follow => (133, 147, 166),
            Importance::Minor => (93, 103, 117),
        }
    }
}

/// 成果物が読めなかった理由。
///
/// JSON として壊れているのと、スキーマ版が対応していないのは別の異常。畳むと、
/// 版が上がったことに誰も気付かないまま古い意味で読んでしまう。
#[derive(Debug)]
pub enum LoadError {
    /// JSON として壊れている。
    Json(serde_json::Error),
    /// スキーマ版が今の実装と合わない。
    UnsupportedSchema(u32),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Json(e) => write!(f, "成果物の JSON が壊れている: {e}"),
            LoadError::UnsupportedSchema(v) => write!(
                f,
                "対応していないスキーマ版 {v}（対応しているのは {}）",
                crate::review::SCHEMA_VERSION
            ),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Json(e) => Some(e),
            LoadError::UnsupportedSchema(_) => None,
        }
    }
}

/// 読み込み済みの成果物と、行から引くための索引。
#[derive(Debug)]
pub struct Annotations {
    review: Review,
    /// (パス, 後像行番号) -> sections の添字。
    new_side: HashMap<(String, u32), usize>,
    /// (パス, 前像行番号) -> sections の添字。削除行はこちらでしか引けない。
    old_side: HashMap<(String, u32), usize>,
    /// 行を持たない変更（バイナリ、モードのみ、純粋な rename）。
    file_level: HashMap<String, usize>,
}

impl Annotations {
    /// 成果物の JSON から作る。
    ///
    /// JSON が壊れている・スキーマ版が違うはどちらも異常で、呼ぶ側に区別
    /// できる形で伝える。ファイルが無いことはここでは扱わない（テキストを
    /// 読めた時点で呼ぶ側の関心は終わっている）。
    pub fn from_json(text: &str) -> Result<Self, LoadError> {
        let review = Review::from_json(text).map_err(LoadError::Json)?;
        if review.schema != crate::review::SCHEMA_VERSION {
            return Err(LoadError::UnsupportedSchema(review.schema));
        }
        let mut a = Annotations {
            review,
            new_side: HashMap::new(),
            old_side: HashMap::new(),
            file_level: HashMap::new(),
        };
        a.build_index();
        Ok(a)
    }

    fn build_index(&mut self) {
        for (idx, ctx) in self.review.sections.iter().enumerate() {
            for r in &ctx.ranges {
                for p in r.positions() {
                    // 取り合いは説明もれ検査が別に報告している。ここでは先着を採る。
                    // 後から来た方で上書きすると、同じ成果物でも項目の並び順で
                    // 色が変わることになる。
                    match (p.side, p.line) {
                        (Side::File, _) => {
                            self.file_level.entry(p.path).or_insert(idx);
                        }
                        (Side::New, Some(n)) => {
                            self.new_side.entry((p.path, n)).or_insert(idx);
                        }
                        (Side::Old, Some(n)) => {
                            self.old_side.entry((p.path, n)).or_insert(idx);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// この位置を持っている項目の番号（sections() の添字）。
    ///
    /// 重要度ではなく番号が要るのは、同じ項目に属する行をひとまとまりに
    /// 集めたいとき（order.rs）。同じ重要度でも別の項目なら別の束になる。
    pub(crate) fn owner(&self, pos: &Position) -> Option<usize> {
        match (pos.side, pos.line) {
            (Side::File, _) => self.file_level.get(&pos.path).copied(),
            (Side::New, Some(n)) => self.new_side.get(&(pos.path.clone(), n)).copied(),
            (Side::Old, Some(n)) => self.old_side.get(&(pos.path.clone(), n)).copied(),
            _ => None,
        }
    }

    /// 全ての項目。重要度順に並んでいる。
    pub fn sections(&self) -> &[Section] {
        &self.review.sections
    }

    pub fn overview(&self) -> &Overview {
        &self.review.overview
    }

    pub fn impacts(&self) -> &[Impact] {
        &self.review.impacts
    }

    pub fn coverage(&self) -> &Coverage {
        &self.review.coverage
    }

    pub fn base(&self) -> &str {
        &self.review.base
    }

    pub fn head(&self) -> &str {
        &self.review.head
    }

    /// 前回の成果物からの進み。初回は None。
    pub fn since_previous(&self) -> Option<&crate::review::SincePrevious> {
        self.review.since_previous.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 追加行 new:10-12 は中核、削除行 old:9 とバイナリ 1 件は追従。
    const SECTIONS: &str = r#"[
        {"title":"中核","body":"b","importance":"core","reason":"r",
         "ranges":[{"path":"src/a.rs","side":"new","start":10,"end":12}]},
        {"title":"追従","body":"b","importance":"follow","reason":"渡す値が同一",
         "ranges":[{"path":"src/a.rs","side":"old","start":9,"end":9},
                   {"path":"logo.png","side":"file"}]}
      ]"#;

    const IMPACTS: &str = r#"[{"feature":"f","change":"ch","verify":"v","confidence":"fact"}]"#;

    fn sample_json() -> String {
        revidere_fixtures::review_with_impacts(SECTIONS, IMPACTS)
    }

    fn sample() -> Annotations {
        Annotations::from_json(&sample_json()).expect("読めること")
    }

    #[test]
    fn 追加行は後像側から解決する() {
        let a = sample();
        assert_eq!(a.owner(&Position::new("src/a.rs", Side::New, 10)), Some(0));
        assert_eq!(a.owner(&Position::new("src/a.rs", Side::New, 12)), Some(0));
        assert_eq!(a.owner(&Position::new("src/a.rs", Side::New, 13)), None);
    }

    #[test]
    fn 削除行は前像側からしか辿れない() {
        // 削除行を後像に寄せていないことの担保。ここが両方から引けたら、
        // 位置モデルが縮んでいる。
        let a = sample();
        assert_eq!(a.owner(&Position::new("src/a.rs", Side::Old, 9)), Some(1));
        assert_eq!(a.owner(&Position::new("src/a.rs", Side::New, 9)), None);
    }

    #[test]
    fn ファイル単位の変更は行番号なしで解決する() {
        let a = sample();
        assert_eq!(a.owner(&Position::file("logo.png")), Some(1));
        assert_eq!(a.owner(&Position::file("src/a.rs")), None);
    }

    #[test]
    fn スキーマ版違いは専用のエラーで拒む() {
        // 版を上げるたびに書き換えずに済むよう、今の版から差し替える。
        let current = format!("\"schema\": {}", crate::review::SCHEMA_VERSION);
        let other = sample_json().replace(&current, "\"schema\": 99");
        match Annotations::from_json(&other) {
            Err(LoadError::UnsupportedSchema(99)) => {}
            other => panic!("スキーマ版違いとして拒否されるはず: {other:?}"),
        }
    }

    #[test]
    fn 壊れたjsonはスキーマ違いとは別のエラー() {
        match Annotations::from_json("{ not json") {
            Err(LoadError::Json(_)) => {}
            other => panic!("JSON 破損として拒否されるはず: {other:?}"),
        }
    }

    #[test]
    fn 取り合いになった行は先の項目が持つ() {
        let contested = sample_json().replace(
            r#"{"path":"src/a.rs","side":"old","start":9,"end":9}"#,
            r#"{"path":"src/a.rs","side":"new","start":10,"end":10}"#,
        );
        let a = Annotations::from_json(&contested).unwrap();
        assert_eq!(a.owner(&Position::new("src/a.rs", Side::New, 10)), Some(0));
    }

    #[test]
    fn 重要度ごとに色が重ならない() {
        let mut seen = Vec::new();
        for i in Importance::ORDER {
            let rgb = i.recommended_rgb();
            assert!(!seen.contains(&rgb), "{i:?} の色が他と重なっている");
            seen.push(rgb);
        }
    }
}
