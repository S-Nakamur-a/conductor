//! Wire-level arguments for the eight tools, deserialized straight off the
//! `tools/call` request.
//!
//! Doc comments on these fields become the JSON Schema `description` the
//! model reads, so they are written for that audience and kept verbatim from
//! the Node server's `.describe(...)` calls.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::review_store::CommentKind;
use crate::walkthrough::WalkthroughStepKind;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPendingComments {
    /// Filter by worktree name
    #[serde(default)]
    pub worktree: Option<String>,
    /// Filter by branch name. If omitted, defaults to the current git branch (auto-detected).
    #[serde(default)]
    pub branch: Option<String>,
    /// Set to true to return comments from all branches (disables auto branch filter)
    #[serde(default)]
    pub all_branches: Option<bool>,
    /// Filter by file path (exact match)
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CommentIdOnly {
    /// Comment ID or unique prefix (min 8 chars)
    pub comment_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplyToComment {
    /// Comment ID or unique prefix (min 8 chars)
    pub comment_id: String,
    /// Reply text
    pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateComment {
    /// Repo-relative file path the comment attaches to (e.g. src/foo.rs)
    pub file_path: String,
    /// 1-based line number the comment starts on
    pub line_start: u32,
    /// 1-based end line for a multi-line range; omit for a single-line comment
    #[serde(default)]
    pub line_end: Option<u32>,
    /// The comment text
    pub body: String,
    /// 'suggest' (default) for a note/observation/tradeoff; 'question' when you want the human to answer something
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
    /// The change summary in Markdown. It is rendered with headings (#), lists (-, 1.), block
    /// quotes (>), inline code (`x`), bold/italic (**/*), and fenced code blocks (```lang) that
    /// get syntax highlighting in the Conductor Viewer. Note: `_` does not produce emphasis (so
    /// snake_case stays intact). Write a concise overview of what the change does and why; may
    /// span multiple lines.
    pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetChangeSummary {
    /// Branch to read the summary for. Omit to use the current git branch.
    #[serde(default)]
    pub branch: Option<String>,
}

/// A step as supplied by the model, before it is anchored to a walkthrough.
///
/// `seq` is part of the wire schema (kept so an older prompt continues to
/// validate) but is not read: `SaveWalkthrough::steps`'s own slice order is
/// what determines the saved order — see
/// [`crate::walkthrough::NewWalkthroughStep`]'s doc for why.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct WalkthroughStep {
    /// Step order within the walkthrough, 0-based
    pub seq: i64,
    /// Repo-relative file path the step points at (e.g. src/foo.rs)
    pub file_path: String,
    /// 1-based line number the step points at, if file-anchored
    #[serde(default)]
    pub line_start: Option<i64>,
    /// 1-based end line for a multi-line range; omit for a single line
    #[serde(default)]
    pub line_end: Option<i64>,
    /// 'intent' (why this change), 'core' (the main implementation), 'ripple' (knock-on changes
    /// elsewhere), or 'test' (what the tests cover)
    pub kind: StepKindArg,
    /// Short step heading
    pub title: String,
    /// Step explanation, following the kind's content contract
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
    /// Branch the walkthrough belongs to
    pub branch: String,
    /// One-line walkthrough title
    pub title: String,
    /// Overview of the change. Also stored as the branch's change summary and shown full-panel as
    /// Conductor's SUMMARY pseudo-file, so write it like a PR description (what the change is for,
    /// why these files, what is out of scope). Markdown is rendered.
    pub summary: String,
    /// Ordered walkthrough steps (see save_walkthrough's step fields)
    pub steps: Vec<WalkthroughStep>,
}
