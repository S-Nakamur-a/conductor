//! ストリップ 1 行の割り付け。

use conductor_core::git_engine::WorktreeInfo;

use crate::strip::{ADD, CLOSE, push, visible_window, width_of};
use crate::workspace::Workspace;

const SEP: &str = "\u{2502} ";

/// ストリップの 1 区画が何を指しているか。
#[derive(Debug, PartialEq, Eq)]
pub enum SlotKind {
    /// 先頭の印。作成や削除を待っている間だけ形が変わる。
    Lead,
    Select(usize),
    Delete(usize),
    Add,
    Sep,
    /// 送り印と「worktree が無い」の案内。押しても何も起きない。
    Muted,
}

pub type Slot = crate::strip::Slot<SlotKind>;

/// worktree 1 つを一目で表す文字列。ブランチ、変更数、ahead/behind。
fn chip_text(worktree: &WorktreeInfo, waiting: bool, active: bool) -> String {
    let mut text = String::from(" ");
    if waiting {
        text.push_str("\u{23f3} ");
    } else if active {
        text.push_str("\u{25cf} ");
    }
    text.push_str(&worktree.branch);
    if !worktree.is_clean {
        text.push_str(&format!(
            " ~{}",
            worktree.added + worktree.modified + worktree.deleted
        ));
    }
    if let Some(ahead) = worktree.ahead.filter(|a| *a > 0) {
        text.push_str(&format!(" \u{2191}{ahead}"));
    }
    if let Some(behind) = worktree.behind.filter(|b| *b > 0) {
        text.push_str(&format!(" \u{2193}{behind}"));
    }
    text.push(' ');
    text
}

/// `width` に収まるチップと、その前後に付く印。
///
/// 窓は毎フレーム選択から決め直す。描画はスクロール位置を書き戻せないし、
/// 選択が必ず見えていれば覚えておく必要もない。そのぶん送り印は押せない。
pub fn slots(ws: &Workspace, width: u16) -> Vec<Slot> {
    let panel = &ws.panels.worktree;
    let lead = if panel.is_busy() {
        "\u{22ef} "
    } else {
        "\u{2387} "
    };

    let mut out = Vec::new();
    push(&mut out, lead.into(), SlotKind::Lead);
    if panel.list().is_empty() {
        push(&mut out, "no worktrees ".into(), SlotKind::Muted);
        return out;
    }

    let chips: Vec<(String, bool)> = panel
        .list()
        .iter()
        .map(|worktree| {
            let text = chip_text(
                worktree,
                ws.panels.terminal.is_waiting(&worktree.path),
                ws.panels.terminal.is_active(&worktree.path),
            );
            (text, !worktree.is_main)
        })
        .collect();

    let widths: Vec<u16> = chips
        .iter()
        .map(|(text, deletable)| width_of(text) + if *deletable { width_of(CLOSE) } else { 0 })
        .collect();
    let avail = width.saturating_sub(width_of(lead) + width_of(ADD));
    let (start, end) = visible_window(
        &widths,
        width_of(SEP),
        avail,
        0,
        panel.selected_index(),
        true,
    );

    if start > 0 {
        push(&mut out, format!("\u{2039}{start} "), SlotKind::Muted);
    }
    for (i, (text, deletable)) in chips.iter().enumerate().take(end).skip(start) {
        if i > start {
            push(&mut out, SEP.into(), SlotKind::Sep);
        }
        push(&mut out, text.clone(), SlotKind::Select(i));
        if *deletable {
            push(&mut out, CLOSE.into(), SlotKind::Delete(i));
        }
    }
    if end < chips.len() {
        push(
            &mut out,
            format!(" {}\u{203a} ", chips.len() - end),
            SlotKind::Muted,
        );
    }
    push(&mut out, ADD.into(), SlotKind::Add);
    out
}
