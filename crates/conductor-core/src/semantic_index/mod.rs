//! sheaf-core の [`sheaf_core::Store`] を conductor に橋渡しする層。
//!
//! `.conductor/` は main worktree にしか無いので、置き場所は `repo_root` から
//! `commondir()` を辿って解決する。照合先のツリー (`tree_root`) は選択中の worktree で、
//! これとは別物。
//!
//! 出自 (生成時点で実際にディスクにあった内容のハッシュ) を外から申告するのは、SCIP 索引が
//! ソース本文を持たないため。コミットを出自にすると、作業ツリーを索引したときに未追跡
//! ファイルが永久に鮮度の検査を通らなくなる。

mod bridge;
mod history;
pub(crate) mod roots;
mod state;
mod survey;

#[cfg(test)]
mod tests;

pub use bridge::Bridge;
pub use roots::IndexRoot;
pub use sheaf_core::Regenerated;
pub use state::{Finished, SemanticIndex};
pub use survey::{BuildOutcome, Built, Survey, build_index, survey, survey_and_load};

use std::path::{Path, PathBuf};

/// いま読んでいるファイルに対して索引がどこまで答えられるか。
///
/// 「まだ無い」「対象外」「古い」を 1 つの `bool` に潰すと、作っている最中に古いと言うことになる。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reading {
    /// 前回と同じファイル。何も起きていない。
    Unchanged,
    /// 索引が今の内容を説明している。
    Indexed,
    /// 今の内容の索引はあるのに、このファイルが載っていない。
    ///
    /// producer がそのファイルを索引しなかったか、生成中に動いて出自から落ちたか、
    /// producer を起動できないか。作り直しても同じものが出るので作りに行かない。
    Stale,
    /// このルートを索引しているところ。
    Building,
    /// 索引はあるが、まだ読み込めていない。答えは次の周に持ち越す。
    Loading,
    /// 索引の対象ではない。
    NotIndexed,
}

/// 種別を、ホバーの見出しに置く 1 語にする。
///
/// 綴りを Rust の宣言キーワードに寄せてあるのは、本文に索引の書いた宣言がそのまま並ぶため。
/// 読めない種別は空にして、見出しごと出さない。
pub fn kind_label(kind: sheaf_core::SymbolKind) -> &'static str {
    use sheaf_core::SymbolKind::*;
    match kind {
        Function => "fn",
        Method => "method",
        Struct => "struct",
        Class => "class",
        Enum => "enum",
        EnumMember => "variant",
        Field => "field",
        Trait => "trait",
        Interface => "interface",
        Package => "package",
        TypeAlias => "type",
        AssociatedType => "assoc type",
        ImplBlock => "impl",
        Module => "mod",
        Constant => "const",
        Static => "static",
        Variable => "let",
        Parameter => "param",
        SelfParameter => "self",
        TypeParameter => "type param",
        Unknown => "",
    }
}

/// リンクされた worktree でも `commondir()` は常に main の `.git` を指すので、その親が
/// main のルートになる。
pub(crate) fn main_conductor_dir(repo_root: &Path) -> Option<PathBuf> {
    let repo = git2::Repository::open(repo_root).ok()?;
    Some(repo.commondir().parent()?.join(".conductor"))
}
