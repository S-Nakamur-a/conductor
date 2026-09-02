//! ツールが受け取る、ワイヤレベルの引数。tools/call から直接デシリアライズされる。
//!
//! これらのフィールドの doc コメントはそのまま JSON Schema の description になり
//! モデルが読むので、その読み手に向けて書いてある。文面はツールの公開契約。

use schemars::JsonSchema;
use serde::Deserialize;

use conductor_core::review_store::CommentKind;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPendingComments {
    /// worktree 名で絞り込む
    #[serde(default)]
    pub worktree: Option<String>,
    /// ブランチ名で絞り込む。省略時は現在の git ブランチ（自動検出）が使われる。
    #[serde(default)]
    pub branch: Option<String>,
    /// true にすると全ブランチのコメントを返す（自動ブランチ絞り込みを無効化する）
    #[serde(default)]
    pub all_branches: Option<bool>,
    /// ファイルパスで絞り込む（完全一致）
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommentIdOnly {
    /// コメント ID、または一意に定まるプレフィックス（8文字以上）
    pub comment_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplyToComment {
    /// コメント ID、または一意に定まるプレフィックス（8文字以上）
    pub comment_id: String,
    /// 返信の本文
    pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateComment {
    /// コメントを付けるファイルの、リポジトリルートからの相対パス（例: src/foo.rs）
    pub file_path: String,
    /// コメントが始まる行番号（1始まり）
    pub line_start: u32,
    /// 複数行にわたる範囲の終了行（1始まり）。単一行のコメントなら省略する
    #[serde(default)]
    pub line_end: Option<u32>,
    /// コメントの本文
    pub body: String,
    /// メモ・所感・トレードオフなら 'suggest'（デフォルト）、人間に答えてほしい問いなら 'question'
    #[serde(default)]
    pub kind: Option<CommentKindArg>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CommentKindArg {
    Suggest,
    Question,
}

impl From<CommentKindArg> for CommentKind {
    fn from(k: CommentKindArg) -> Self {
        match k {
            CommentKindArg::Suggest => CommentKind::Suggest,
            CommentKindArg::Question => CommentKind::Question,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetChangeSummary {
    /// Markdown で書いた change summary。Conductor Viewer では見出し (#)、リスト (-, 1.)、
    /// 引用 (>)、インラインコード (`x`)、太字/斜体 (**/*)、そして構文ハイライトの付く
    /// フェンス付きコードブロック (```lang) がレンダリングされる。注意: _ は強調にならない
    /// (snake_case がそのまま保たれる)。変更が何をするのか、なぜするのかの概要を簡潔に
    /// 書くこと。複数行にまたがってよい。
    pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetChangeSummary {
    /// summary を読み出すブランチ。省略時は現在の git ブランチを使う。
    #[serde(default)]
    pub branch: Option<String>,
}
