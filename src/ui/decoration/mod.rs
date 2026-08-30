//! ワークツリーパネルの空きスペースに表示する装飾の描画。
//!
//! aquarium・space・garden・city の複数のアニメーションモードをサポートする。
//! モードを足すときは、状態構造体と tick_* / render_* を1サブモジュールに
//! まとめて置く。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme::Theme;

mod aquarium;
mod city;
mod garden;
mod space;

#[cfg(test)]
mod tests;

// これらのモード別の型は DecorationStates::aquarium.fish のように *State
// 構造体のフィールド経由でのみ到達し、このモジュール外から直接名指しされることは
// ない。ファイル分割前、このファイルに直接 pub 構造体定義があった頃と同じ扱い。
#[allow(unused_imports)]
pub use aquarium::{AquariumState, Bubble, Fish};
#[allow(unused_imports)]
pub use city::{Building, Car, CityState, SkyObject};
#[allow(unused_imports)]
pub use garden::{Bird, Butterfly, GardenPlant, GardenState};
#[allow(unused_imports)]
pub use space::{Planet, ShootingStar, SpaceState, Star};

use aquarium::{render_aquarium, tick_aquarium};
use city::{render_city, tick_city};
use garden::{render_garden, tick_garden};
use space::{render_space, tick_space};

// 共通の型

/// config から解釈した装飾モード。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationMode {
    Aquarium,
    Space,
    Garden,
    City,
    None,
}

impl DecorationMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "none" => Self::None,
            "space" => Self::Space,
            "garden" => Self::Garden,
            "city" => Self::City,
            _ => Self::Aquarium,
        }
    }

    /// このモードが定期的な tick を必要とするアニメーションを実行するなら true を返す。
    pub fn has_animation(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// 全モード共通で、アニメーションの活発さに影響する activity レベル。
///
/// Active — Claude Code がユーザー入力待ちの状態（動きが活発になる）。
/// Calm   — Claude Code が作業中の状態（動きが落ち着く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationActivity {
    Calm,
    Active,
}

/// 全ての装飾モードの状態を保持するコンテナ。
///
/// 実際に使われるのは実行中のモードに対応する状態だけであり、
/// それ以外はユーザーがモードを切り替えるまでデフォルト（未初期化）値のままとなる。
#[derive(Debug, Clone, Default)]
pub struct DecorationStates {
    pub aquarium: AquariumState,
    pub space: SpaceState,
    pub garden: GardenState,
    pub city: CityState,
}

/// 実行中の装飾を1ティック進める。
pub fn tick_decoration(
    states: &mut DecorationStates,
    tick: u64,
    width: u16,
    height: u16,
    activity: DecorationActivity,
    mode: DecorationMode,
) {
    match mode {
        DecorationMode::Aquarium => {
            tick_aquarium(&mut states.aquarium, tick, width, height, activity);
        }
        DecorationMode::Space => {
            tick_space(&mut states.space, tick, width, height, activity);
        }
        DecorationMode::Garden => {
            tick_garden(&mut states.garden, tick, width, height, activity);
        }
        DecorationMode::City => {
            tick_city(&mut states.city, tick, width, height, activity);
        }
        DecorationMode::None => {}
    }
}

/// config のモードに基づき装飾の描画をディスパッチする。
pub fn render_decoration(
    frame: &mut Frame,
    area: Rect,
    states: &DecorationStates,
    theme: &Theme,
    mode: DecorationMode,
) {
    match mode {
        DecorationMode::Aquarium => {
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border_unfocused));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            render_aquarium(frame, inner, &states.aquarium, theme);
        }
        DecorationMode::Space => {
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border_unfocused));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            render_space(frame, inner, &states.space, theme);
        }
        DecorationMode::Garden => {
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border_unfocused));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            render_garden(frame, inner, &states.garden, theme);
        }
        DecorationMode::City => {
            let block = Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.border_unfocused));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            render_city(frame, inner, &states.city, theme);
        }
        DecorationMode::None => {}
    }
}

/// 絵文字セルの行優先グリッドを組み立て、[Paragraph] として描画する。
///
/// このヘルパーは全モードで共有される。各グリッドセルは None（スペースとして
/// 描画）か Some(emoji)（幅2カラムのスタイル付き span として描画）のいずれか。
fn render_grid(frame: &mut Frame, area: Rect, grid: &[Vec<Option<&str>>], theme: &Theme) {
    let lines: Vec<Line> = grid
        .iter()
        .map(|row| {
            let mut spans: Vec<Span> = Vec::new();
            let mut col = 0;
            while col < row.len() {
                if let Some(emoji) = row[col] {
                    spans.push(Span::styled(emoji, Style::default().fg(theme.fg)));
                    col += 2; // 絵文字は幅2セル分
                } else {
                    spans.push(Span::raw(" "));
                    col += 1;
                }
            }
            Line::from(spans)
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// tick カウンタから導出する単純な疑似乱数。
///
/// 暗号学的な安全性はない — アニメーションに変化をつけられれば十分という位置づけ。
fn pseudo_random(tick: u64, seed: u64) -> u64 {
    let mut x = tick.wrapping_mul(6364136223846793005).wrapping_add(seed);
    x ^= x >> 16;
    x = x.wrapping_mul(2246822519);
    x ^= x >> 13;
    x
}
