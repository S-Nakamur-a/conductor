//! セッションタブ行の中身と配置。

use conductor_svc::pty::SessionKind;

use crate::strip::{ADD, CLOSE, push, visible_window, width_of};

/// タブ行の 1 区画が何を指しているか。
#[derive(Debug, PartialEq, Eq)]
pub enum SlotKind {
    Tab {
        session: String,
        selected: bool,
    },
    /// 直前のタブのセッションを落とす。
    Close {
        session: String,
    },
    Add,
    /// セッションが 1 つも無いときの案内文。押しても何も起きない。
    Hint,
}

pub type Slot = crate::strip::Slot<SlotKind>;

fn hint(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Shell => " no shell — ctrl+t to start one ",
        _ => " no Claude Code — ctrl+n to start one ",
    }
}

/// `width` に収まるタブと、その後ろに置く `[+]` チップ。チップの幅を先に取り分ける
/// のは、名前の長いセッションが並ぶほど新しいセッションを起こす口が要るため。
/// 同じ理由で、入り切らなければ `[x]` の方を落とす。
pub fn row(
    kind: SessionKind,
    sessions: &[(&str, &str)],
    current: Option<&str>,
    width: u16,
) -> Vec<Slot> {
    let add_w = width_of(ADD);
    let mut slots = Vec::new();

    if sessions.is_empty() {
        push(&mut slots, hint(kind).into(), SlotKind::Hint);
    } else {
        let selected = sessions
            .iter()
            .position(|(id, _)| Some(*id) == current)
            .unwrap_or(0);
        let labels: Vec<String> = sessions
            .iter()
            .map(|(_, label)| format!(" {label} "))
            .collect();
        let close_w = width_of(CLOSE);
        let widths: Vec<u16> = labels.iter().map(|l| width_of(l) + close_w).collect();
        let (start, end) =
            visible_window(&widths, 0, width.saturating_sub(add_w), 0, selected, true);
        for i in start..end {
            push(
                &mut slots,
                labels[i].clone(),
                SlotKind::Tab {
                    session: sessions[i].0.into(),
                    selected: i == selected,
                },
            );
            if slots.last().map_or(0, |s| s.end) + close_w + add_w <= width {
                push(
                    &mut slots,
                    CLOSE.into(),
                    SlotKind::Close {
                        session: sessions[i].0.into(),
                    },
                );
            }
        }
    }

    if slots.last().map_or(0, |s| s.end) + add_w <= width {
        push(&mut slots, ADD.into(), SlotKind::Add);
    }
    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(slots: &[Slot], x: u16) -> Option<&SlotKind> {
        slots.iter().find(|s| s.contains(x)).map(|s| &s.kind)
    }

    #[test]
    fn タブの後ろに閉じる区画が並びチップは最後に来る() {
        let sessions = [("a", "CC:1"), ("b", "CC:2")];
        let slots = row(SessionKind::ClaudeCode, &sessions, Some("b"), 40);

        assert_eq!(
            slots.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(),
            [(0, 6), (6, 10), (10, 16), (16, 20), (20, 23)]
        );
        assert_eq!(
            hit(&slots, 0),
            Some(&SlotKind::Tab {
                session: "a".into(),
                selected: false
            })
        );
        assert_eq!(
            hit(&slots, 7),
            Some(&SlotKind::Close {
                session: "a".into()
            }),
            "閉じるのは直前のタブのセッション"
        );
        assert_eq!(
            hit(&slots, 12),
            Some(&SlotKind::Tab {
                session: "b".into(),
                selected: true
            })
        );
        assert_eq!(
            hit(&slots, 18),
            Some(&SlotKind::Close {
                session: "b".into()
            })
        );
        assert_eq!(hit(&slots, 21), Some(&SlotKind::Add));
        assert_eq!(hit(&slots, 23), None, "チップの外は何も指さない");
    }

    #[test]
    fn 幅が足りなければ選んでいるタブを見せチップは残す() {
        let sessions = [("a", "CC:1"), ("b", "CC:2"), ("c", "CC:3")];
        let slots = row(SessionKind::ClaudeCode, &sessions, Some("c"), 16);

        let labels: Vec<&str> = slots.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, [" CC:3 ", CLOSE, ADD]);
        assert_eq!(hit(&slots, 12), Some(&SlotKind::Add));
    }

    /// 削れる順は [x] → + の順。新しいセッションを起こす口が先に消えては困る。
    #[test]
    fn 閉じる区画が入らなければそれだけ落としてチップを残す() {
        let sessions = [("a", "CC:1"), ("b", "CC:2"), ("c", "CC:3")];
        let slots = row(SessionKind::ClaudeCode, &sessions, Some("c"), 12);

        let labels: Vec<&str> = slots.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, [" CC:3 ", ADD]);
    }

    #[test]
    fn セッションが無ければ案内文とチップが並ぶ() {
        let slots = row(SessionKind::Shell, &[], None, 60);
        assert!(matches!(slots[0].kind, SlotKind::Hint));
        assert!(slots[0].label.contains("ctrl+t"));
        assert_eq!(hit(&slots, 0), Some(&SlotKind::Hint), "案内文は押せない");
        assert_eq!(hit(&slots, slots[1].start), Some(&SlotKind::Add));
    }

    #[test]
    fn チップの幅も無ければ落とす() {
        let slots = row(SessionKind::Shell, &[("a", "SH:1")], Some("a"), 2);
        assert!(
            slots.iter().all(|s| !matches!(s.kind, SlotKind::Add)),
            "{slots:?}"
        );
    }
}
