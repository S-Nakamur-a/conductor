//! 画面に重ねる演出。何を描くか ([Kind])、どこへ出すか ([Target])、いつ始めるか
//! (呼ぶ側の [Fx::play]) を分けてある。新しい演出は [Kind] に variant を足して
//! その impl に振る舞いを書くだけでよい。
//!
//! [Kind] の名前は描くものであって用途ではない。起動時に何を出すかは
//! [crate::workspace::Workspace::new] が、索引の完了に何を出すかは [crate::index] が
//! 決めていて、終了時に同じ演出を出したければそこで play するだけでよい。
//!
//! レイアウトには触らない。各パネルが最終的な矩形へ描き切ったあとのセルを加工する
//! だけなので、列幅も PTY のサイズも動かない。幅を動かすと [crate::panels::terminal]
//! がフレームごとに PTY を resize してしまう。

mod assemble;
mod stagger;

use std::time::{Duration, Instant};

use conductor_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::layout::Region;

pub use stagger::Stagger;

const FLASH_MS: u64 = 260;
/// 不定バーが 1 周する時間。
const BAR_CYCLE_MS: f64 = 900.0;
/// 不定バーの尾の長さ。
const BAR_TAIL: u16 = 6;
/// 不定バーの 1 セルを 8 段に割るブロック。
const EIGHTH: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

/// 演出の種類。
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// 四隅から枠を組み上げ、閉じきってから中身を見せる。矩形ごとにずらして始まる。
    Assemble { stagger: Stagger },
    /// 終わりの見えない作業中。上辺を流れ続ける光で、止めるまで続く。
    Busy,
    /// 枠を一度 accent へ沸かせて元の色へ落とす。完了や切替の合図。
    Flash,
}

/// 演出を重ねる先。矩形は描く時点のレイアウトから引くので、ここでは名前で持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// アコーディオンのパネル全部。左から順。
    Panels,
    Region(Region),
    Modal,
}

impl Kind {
    /// アコーディオンの 5 枚を左から順に組み上げる。
    pub fn assemble() -> Self {
        Self::Assemble {
            stagger: Stagger::jittered(5, assemble::STAGGER_STEP, assemble::JITTER),
        }
    }

    /// 経過時間を進捗へ。None なら終わり。Busy は終わらないので位相を返す。
    fn progress(&self, elapsed: Duration) -> Option<f64> {
        match self {
            Kind::Assemble { .. } => finite(elapsed, assemble::DURATION_MS),
            Kind::Busy => Some(elapsed.as_millis() as f64 / BAR_CYCLE_MS % 1.0),
            Kind::Flash => finite(elapsed, FLASH_MS),
        }
    }

    /// 走っているあいだ他の演出を出さない。2 つの光が同時に走るとどちらも読めない。
    fn exclusive(&self) -> bool {
        matches!(self, Kind::Assemble { .. })
    }

    /// 入力があったら完成状態へ飛ばす。押した人を待たせる理由はない。
    fn skip_on_input(&self) -> bool {
        matches!(self, Kind::Assemble { .. })
    }

    fn paint(&self, buf: &mut Buffer, screen: Rect, rects: &[Rect], p: f64, theme: &Theme) {
        match self {
            Kind::Assemble { stagger } => assemble::paint(buf, screen, rects, p, stagger, theme),
            Kind::Busy => rects.iter().for_each(|r| paint_bar(buf, *r, p, theme)),
            Kind::Flash => rects.iter().for_each(|r| paint_flash(buf, *r, p, theme)),
        }
    }
}

fn finite(elapsed: Duration, total_ms: u64) -> Option<f64> {
    let p = ratio(elapsed, total_ms);
    (p < 1.0).then_some(p)
}

#[derive(Debug)]
struct Running {
    kind: Kind,
    target: Target,
    /// 最初のフレームを描く直前に入る。積んだ時点で始めると、起動では worktree・diff の
    /// 走査と端末への背景色問い合わせが挟まり、実測 2 秒後に出る最初のフレームより前に
    /// 演出が終わっていた。
    started: Option<Instant>,
}

impl Running {
    fn progress(&self) -> Option<f64> {
        let elapsed = self.started.map(|at| at.elapsed()).unwrap_or_default();
        self.kind.progress(elapsed)
    }

    fn is(&self, kind: &Kind, target: Target) -> bool {
        std::mem::discriminant(&self.kind) == std::mem::discriminant(kind) && self.target == target
    }
}

/// 走っている演出の集まり。[crate::workspace::Workspace] が持ち、描画の最後に重ねる。
#[derive(Debug, Default)]
pub struct Fx {
    running: Vec<Running>,
}

impl Fx {
    pub fn play(&mut self, kind: Kind, target: Target) {
        self.running.push(Running {
            kind,
            target,
            started: None,
        });
    }

    /// 終わりのない演出を止める。
    pub fn stop(&mut self, kind: &Kind, target: Target) {
        self.running.retain(|r| !r.is(kind, target));
    }

    pub fn is_playing(&self, kind: &Kind, target: Target) -> bool {
        self.running.iter().any(|r| r.is(kind, target))
    }

    pub fn start_pending(&mut self) {
        let now = Instant::now();
        for r in &mut self.running {
            r.started.get_or_insert(now);
        }
    }

    pub fn skip(&mut self) {
        self.running.retain(|r| !r.kind.skip_on_input());
    }

    pub fn is_animating(&self) -> bool {
        self.running.iter().any(|r| r.progress().is_some())
    }

    /// 終わった演出を落とす。落とした直後の 1 フレームは素の画面を描き直したいので、
    /// 何か落としたときも true。
    pub fn tick(&mut self) -> bool {
        let before = self.running.len();
        self.running.retain(|r| r.progress().is_some());
        before != self.running.len() || !self.running.is_empty()
    }

    /// `resolve` は行き先を今のレイアウトの矩形に直す。
    pub fn apply(
        &self,
        buf: &mut Buffer,
        screen: Rect,
        theme: &Theme,
        resolve: impl Fn(Target) -> Vec<Rect>,
    ) {
        let live: Vec<(&Running, f64)> = self
            .running
            .iter()
            .filter_map(|r| r.progress().map(|p| (r, p)))
            .collect();
        let exclusive = live.iter().find(|(r, _)| r.kind.exclusive());
        let shown = exclusive.map(std::slice::from_ref).unwrap_or(&live);
        for (r, p) in shown {
            r.kind.paint(buf, screen, &resolve(r.target), *p, theme);
        }
    }
}

fn ratio(elapsed: Duration, total_ms: u64) -> f64 {
    if total_ms == 0 {
        return 1.0;
    }
    (elapsed.as_millis() as f64 / total_ms as f64).clamp(0.0, 1.0)
}

/// 両端で傾きが 0 になる smoothstep。
fn ease(p: f64) -> f64 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

fn on_edge(r: Rect, x: u16, y: u16) -> bool {
    x == r.x || x == r.right().saturating_sub(1) || y == r.y || y == r.bottom().saturating_sub(1)
}

/// 進行度が取れないので割合ではなく「流れ続ける光」で表す。埋め尽くすと残り時間として
/// 読まれるので、先端から数セルだけを尾として残す。
fn paint_bar(buf: &mut Buffer, panel: Rect, phase: f64, theme: &Theme) {
    if panel.width < 4 {
        return;
    }
    let span = panel.width.saturating_sub(2);
    let head = phase.rem_euclid(1.0) * f64::from(span);
    let lit = head.floor() as u16;
    let frac = ((head - head.floor()) * 8.0) as usize;
    for i in 0..span {
        let Some(cell) = buf.cell_mut((panel.x + 1 + i, panel.y)) else {
            continue;
        };
        let behind = lit.saturating_sub(i);
        if i == lit {
            if frac > 0 {
                cell.set_symbol(EIGHTH[frac]);
                cell.set_fg(theme.fg);
            }
        } else if i < lit && behind <= BAR_TAIL {
            cell.set_symbol("█");
            cell.set_fg(Theme::lerp(
                theme.accent,
                theme.border_unfocused,
                f64::from(behind) / f64::from(BAR_TAIL),
            ));
        }
    }
}

/// 戻り先は描かれている色そのもの。フォーカスの有無で枠の色が違っても、終わりに
/// 色が跳ねない。
fn paint_flash(buf: &mut Buffer, panel: Rect, progress: f64, theme: &Theme) {
    if progress >= 1.0 {
        return;
    }
    let flash = (1.0 - progress / 0.35).max(0.0);
    let settle = ease(((progress - 0.3) / 0.7).max(0.0));
    let peak = Theme::lerp(theme.accent, theme.fg, flash);
    for y in panel.y..panel.bottom() {
        for x in panel.x..panel.right() {
            if on_edge(panel, x, y)
                && let Some(cell) = buf.cell_mut((x, y))
            {
                cell.set_fg(Theme::lerp(peak, cell.fg, settle));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    pub(super) fn theme() -> Theme {
        Theme::from_name("dracula")
    }

    /// 前景色まで入れて埋める。Color::Reset のままだと lighten も lerp も素通しになり、
    /// 色を動かす側のテストが素通りしてしまう。
    pub(super) fn filled(area: Rect) -> Buffer {
        let mut buf = Buffer::empty(area);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let cell = buf.cell_mut((x, y)).unwrap();
                cell.set_symbol("x");
                cell.set_fg(theme().fg);
            }
        }
        buf
    }

    pub(super) const AREA: Rect = Rect::new(0, 0, 58, 15);
    const VIEWER: Target = Target::Region(Region::Viewer);

    fn panels() -> Vec<Rect> {
        vec![
            Rect::new(0, 3, 17, 11),
            Rect::new(17, 3, 24, 11),
            Rect::new(41, 3, 16, 8),
        ]
    }

    fn resolve(target: Target) -> Vec<Rect> {
        match target {
            Target::Panels => panels(),
            Target::Region(Region::Viewer) => vec![Rect::new(17, 3, 24, 11)],
            _ => Vec::new(),
        }
    }

    fn lit(buf: &Buffer) -> usize {
        let bar = Rect::new(17, 3, 24, 11);
        (1..bar.width - 1)
            .filter(|i| buf.cell((bar.x + i, bar.y)).unwrap().symbol() == "█")
            .count()
    }

    #[test]
    fn 時計は最初のフレームまで動かない() {
        let mut fx = Fx::default();
        fx.play(Kind::assemble(), Target::Panels);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(fx.running[0].progress(), Some(0.0), "描く前に進んでいる");

        fx.start_pending();
        std::thread::sleep(Duration::from_millis(30));
        assert!(
            fx.running[0].progress().unwrap() > 0.0,
            "描き始めても進まない"
        );
    }

    /// 合図まで消すと、打ちながら待つ人に完了が伝わらない。
    #[test]
    fn 入力で飛ぶのは起動演出だけ() {
        let mut fx = Fx::default();
        fx.play(Kind::assemble(), Target::Panels);
        fx.play(Kind::Flash, VIEWER);
        assert!(fx.is_animating());
        fx.skip();
        assert!(!fx.is_playing(&Kind::assemble(), Target::Panels));
        assert!(fx.is_playing(&Kind::Flash, VIEWER));
    }

    #[test]
    fn 終わった演出は落ちて素の画面を一度描かせる() {
        let mut fx = Fx::default();
        fx.play(Kind::Flash, VIEWER);
        fx.running[0].started = Some(Instant::now() - Duration::from_millis(FLASH_MS + 10));
        assert!(!fx.is_animating());
        assert!(fx.tick(), "落とした直後に描き直しを求めない");
        assert!(!fx.tick());
    }

    /// 不定バーは進捗ではないので、どの時点でも辺を埋め尽くさない。埋まって見えると
    /// 「あと少し」と読まれてしまう。
    #[test]
    fn 不定バーは辺を埋め尽くさない() {
        let panel = Rect::new(17, 3, 24, 11);
        for step in 0..20 {
            let phase = f64::from(step) / 20.0;
            let mut buf = filled(AREA);
            paint_bar(&mut buf, panel, phase, &theme());
            assert!(
                lit(&buf) <= usize::from(BAR_TAIL) + 1,
                "phase={phase} で {} セルが点灯した",
                lit(&buf)
            );
        }
    }

    #[test]
    fn 沸きは枠だけに触る() {
        let panel = Rect::new(17, 3, 24, 11);
        let mut buf = filled(AREA);
        let before = buf.cell((25, 8)).unwrap().style();
        paint_flash(&mut buf, panel, 0.5, &theme());
        assert_eq!(buf.cell((25, 8)).unwrap().style(), before, "内側が変わった");
        assert_ne!(
            buf.cell((17, 3)).unwrap().style(),
            Style::default(),
            "枠が変わっていない"
        );
    }

    #[test]
    fn 起動演出中は他の演出を重ねない() {
        let mut fx = Fx::default();
        fx.play(Kind::assemble(), Target::Panels);
        fx.play(Kind::Busy, VIEWER);
        fx.start_pending();
        fx.running[1].started = Some(Instant::now() - Duration::from_millis(300));

        let mut buf = filled(AREA);
        fx.apply(&mut buf, AREA, &theme(), resolve);
        assert_eq!(lit(&buf), 0, "起動演出の上にバーが乗っている");

        fx.skip();
        let mut buf = filled(AREA);
        fx.apply(&mut buf, AREA, &theme(), resolve);
        assert!(lit(&buf) > 0, "演出が終わってもバーが出ない");
    }
}
