//! パネル共通の枠 (タイトル・ボーダー・右上の展開ボタン)。
//!
//! Explorer / Viewer / Terminal / Worktree の各パネルは同じ体裁の枠を持つ:
//! 左上にタイトル、フォーカス中は太いボーダー、右上に [<=>] の最大化トグル。
//! これを各ファイルで手書きしていたので、フォーカス時の字形や展開ボタンの
//! 表記を変えるたびに全パネルを追いかける必要があった。

use std::borrow::Cow;

use ratatui::layout::Alignment;
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

use crate::theme::Theme;

/// 最大化されていないパネルの展開ボタン。
const EXPAND_LABEL: &str = "[<=>]";
/// 最大化中のパネルの復帰ボタン。
const COLLAPSE_LABEL: &str = "[>=<]";

/// パネル枠の組み立て。[Self::into_block] で ratatui の [Block] にする。
///
/// 追加の装飾 (title_bottom など) が要るパネルは、得られた Block に
/// そのまま生やせばよい — ここで面倒を見るのは全パネルに共通する部分だけ。
pub struct PanelChrome<'a> {
    theme: &'a Theme,
    title: Cow<'a, str>,
    title_style: Option<Style>,
    focused: bool,
    border_color: Color,
    /// Some(最大化中か) なら右上に展開ボタンを出す。None なら出さない。
    expanded: Option<bool>,
}

impl<'a> PanelChrome<'a> {
    pub fn new(
        theme: &'a Theme,
        title: impl Into<Cow<'a, str>>,
        focused: bool,
        border_color: Color,
    ) -> Self {
        Self {
            theme,
            title: title.into(),
            title_style: None,
            focused,
            border_color,
            expanded: None,
        }
    }

    /// 右上に最大化トグルを出す。expanded は「いま最大化されているか」。
    pub fn with_expand_button(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// タイトルの配色を上書きする (grab 中の警告色など)。
    /// 指定しなければ、フォーカス中は太字の前景色、非フォーカスは muted。
    pub fn with_title_style(mut self, style: Style) -> Self {
        self.title_style = Some(style);
        self
    }

    pub fn into_block(self) -> Block<'a> {
        let title_style = self.title_style.unwrap_or(if self.focused {
            Style::default()
                .fg(self.theme.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.theme.muted)
        });

        let mut block = Block::default()
            .title(Span::styled(self.title, title_style))
            .borders(Borders::ALL)
            .border_type(if self.focused {
                BorderType::Thick
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(self.border_color));

        if let Some(expanded) = self.expanded {
            let (label, color) = if expanded {
                (COLLAPSE_LABEL, self.theme.border_focused)
            } else {
                (EXPAND_LABEL, self.theme.border_unfocused)
            };
            block = block.title_top(
                Line::from(Span::styled(label, Style::default().fg(color)))
                    .alignment(Alignment::Right),
            );
        }
        block
    }
}
