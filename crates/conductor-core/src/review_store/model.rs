//! review_store の行に対応する型。enum は DB の文字列表現と相互変換できる。

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

macro_rules! db_enum {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// DB に保存される文字列表現。
            pub fn as_str(&self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.as_str()))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                match value.as_str()? {
                    $($text => Ok(Self::$variant),)+
                    other => Err(FromSqlError::Other(
                        format!("unknown {}: {other}", stringify!($name)).into(),
                    )),
                }
            }
        }
    };
}

db_enum!(CommentKind { Suggest => "suggest", Question => "question" });
db_enum!(Author { User => "user", Claude => "claude" });
db_enum!(CommentStatus { Pending => "pending", Resolved => "resolved" });

/// ファイルと行範囲に紐づくレビューコメント。
#[derive(Debug, Clone)]
pub struct ReviewComment {
    pub id: String,
    pub worktree: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: Option<u32>,
    pub kind: CommentKind,
    pub body: String,
    pub status: CommentStatus,
    pub author: Author,
    pub branch: Option<String>,
    pub created_at: String,
}

/// これから挿入するコメント。worktree 列と branch 列には同じブランチ名が入る。
#[derive(Debug, Clone, Copy)]
pub struct NewReview<'a> {
    pub branch: &'a str,
    pub file_path: &'a str,
    pub line_start: u32,
    pub line_end: Option<u32>,
    pub kind: CommentKind,
    pub body: &'a str,
    pub author: Author,
}

/// 保存済みのコメント雛形。
#[derive(Debug, Clone)]
pub struct CommentTemplate {
    pub id: String,
    pub name: String,
    pub body: String,
    pub kind: CommentKind,
}

/// コメントへの返信。
#[derive(Debug, Clone)]
pub struct ReviewReply {
    pub id: String,
    pub body: String,
    pub author: Author,
    pub created_at: String,
}

/// ブランチに紐づく PR の素性。PR の無いブランチ名だけからも始められるので全て任意。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrReviewMeta {
    pub pr_number: Option<i64>,
    pub pr_url: Option<String>,
    pub pr_title: Option<String>,
    pub base_ref: Option<String>,
    pub head_ref: Option<String>,
    pub author: Option<String>,
}

/// ターミナル出力のスナップショット。
#[derive(Debug, Clone)]
pub struct SessionHistory {
    pub worktree: String,
    pub label: String,
    pub kind: String,
    pub output_text: String,
    pub saved_at: String,
}
