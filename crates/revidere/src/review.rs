// 成果物の型。本当の公開 API は JSON の綴りの方で、Rust の型はその写し。
// ホストは JSON を読むだけでよく、Rust である必要はない。

use serde::{Deserialize, Serialize};

/// 成果物のスキーマ版。破壊的変更で上げる。
pub const SCHEMA_VERSION: u32 = 2;

/// revidere が書き出すものの置き場。成果物も貯めた応答もこの下。
///
/// ホスト (conductor) と同じディレクトリを使う。分けても、無視する設定と
/// 掃除の手間が倍になるだけ。
pub const DIR: &str = ".conductor";

/// どの区間を見たレビューか。
///
/// [Scope::SincePrevious] が要るのは、指摘そのものが conductor の外
/// (Claude Code の会話や GitHub) にあって取り込めないから。直しを確かめる
/// 手立ては「どこがどう変わったか」を読むことしかない。
///
/// 1 枚の成果物に両方を持たせない。網羅性は区間ごとの性質で、混ぜると
/// 説明もれ検査が意味を失う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// ベースブランチとの共通祖先から作業ツリーまで。
    #[default]
    Base,
    /// 前回のレビューが見ていたコミットから作業ツリーまで。
    SincePrevious,
}

/// 成果物の置き場。`<repo>/.conductor/review.json` とその前回差分版。
///
/// 書く側と読む側が別々にパスを組み立てると、片方だけ変えたときに「書いたのに
/// 読まれない」が黙って起きる。
pub fn artifact_path(repo_root: &std::path::Path, scope: Scope) -> std::path::PathBuf {
    let name = match scope {
        Scope::Base => "review.json",
        // 起点をファイル名に入れない。1 ラウンド 1 枚で上書きする方が、溜まった
        // ものを掃除せずに済む。どの起点で作ったかは中の base で分かる。
        Scope::SincePrevious => "review-since-previous.json",
    };
    repo_root.join(DIR).join(name)
}

/// 変更箇所がどちら側のものか。
///
/// 削除行を後像の行番号に寄せる妥協をしないために Old を一級で持つ。
/// 後像行番号しか持てないホストがあっても、こちらのモデルは縮めない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// 後像（追加行）。
    New,
    /// 前像（削除行）。
    Old,
    /// 行を持たないファイル単位の変更（バイナリ、モードのみ、純粋な rename）。
    File,
}

/// 変更箇所 1 つ。分割できない最小の単位で、分類はこの粒度で行う。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub path: String,
    pub side: Side,
    /// 1 始まりの行番号。side が File のときだけ None。
    pub line: Option<u32>,
}

impl Position {
    pub fn new(path: impl Into<String>, side: Side, line: u32) -> Self {
        Self {
            path: path.into(),
            side,
            line: Some(line),
        }
    }

    pub fn file(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            side: Side::File,
            line: None,
        }
    }
}

impl std::fmt::Display for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.side, self.line) {
            (Side::File, _) => write!(f, "{} (file)", self.path),
            (Side::New, Some(n)) => write!(f, "{}:new:{}", self.path, n),
            (Side::Old, Some(n)) => write!(f, "{}:old:{}", self.path, n),
            (s, None) => write!(f, "{}:{:?}:?", self.path, s),
        }
    }
}

/// 位置の連続した範囲。モデルには 1 行ずつではなくこの形で答えさせる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub path: String,
    pub side: Side,
    /// side が File のときは None。それ以外は 1 始まり、両端を含む。
    #[serde(default)]
    pub start: Option<u32>,
    #[serde(default)]
    pub end: Option<u32>,
}

impl Range {
    /// この範囲が指す位置を列挙する。変更一覧に無い位置もそのまま返す
    /// （存在しない行を指したことを検査側で捕まえるため、ここでは黙らせない）。
    pub fn positions(&self) -> Vec<Position> {
        match (self.side, self.start, self.end) {
            (Side::File, _, _) => vec![Position::file(self.path.clone())],
            (side, Some(s), Some(e)) if s <= e => (s..=e)
                .map(|n| Position::new(self.path.clone(), side, n))
                .collect(),
            (side, Some(s), None) => vec![Position::new(self.path.clone(), side, s)],
            _ => Vec::new(),
        }
    }
}

/// 変更の重要度。全ての変更箇所がちょうど 1 つに属する。
///
/// 振る舞いが変わったかを見るのは帰結の中を割るためだけで、Core とは競合
/// しない (主目的の実装が振る舞いを変えていても Core)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Importance {
    /// この PR の主目的そのもの。読まないと変更が分からない。
    Core,
    /// 主目的の帰結だが、振る舞いが変わった。流しそうになるが流せない。
    Ripple,
    /// 主目的の帰結で、振る舞いは変わらない。
    Follow,
    /// テスト、版番号、コメントのみ。
    Minor,
}

impl Importance {
    pub fn label_ja(self) -> &'static str {
        match self {
            Importance::Core => "本題",
            Importance::Ripple => "影響あり",
            Importance::Follow => "影響なし",
            Importance::Minor => "おまけ",
        }
    }

    /// 読む順。強い方が先。
    pub const ORDER: [Importance; 4] = [
        Importance::Core,
        Importance::Ripple,
        Importance::Follow,
        Importance::Minor,
    ];
}

/// 段階 1。毎回同じ 5 欄が同じ順で埋まる。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Overview {
    /// 何が不足・破綻していたか。「〜という機能を足したい」もここ。
    pub problem: String,
    /// 追加・変更した責務。名前ではなく責務で書く。
    pub change: String,
    /// 既存の何と新規の何が、どう繋がって動くか。
    pub mechanism: String,
    /// なぜそのディレクトリ・型なのか。他の候補を退けた理由。
    pub placement: String,
    /// ファイル数と、実際に読むべき数。
    pub scope: String,
}

/// 段階 2 の 1 項目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    pub body: String,
    pub importance: Importance,
    /// なぜその重要度なのか。どの重要度でも必須。
    ///
    /// 誤分類は機械では見つからないが、理由が読めれば人が見つけられる。
    /// 一部の重要度にだけ課すと、書く手間を避けて課していない側へ逃げる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub ranges: Vec<Range>,
    /// 他の項目との関係。無い項目もある。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
}

/// 項目から項目への関係。「これは何の帰結か」を指す。
///
/// 相手を添字ではなく title で指す。添字は数え間違えても黙って別の項目を
/// 指すが、title なら存在しない相手を指したことが後から分かる。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    pub to: String,
    /// なぜそう言えるのか。関係の種類はここへ吸収し、別の欄にはしない。
    pub reason: String,
    /// 主の関係。読む順はこれだけを辿るので、1 つの項目に高々 1 つ。
    #[serde(default)]
    pub primary: bool,
}

/// 主張の確度。静的に確定しているものと、読んで書いた予想を混ぜない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Fact,
    Guess,
}

/// 段階 3。関数ではなく機能の単位。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Impact {
    /// 機能の名前。利用者が認識している呼び方で書く。
    pub feature: String,
    /// 何がどう変わるか。
    pub change: String,
    /// 何を確かめれば分かるか。
    pub verify: String,
    /// 残る穴。無ければ None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<String>,
    pub confidence: Confidence,
}

/// 説明もれ検査の結果。生成物ではなく検査の出力なので、モデルには書かせない。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Coverage {
    /// 変更一覧にある変更箇所の総数。
    pub total: usize,
    /// ちょうど 1 つの項目に属した位置の数。
    pub classified: usize,
    /// どの項目にも属さなかった位置。
    pub unclassified: Vec<Position>,
    /// 2 つ以上の項目に属した位置。
    pub conflicts: Vec<Position>,
    /// 項目が指したが、変更一覧に存在しない位置。行番号の作り話を捕まえる。
    pub unknown: Vec<Position>,
}

impl Coverage {
    /// 全ての変更箇所がちょうど 1 つの項目に属し、作り話も無いか。
    pub fn is_complete(&self) -> bool {
        self.unclassified.is_empty() && self.conflicts.is_empty() && self.unknown.is_empty()
    }
}

/// 前回の成果物からの進み。作り直すたびに引き直す。
///
/// 2 度目以降の読者が全部を読み直さずに済むように、毎回ゼロベースで作る
/// レビュー本体とは別に持たせる。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SincePrevious {
    /// 比べる起点。今の HEAD と違う HEAD で作られた、直近の成果物が見ていた
    /// コミット。
    pub previous_head: String,
    /// 今回の HEAD コミット。
    pub head: String,
    /// 前回の HEAD から今の作業ツリーまでで変わったファイル。引けなければ None。
    ///
    /// 空の Vec と None を畳まない。前回のコミットが残っていなくて引けなかった
    /// だけなのに「変わったファイルは無い」と言うと、山ほど動いていても無いと
    /// 言い切ることになる。
    pub files: Option<Vec<String>>,
    /// 前回の HEAD が今の履歴から辿れない（rebase / amend / force push、
    /// あるいは古いコミットへの巻き戻し）。
    pub history_rewritten: bool,
}

/// 成果物 1 件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub schema: u32,
    /// レビューの起点。ベースと HEAD の共通祖先のコミット ID。
    pub base: String,
    /// 解析時の HEAD コミット ID。差分の終点はここではなく作業ツリー。
    pub head: String,
    pub overview: Overview,
    /// 重要度順に並べる。同じ重要度の中は元の順を保つ。
    pub sections: Vec<Section>,
    pub impacts: Vec<Impact>,
    pub coverage: Coverage,
    /// 前回の成果物からの進み。初回は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_previous: Option<SincePrevious>,
}

impl Review {
    pub fn sort_sections(&mut self) {
        self.sections.sort_by_key(|c| c.importance);
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 成果物の置き場は区間ごとに1つずつホストのディレクトリの下にある() {
        let repo = std::path::Path::new("/repo");
        assert_eq!(
            artifact_path(repo, Scope::Base),
            std::path::Path::new("/repo/.conductor/review.json")
        );
        for scope in [Scope::Base, Scope::SincePrevious] {
            assert!(
                artifact_path(repo, scope).starts_with(repo.join(DIR)),
                "{scope:?}"
            );
        }
        assert_ne!(
            artifact_path(repo, Scope::Base),
            artifact_path(repo, Scope::SincePrevious)
        );
    }

    #[test]
    fn 範囲は指す位置に展開される() {
        let file = Position::file("src/x.rs");
        let new = |n| Position::new("src/x.rs", Side::New, n);
        let old = |n| Position::new("src/x.rs", Side::Old, n);
        for (name, side, start, end, want) in [
            (
                "両端を含む全ての行",
                Side::New,
                Some(10),
                Some(12),
                vec![new(10), new(11), new(12)],
            ),
            ("始点だけなら 1 つ", Side::Old, Some(4), None, vec![old(4)]),
            // start > end はモデルの誤りなので、黙って直さず空にする。当該位置が
            // unclassified に落ちて説明もれ検査で表に出る。
            ("逆向きは空", Side::New, Some(9), Some(2), vec![]),
            ("行番号の無い範囲は空", Side::New, None, None, vec![]),
            (
                "ファイル側は行番号を無視",
                Side::File,
                Some(1),
                Some(100),
                vec![file.clone()],
            ),
        ] {
            let r = Range {
                path: "src/x.rs".into(),
                side,
                start,
                end,
            };
            assert_eq!(r.positions(), want, "{name}");
        }
    }

    #[test]
    fn 重要度と側はjsonの小文字綴りを往復する() {
        for (imp, name) in [
            (Importance::Core, "core"),
            (Importance::Ripple, "ripple"),
            (Importance::Follow, "follow"),
            (Importance::Minor, "minor"),
        ] {
            let json = serde_json::to_string(&imp).unwrap();
            assert_eq!(json, format!("\"{name}\""), "{imp:?}");
            assert_eq!(serde_json::from_str::<Importance>(&json).unwrap(), imp);
        }
        for (side, name) in [(Side::New, "new"), (Side::Old, "old"), (Side::File, "file")] {
            let json = serde_json::to_string(&side).unwrap();
            assert_eq!(json, format!("\"{name}\""), "{side:?}");
            assert_eq!(serde_json::from_str::<Side>(&json).unwrap(), side);
        }
    }
}
