//! 離した選択を、クリップボードへ渡す文字にする。見た目の行の境目が折り返しか本物の
//! 改行かは、区画が映しているものに聞く。

use std::path::PathBuf;

use ratatui::layout::Rect;

use crate::layout::{Layout, Region};
use crate::panels::terminal::render::{content_area, pty_area, soft_wrapped_rows};
use crate::rejoin::{self, Break};
use crate::select::Picked;
use crate::workspace::{Focus, Workspace};

/// `Task::RejoinCopy` に渡すもの。ログは数十 MB になるので UI では読まない。
#[derive(Debug, Clone)]
pub struct FromLog {
    pub working_dir: PathBuf,
    pub session_id: String,
    pub picked: Picked,
    pub seq: u64,
}

/// すぐ書く文字と、ログで決め直す仕事。
pub fn resolve(
    ws: &mut Workspace,
    layout: &Layout,
    region: Region,
    picked: Picked,
) -> (String, Option<FromLog>) {
    ws.chrome.copy_seq += 1;
    let Some(rect) = layout.rect(region) else {
        return (picked.plain(), None);
    };
    match region {
        Region::TerminalShell | Region::FleetChanges => {
            let wrapped = soft_wrapped_rows(ws, region, rect);
            let breaks: Vec<Break> = (0..picked.rows.len().saturating_sub(1))
                .map(|i| {
                    if wrapped.contains(&(picked.top + i as u16)) {
                        Break::SoftWrap
                    } else {
                        Break::Newline
                    }
                })
                .collect();
            (picked.join(&breaks), None)
        }
        Region::TerminalClaude | Region::FleetSession => claude(ws, region, rect, picked),
        _ => (picked.plain(), None),
    }
}

fn claude(
    ws: &mut Workspace,
    region: Region,
    rect: Rect,
    picked: Picked,
) -> (String, Option<FromLog>) {
    let (session, content) = match region {
        Region::TerminalClaude => (
            ws.panels.terminal.session_in(Focus::TerminalClaude),
            content_area(rect),
        ),
        _ => (ws.fleet_session(), pty_area(rect)),
    };
    let panel = &ws.panels.terminal;
    if let Some(reflow) = panel.transcript_of(session.as_deref()) {
        let Some(highlighter) = ws.panels.viewer.highlighter_ref() else {
            return (picked.plain(), None);
        };
        let first = usize::from(content.y.saturating_sub(picked.top));
        let shown = 0..usize::from(content.height);
        let source = reflow.unwrapped_text(shown, &ws.theme, highlighter);
        let mut rows = picked.rows.clone();
        rows.drain(..first.min(rows.len()));
        let mut breaks = vec![Break::Newline; first];
        breaks.extend(rejoin::breaks(&rows, &source));
        return (picked.join(&breaks), None);
    }
    let Some((working_dir, session_id)) = panel.claude_log_of(session.as_deref()) else {
        return (picked.plain(), None);
    };
    (
        picked.plain(),
        Some(FromLog {
            working_dir,
            session_id,
            picked,
            seq: ws.chrome.copy_seq,
        }),
    )
}

/// ログの本文で区切りを決め直した文字。見た目どおりと変わらなければ None。
pub fn from_log(job: &FromLog, entries: &[conductor_core::claude_log::LogEntry]) -> Option<String> {
    let source = rejoin::log_text(entries);
    let text = job.picked.join(&rejoin::breaks(&job.picked.rows, &source));
    (text != job.picked.plain()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskResult;
    use conductor_core::claude_log::{DisplayBlock, LogEntry, Role};

    fn job(rows: &[&str], seq: u64) -> FromLog {
        let rows: Vec<String> = rows.iter().map(|r| r.to_string()).collect();
        FromLog {
            working_dir: PathBuf::from("/tmp"),
            session_id: "s".into(),
            picked: Picked {
                rows: rows.clone(),
                top: 0,
                first: 0,
                parts: rows,
            },
            seq,
        }
    }

    #[test]
    fn ログの本文で折り返しを繋ぐ() {
        let entries = vec![LogEntry {
            role: Role::Assistant,
            blocks: vec![DisplayBlock::Text("one two three\nfour".into())],
        }];
        let job = job(&["⏺ one two", "  three", "  four"], 1);
        assert_eq!(
            from_log(&job, &entries).as_deref(),
            Some("⏺ one two three\n  four")
        );
    }

    #[test]
    fn 見た目どおりと変わらなければ書き直さない() {
        let entries = vec![LogEntry {
            role: Role::Assistant,
            blocks: vec![DisplayBlock::Text("one\ntwo".into())],
        }];
        assert_eq!(from_log(&job(&["one", "two"], 1), &entries), None);
    }

    #[test]
    fn ログで決め直せなかった結果は書かない() {
        let mut ws = Workspace::for_test();
        ws.chrome.copy_seq = 1;
        let _ = ws.accept(TaskResult::Rejoined { seq: 1, text: None });
        assert_eq!(ws.chrome.clipboard, None);
    }

    #[test]
    fn 後のコピーが済んでから届いた前の結果は書かない() {
        let mut ws = Workspace::for_test();
        ws.chrome.copy_seq = 2;
        let _ = ws.accept(TaskResult::Rejoined {
            seq: 1,
            text: Some("old".into()),
        });
        assert_eq!(ws.chrome.clipboard, None);
        let _ = ws.accept(TaskResult::Rejoined {
            seq: 2,
            text: Some("new".into()),
        });
        assert_eq!(ws.chrome.clipboard.as_deref(), Some("new"));
    }
}
