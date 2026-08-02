//! 画面下部のステータスバー: 一時的なフラッシュメッセージ、
//! アイドル時はキーマップから動的に導出したフォーカス中パネルの
//! キーバインドヒントを表示する。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

/// 画面下部にステータスバーを描画する。
pub fn render_status_bar(frame: &mut Frame, area: Rect, app: &crate::app::App) {
    use crate::app::StatusLevel;

    let theme = &app.theme;

    if let Some(ref msg) = app.status_message {
        let age = app.ui_tick.wrapping_sub(msg.created_at_tick);

        // レベルに応じた色。
        let fg_color = match msg.level {
            StatusLevel::Success => theme.success,
            StatusLevel::Error => theme.error,
            StatusLevel::Warning => theme.warning,
            StatusLevel::Info => theme.info,
        };

        // 最初の約500ms（30 tick）は背景をフラッシュさせる。
        let bg_color = if age < 30 {
            if (age / 5) % 2 == 0 {
                match msg.level {
                    StatusLevel::Success => theme.status_bg_success,
                    StatusLevel::Error => theme.status_bg_error,
                    StatusLevel::Warning => theme.status_bg_warning,
                    StatusLevel::Info => theme.status_bg_info,
                }
            } else {
                Color::Reset
            }
        } else {
            Color::Reset
        };

        // フェード: 2.5秒（150 tick）経過後は薄暗いスタイルにする。
        let style = if age >= 150 {
            Style::default().fg(theme.muted).bg(Color::Reset)
        } else {
            let mut s = Style::default().fg(fg_color).bg(bg_color);
            if age < 30 {
                s = s.add_modifier(Modifier::BOLD);
            }
            s
        };

        let display_text = format!("{}{}", msg.icon(), msg.text);
        let span = Span::styled(display_text, style);
        frame.render_widget(Paragraph::new(span), area);
    } else {
        // デフォルトのキーバインドヒントテキスト。実際のバインディング
        // （ユーザによる上書きを含む）から常にずれないよう、キーマップから
        // その都度動的に導出する。
        let hint = status_bar_hint(app.focus, &app.keymap);
        let span = Span::styled(hint, Style::default().fg(theme.hint));
        frame.render_widget(Paragraph::new(span), area);
    }
}

/// フォーカス中のパネルのフッター用キーバインドヒントを、現在のキーマップから組み立てる。
///
/// 各パネルは (ラベル, アクション) の順序付きリストを持ち、各エントリごとに
/// アクション1つにつき代表的なキーコードを1つ表示する（/ で連結、例: j/k: nav）。
/// アクションがすべて未割り当てのエントリは除外されるので、ヒントが何も起きない
/// キーを案内することはない。
pub(super) fn status_bar_hint(focus: crate::app::Focus, keymap: &crate::keymap::KeyMap) -> String {
    use crate::app::Focus;
    use crate::keymap::Action;

    // (ラベル, 代表的なキーコードを '/' で連結して表示するアクション群)。
    let entries: &[(&str, &[Action])] = match focus {
        Focus::Worktree => &[
            ("nav", &[Action::NavigateDown, Action::NavigateUp]),
            ("panel", &[Action::CycleFocusForward]),
            ("open", &[Action::Select]),
            ("new", &[Action::CreateWorktree]),
            ("switch", &[Action::SwitchBranch]),
            ("grab", &[Action::GrabBranch]),
        ],
        Focus::Explorer => &[
            ("nav", &[Action::NavigateDown, Action::NavigateUp]),
            ("panel", &[Action::CycleFocusForward]),
            ("open", &[Action::Select]),
            ("fold", &[Action::CollapseOrLeft, Action::ExpandOrRight]),
            ("diff", &[Action::ShowDiffList]),
            ("search", &[Action::SearchFilename]),
        ],
        Focus::Viewer => &[
            ("scroll", &[Action::NavigateDown, Action::NavigateUp]),
            ("panel", &[Action::CycleFocusForward]),
            ("search", &[Action::SearchInFile]),
            ("thread", &[Action::ToggleInlineThread]),
            ("back", &[Action::ExitToExplorer]),
        ],
        Focus::TerminalClaude => &[
            ("leave", &[Action::LeaveTerminal]),
            ("panel", &[Action::CycleFocusForward]),
            ("new CC", &[Action::NewClaudeCode]),
            ("session", &[Action::NextSession]),
        ],
        Focus::TerminalShell => &[
            ("leave", &[Action::LeaveTerminal]),
            ("panel", &[Action::CycleFocusForward]),
            ("new shell", &[Action::NewShell]),
            ("session", &[Action::NextSession]),
        ],
        Focus::Editor => &[
            ("Claude", &[Action::LeaveTerminal]),
            ("zoom", &[Action::TogglePanelExpand]),
            ("panel", &[Action::CycleFocusForward]),
        ],
    };

    let context = focus.key_context();
    let mut parts: Vec<String> = Vec::new();
    for (label, actions) in entries {
        let chords: Vec<String> = actions
            .iter()
            .filter_map(|a| representative_chord(keymap, context, *a))
            .collect();
        if !chords.is_empty() {
            parts.push(format!("{}: {label}", chords.join("/")));
        }
    }

    // コマンドパレットとチートシートは常に案内する — これらは他のすべての
    // アクションへの入口であり、どのコンテキストのフッターにも含めるべきもの
    // （パレットはPTY越しでも発火するが、? は実際に発火する場所、つまり
    // ターミナル/エディタ以外でのみ表示する）。
    if let Some(c) = representative_chord(keymap, context, Action::CommandPalette) {
        parts.push(format!("{c}: cmds"));
    }
    if let Some(c) = representative_chord(keymap, context, Action::ShowHelp) {
        parts.push(format!("{c}: keys"));
    }

    // ターミナルはそれ以外のキーをすべてPTYへ転送する — その旨を案内しておく。
    if matches!(focus, Focus::TerminalClaude | Focus::TerminalShell) {
        parts.push("keys → terminal".to_string());
    }

    parts.join(" | ")
}

/// コンテキスト内のアクションに対してユーザに見せるのに最も適したキーコード1つ:
/// 最短の ASCII のみのもの。macOS の Option グリフのフォールバック（¬, ˙, …）や
/// その他の非ASCIIキーコードもキーマップを往復はするが画面上では意味をなさないため、
/// 素のキーコードが存在する限りそちらを優先する。
pub(crate) fn representative_chord(
    keymap: &crate::keymap::KeyMap,
    context: crate::keymap::KeyContext,
    action: crate::keymap::Action,
) -> Option<String> {
    keymap
        .keys_for_action(context, action)
        .into_iter()
        .filter(|c| c.is_ascii())
        .min_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
}
