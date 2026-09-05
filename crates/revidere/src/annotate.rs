// 成果物を「行から引ける」形にしたもの。読む側が最初に触る面。
//
// 成果物は項目の側から範囲を持っているが、diff を描く側が欲しいのは逆向きの
// 「この行の持ち主は」。毎回 sections を線形に走ると描画のたびに効くので、
// ここで一度だけ索引を作る。

use crate::review::{Coverage, Impact, Importance, Overview, Position, Review, Section, Side};
use std::collections::HashMap;

impl Importance {
    /// 帯や見出しに使う色の目安。素の RGB なのは、描画ライブラリの型を
    /// ここへ持ち込まないため。diff の緑/赤とぶつからない暖色と無彩色。
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
/// 2 つを畳まない。畳むと、版が上がったことに誰も気付かないまま古い意味で
/// 読んでしまう。
#[derive(Debug)]
pub enum LoadError {
    Json(serde_json::Error),
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
    new_side: HashMap<(String, u32), usize>,
    /// 削除行はこちらでしか引けない。後像側には寄せない。
    old_side: HashMap<(String, u32), usize>,
    file_level: HashMap<String, usize>,
}

impl Annotations {
    /// 成果物の JSON から作る。
    ///
    /// ファイルが無いことはここでは扱わない。テキストを読めた時点で、
    /// それは呼ぶ側の関心の外にある。
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
                    // 取り合いは説明もれ検査が別に報告している。ここで後着を
                    // 採ると、同じ成果物でも項目の並び順で色が変わる。
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
    /// 重要度ではなく番号なのは、同じ重要度でも別の項目なら別の束になるから。
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
    use revidere_fixtures::Section as S;

    /// 追加行 new:10-12 は中核、削除行 old:9 とバイナリ 1 件は追従。
    fn sample_sections() -> Vec<S> {
        vec![
            S::new("中核", "core").lines("src/a.rs", "new", 10, Some(12)),
            S::new("追従", "follow")
                .line("src/a.rs", "old", 9)
                .whole_file("logo.png"),
        ]
    }

    const IMPACTS: &str = r#"[{"feature":"f","change":"ch","verify":"v","confidence":"fact"}]"#;

    fn sample_json() -> String {
        revidere_fixtures::review_with_impacts(
            &revidere_fixtures::sections(&sample_sections()),
            IMPACTS,
        )
    }

    fn sample() -> Annotations {
        Annotations::from_json(&sample_json()).expect("読めること")
    }

    /// 削除行が後像側からも引けたら、位置モデルが後像へ縮んでいる。
    #[test]
    fn 位置は側ごとに別々の索引から解決する() {
        let a = sample();
        for (name, pos, want) in [
            (
                "追加行の先頭",
                Position::new("src/a.rs", Side::New, 10),
                Some(0),
            ),
            (
                "追加行の末尾",
                Position::new("src/a.rs", Side::New, 12),
                Some(0),
            ),
            ("範囲の外", Position::new("src/a.rs", Side::New, 13), None),
            ("削除行", Position::new("src/a.rs", Side::Old, 9), Some(1)),
            (
                "削除行と同じ番号の後像",
                Position::new("src/a.rs", Side::New, 9),
                None,
            ),
            ("ファイル単位", Position::file("logo.png"), Some(1)),
            (
                "行を持つファイルのファイル単位",
                Position::file("src/a.rs"),
                None,
            ),
        ] {
            assert_eq!(a.owner(&pos), want, "{name}");
        }
    }

    #[test]
    fn 取り合いになった行は先の項目が持つ() {
        let contested = vec![
            sample_sections()[0].clone(),
            S::new("追従", "follow").line("src/a.rs", "new", 10),
        ];
        let json = revidere_fixtures::review(&revidere_fixtures::sections(&contested));
        let a = Annotations::from_json(&json).unwrap();
        assert_eq!(a.owner(&Position::new("src/a.rs", Side::New, 10)), Some(0));
    }

    #[test]
    fn 読めない成果物は理由ごとに別のエラーになる() {
        // 版を上げるたびに書き換えずに済むよう、今の版から差し替える。
        let current = format!("\"schema\": {}", crate::review::SCHEMA_VERSION);
        let bumped = sample_json().replace(&current, "\"schema\": 99");
        match Annotations::from_json(&bumped) {
            Err(LoadError::UnsupportedSchema(99)) => {}
            other => panic!("スキーマ版違いとして拒否されるはず: {other:?}"),
        }
        match Annotations::from_json("{ not json") {
            Err(LoadError::Json(_)) => {}
            other => panic!("JSON 破損として拒否されるはず: {other:?}"),
        }
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
