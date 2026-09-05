//! レビューを作る前の確認。
//!
//! AI の呼び出しは数分と費用がかかるので、W の押し間違いやメニューの隣の行を選んだ
//! だけで走り出さないようにする。

use conductor_core::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::panels::revidere::Artifact;

#[derive(Debug)]
pub struct RevidereConfirm {
    pub branch: String,
    /// 見る区間の呼び名。
    pub scope: &'static str,
    pub artifact: Artifact,
    /// y で流す Effect。対象を捕まえたまま組み立てられるよう、閉包ではなく実体を持つ。
    pub on_yes: Vec<Effect>,
}

/// 作り直しなのか初めてなのかで、押す前に知りたいことが変わる。
fn wording(artifact: Artifact) -> (&'static str, &'static str, &'static str) {
    match artifact {
        Artifact::None => ("Review", "No review for this worktree yet.", "analyse"),
        Artifact::Stale => (
            "Review",
            "A review exists, but commits have landed since.",
            "analyse",
        ),
        Artifact::Current => (
            "Re-analyse",
            "A review for this commit already exists.",
            "re-analyse",
        ),
    }
}

pub fn title(confirm: &RevidereConfirm) -> String {
    wording(confirm.artifact).0.to_string()
}

pub fn lines(confirm: &RevidereConfirm, theme: &Theme) -> Vec<Line<'static>> {
    let (_, situation, verb) = wording(confirm.artifact);
    vec![
        Line::styled(situation, Style::default().fg(theme.fg)),
        Line::styled(
            format!("{} [{}]", confirm.branch, confirm.scope),
            Style::default().fg(theme.muted),
        ),
        Line::styled(
            "It calls the AI and takes a few minutes.",
            Style::default().fg(theme.muted),
        ),
        Line::default(),
        Line::from(vec![
            Span::styled(
                "y",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(": {verb} / "), Style::default().fg(theme.muted)),
            Span::styled(
                "n",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(": cancel", Style::default().fg(theme.muted)),
        ]),
    ]
}
