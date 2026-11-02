//! diff の出どころ。

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSource {
    /// merge-base(base, HEAD) から作業ツリー (index 込み) まで。
    WorkingTree { base: String },
    /// 最初の親からそのコミットまで。ルートコミットは空の tree から数え、マージ
    /// コミットは 2 つ目以降の親を見ない (git diff <c>^ <c> と同じ)。
    Commit { oid: String },
}

impl DiffSource {
    pub fn working_tree(base: &str) -> Self {
        Self::WorkingTree {
            base: base.to_string(),
        }
    }

    pub fn commit(oid: &str) -> Self {
        Self::Commit {
            oid: oid.to_string(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::WorkingTree { .. } => "working tree".to_string(),
            Self::Commit { oid } => short_oid(oid).to_string(),
        }
    }
}

pub fn short_oid(oid: &str) -> &str {
    &oid[..8.min(oid.len())]
}
