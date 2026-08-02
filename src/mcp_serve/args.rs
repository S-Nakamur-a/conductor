//! 8個のツールが受け取る、ワイヤレベルの引数。tools/call リクエストから直接
//! デシリアライズされる。
//!
//! これらのフィールドの doc コメントはそのまま JSON Schema の description に
//! なりモデルが読むので、その読み手に向けて書いてあり、Node サーバの
//! .describe(...) 呼び出しの文面を逐語的に受け継いでいる。

use schemars::JsonSchema;
use serde::Deserialize;

use crate::review_store::CommentKind;
use crate::walkthrough::WalkthroughStepKind;

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

/// モデルから渡された時点のステップ。まだウォークスルーに紐付けられる前の形。
///
/// seq はワイヤスキーマの一部だが（古いプロンプトが引き続き通用するように
/// 残してある）実際には読まれない。保存される順序を決めるのは
/// SaveWalkthrough::steps 自身のスライス順である — 理由は
/// [crate::walkthrough::NewWalkthroughStep] の doc を参照。
#[derive(Debug, Deserialize, JsonSchema)]
pub struct WalkthroughStep {
    /// ウォークスルー内でのステップ順序（0始まり）
    pub seq: i64,
    /// ステップが指すファイルの、リポジトリルートからの相対パス（例: src/foo.rs）
    pub file_path: String,
    /// ファイルに紐付くステップの場合、ステップが指す行番号（1始まり）
    #[serde(default)]
    pub line_start: Option<i64>,
    /// 複数行にわたる範囲の終了行（1始まり）。単一行なら省略する
    #[serde(default)]
    pub line_end: Option<i64>,
    /// 'intent'（なぜこの変更をしたか）、'core'（実装の中心部分）、
    /// 'ripple'（他箇所への波及的な変更）、'test'（テストが何をカバーしているか）のいずれか
    pub kind: StepKindArg,
    /// ステップの短い見出し
    pub title: String,
    /// ステップの説明。kind ごとの内容の取り決めに従う
    pub body: String,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StepKindArg {
    Intent,
    Core,
    Ripple,
    Test,
}

impl From<StepKindArg> for WalkthroughStepKind {
    fn from(k: StepKindArg) -> Self {
        match k {
            StepKindArg::Intent => WalkthroughStepKind::Intent,
            StepKindArg::Core => WalkthroughStepKind::Core,
            StepKindArg::Ripple => WalkthroughStepKind::Ripple,
            StepKindArg::Test => WalkthroughStepKind::Test,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveWalkthrough {
    /// ウォークスルーが属するブランチ
    pub branch: String,
    /// ウォークスルーの1行タイトル
    pub title: String,
    /// 変更の概要。ブランチの change summary としても保存され、Conductor の
    /// SUMMARY 疑似ファイルとしてパネル全体に表示されるので、PR の説明文の
    /// ように書くこと（この変更が何のためか、なぜこれらのファイルを触るのか、
    /// 対象外にしたことは何か）。Markdown としてレンダリングされる。
    pub summary: String,
    /// 順序付きのウォークスルーのステップ（各ステップのフィールドは save_walkthrough を参照）
    pub steps: Vec<WalkthroughStep>,
}
