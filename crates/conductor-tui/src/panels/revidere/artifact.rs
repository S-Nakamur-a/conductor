//! 成果物 (`<worktree>/.conductor/review.json`) を読み、読む順を組む。ワーカーで走る。
//!
//! 型を宣言し直さない。Importance も Side も Section も revidere の側にあるものを
//! そのまま使う。ホストが写しを持つと、スキーマが動いたときに黙って壊れる。
//!
//! diff まで revidere から取るのは、読む順を組むには解析が見たのと同じ diff が
//! 要るため。パスの綴り・削除行の前像行番号・rename の扱いが 1 つでもずれると、
//! 項目が指す位置が変更一覧から外れて「説明の無い変更行」に化ける。

use std::path::Path;

use revidere::{Annotations, ReadingOrder, Scope};

/// 読み込み済みの成果物と、そこから組んだ読む順。
#[derive(Debug)]
pub struct Loaded {
    pub annotations: Annotations,
    pub order: ReadingOrder,
    /// 成果物が対象にしていた区間の起点。終点は作業ツリーなので対になる commit id は無い。
    pub base: String,
}

impl Loaded {
    /// 全ての変更箇所がちょうど 1 つの項目に属し、行番号の作り話も無いか。
    pub fn is_complete(&self) -> bool {
        self.annotations.coverage().is_complete()
    }

    pub fn total_positions(&self) -> usize {
        self.annotations.coverage().total
    }

    pub fn unexplained(&self) -> usize {
        let coverage = self.annotations.coverage();
        coverage.total.saturating_sub(coverage.classified)
    }

    /// 解析したときの HEAD。確認ダイアログが今のコミットと突き合わせる。
    pub fn head(&self) -> &str {
        self.annotations.head()
    }
}

/// [load] の結果。「無い」と「壊れている」を畳まない。
#[derive(Debug)]
pub enum Outcome {
    /// 成果物がまだ無い。走らせていないだけなので正常な状態。
    Missing,
    Loaded(Box<Loaded>),
    /// 読めるはずのものが読めなかった。JSON が壊れている、スキーマ版が違う、
    /// あるいは diff が取れない。どれもユーザに見せる価値のある異常。
    Broken(String),
}

pub fn load(worktree: &Path, scope: Scope) -> Outcome {
    let path = revidere::review::artifact_path(worktree, scope);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Outcome::Missing,
        Err(e) => return Outcome::Broken(format!("{}: {e}", path.display())),
    };
    let annotations = match Annotations::from_json(&text) {
        Ok(annotations) => annotations,
        Err(e) => return Outcome::Broken(format!("{}: {e}", path.display())),
    };
    // 前回からの差分は 1 ラウンド 1 枚。起点が今の前回と違えば、それは前の回の
    // 成果物で、この回についてはまだ解析していないのと同じ。壊れてはいない。
    if scope == Scope::SincePrevious
        && Some(annotations.base()) != previous_head(worktree).as_deref()
    {
        return Outcome::Missing;
    }

    // 起点は成果物に書いてあるコミット ID をそのまま使う。ここで base を推定し直すと、
    // 解析のあとにベースが動いただけで項目の指す位置が全部ずれる。
    let raw = match revidere::git::diff(worktree, annotations.base()) {
        Ok(raw) => raw,
        Err(e) => return Outcome::Broken(format!("diff を取れない: {}", e.0)),
    };
    let diff = revidere::diff::parse(&raw);
    let order = ReadingOrder::build(&diff, &annotations);
    Outcome::Loaded(Box::new(Loaded {
        base: annotations.base().to_string(),
        annotations,
        order,
    }))
}

/// 差分を解析するときの起点であり、その成果物が今の回のものかを見分ける鍵でもある。
/// 写しを 2 か所に持つと、片方だけ古くなったときに黙って別の区間を見ることになる。
pub fn previous_head(worktree: &Path) -> Option<String> {
    let text =
        std::fs::read_to_string(revidere::review::artifact_path(worktree, Scope::Base)).ok()?;
    let annotations = Annotations::from_json(&text).ok()?;
    Some(annotations.since_previous()?.previous_head.clone())
}

/// 画面とステータスに出す区間の呼び名。1 か所で持つ。
pub fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Base => "ブランチ全体",
        Scope::SincePrevious => "前回のレビューから",
    }
}

/// 重要度の色。意味は revidere の側にあり、こちらが決めるのは見え方だけ。
pub fn importance_color(importance: revidere::Importance) -> ratatui::style::Color {
    let (r, g, b) = importance.recommended_rgb();
    ratatui::style::Color::Rgb(r, g, b)
}
