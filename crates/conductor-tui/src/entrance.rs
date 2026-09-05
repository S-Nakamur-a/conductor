//! 起動と索引の演出。描き終わったバッファを進捗で伏せる。
//!
//! レイアウトには触らない。各パネルが最終的な矩形へ描き切ったあとのセルを加工する
//! だけなので、列幅も PTY のサイズも動かない。幅を動かすと [crate::panels::terminal]
//! がフレームごとに PTY を resize してしまう。

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use conductor_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// ゆらぎを作るパネル数。
const PANELS: usize = 5;

const ENTRANCE_MS: u64 = 640;
const INDEX_DONE_MS: u64 = 260;
/// 不定バーが 1 周する時間。
const BAR_CYCLE_MS: f64 = 900.0;
/// パネルごとの開始のずらしと、そこに乗せるゆらぎの幅。
const PANEL_STAGGER: f64 = 0.08;
const JITTER_MS: i64 = 40;

/// 四隅を置いてから辺が伸び始めるまでの割合。
const HOLD_RATIO: f64 = 0.25;
/// 伸びる先端から何セルぶん明るさを引きずるか。
const EDGE_TRAIL: u16 = 10;
/// 枠が閉じたあとの持ち上げの山の高さ。
const GLOW_PEAK: f64 = 0.40;
/// 1 枚のパネルが持ち時間のうち枠を描くのに使う割合。
const FRAME_SHARE: f64 = 0.55;
/// 不定バーの尾の長さ。
const BAR_TAIL: u16 = 6;
/// 不定バーの 1 セルを 8 段に割るブロック。
const EIGHTH: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

#[derive(Debug, Default, PartialEq, Eq)]
enum Boot {
    /// 設定で切られたか、済んだか、飛ばされたか。
    #[default]
    Off,
    /// 出す気はあるが、まだ 1 枚も描いていない。
    Pending,
    Running(Instant),
}

#[derive(Debug, Default)]
pub struct Entrance {
    boot: Boot,
    /// パネルごとの開始時点 (進捗の割合)。
    offsets: Vec<f64>,
    index_since: Option<Instant>,
    index_done: Option<Instant>,
}

impl Entrance {
    pub fn new(enabled: bool) -> Self {
        Self {
            boot: if enabled { Boot::Pending } else { Boot::Off },
            offsets: offsets(&jitter()),
            ..Default::default()
        }
    }

    /// 最初のフレームを描く直前に時計を始める。
    ///
    /// 起動時に始めると、そこから worktree・diff の走査と端末への背景色問い合わせが
    /// 挟まる。実測でフレームが出るのは 2 秒後で、演出は画面に何か出るより前に
    /// 終わっていた。
    pub fn start_if_pending(&mut self) {
        if self.boot == Boot::Pending {
            self.boot = Boot::Running(Instant::now());
        }
    }

    /// 入力があったら完成状態へ飛ばす。押した人を待たせる理由はない。
    pub fn skip(&mut self) {
        self.boot = Boot::Off;
    }

    /// 索引が作られているかを毎フレーム伝える。開始と完了の縁でだけ状態が動く。
    pub fn note_index_building(&mut self, building: bool) {
        match (self.index_since.is_some(), building) {
            (false, true) => self.index_since = Some(Instant::now()),
            (true, false) => {
                self.index_since = None;
                self.index_done = Some(Instant::now());
            }
            _ => {}
        }
    }

    /// 時間で動いているものがあるか。メインループはこれを見てフレームを流し続ける。
    pub fn is_animating(&self) -> bool {
        self.boot_progress().is_some()
            || self.index_since.is_some()
            || self.index_done_progress().is_some()
    }

    /// 起動演出の進捗。終わっていれば None。
    fn boot_progress(&self) -> Option<f64> {
        match self.boot {
            Boot::Off => None,
            Boot::Pending => Some(0.0),
            Boot::Running(at) => {
                let p = ratio(at.elapsed(), ENTRANCE_MS);
                (p < 1.0).then_some(p)
            }
        }
    }

    /// 索引の実行中を示す不定バーの位相。進捗ではないので割合ではなく循環値。
    fn index_bar_phase(&self) -> Option<f64> {
        let since = self.index_since?;
        Some(since.elapsed().as_millis() as f64 / BAR_CYCLE_MS % 1.0)
    }

    fn index_done_progress(&self) -> Option<f64> {
        let at = self.index_done?;
        let p = ratio(at.elapsed(), INDEX_DONE_MS);
        (p < 1.0).then_some(p)
    }
}

/// 全パネルが描き終わったバッファを加工する。描画の最後に呼ぶ。
///
/// `panels` は左から順のパネルの矩形、`bar` は索引の合図を出す 1 枚。
pub fn apply(
    entrance: &Entrance,
    buf: &mut Buffer,
    area: Rect,
    panels: &[Rect],
    bar: Option<Rect>,
    theme: &Theme,
) {
    if let Some(p) = entrance.boot_progress() {
        apply_entrance(buf, area, panels, p, &entrance.offsets, theme);
        // 起動演出のあいだは索引の合図を重ねない。2 つの光が同時に走るとどちらも読めない。
        return;
    }
    let Some(bar) = bar else { return };
    if let Some(phase) = entrance.index_bar_phase() {
        apply_index_bar(buf, bar, phase, theme);
    }
    if let Some(p) = entrance.index_done_progress() {
        apply_index_done(buf, bar, p, theme);
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

/// パネルごとの開始のゆらぎ。順序は [offsets] が保つので、ここは幅だけ決める。
fn jitter() -> Vec<f64> {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let span = JITTER_MS as f64 / ENTRANCE_MS as f64;
    (0..PANELS as u32)
        // 起動ごとに違えばよいだけなので、質のよい乱数は要らない。
        .map(|i| (f64::from(seed.rotate_left(i * 8) % 1000) / 1000.0 * 2.0 - 1.0) * span)
        .collect()
}

/// パネルごとの開始時点。ずらしにゆらぎを乗せたうえで、前のパネルより早く始まらない
/// よう押し出す。ゆらぎ幅がずらしより広いと素朴な加算では順序が入れ替わる。
fn offsets(jitter: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(jitter.len());
    let mut prev = f64::NEG_INFINITY;
    for (i, j) in jitter.iter().enumerate() {
        let v = (PANEL_STAGGER * i as f64 + j).max(prev);
        out.push(v);
        prev = v;
    }
    out
}

fn apply_entrance(
    buf: &mut Buffer,
    area: Rect,
    panels: &[Rect],
    progress: f64,
    offsets: &[f64],
    theme: &Theme,
) {
    if progress >= 1.0 {
        return;
    }
    let closed = frames_closed_at(offsets);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            match locate(panels, x, y) {
                Part::Edge(i) => {
                    let p = staggered(progress, i, offsets) / FRAME_SHARE;
                    paint_edge(buf, panels[i], x, y, p.min(1.0), progress, theme);
                }
                // 枠が閉じきるまで中身は伏せる。まだ辺が伸びている横で本文が読めると、
                // 枠が飾りに見える。
                Part::Inside | Part::Chrome => {
                    if progress < closed
                        && let Some(cell) = buf.cell_mut((x, y))
                    {
                        cell.reset();
                    }
                }
            }
        }
    }
    apply_glow(buf, area, progress, closed, theme);
}

/// 最後のパネルの枠が閉じきる時点。ずらしが大きいほど後ろへ動くので offsets から導く。
/// 固定値にすると、ずらしを増やしたときに中身が枠より先に出る。
fn frames_closed_at(offsets: &[f64]) -> f64 {
    let last = offsets.last().copied().unwrap_or(0.0);
    last + FRAME_SHARE * (1.0 - last)
}

/// 枠が閉じたあと、画面全体をひと呼吸だけ持ち上げて「もう使える」を示す。
///
/// 前景しか触らないのは、端末の既定背景が [Color::Reset] で [Theme::lerp] が
/// 効かないため。
fn apply_glow(buf: &mut Buffer, area: Rect, progress: f64, start: f64, theme: &Theme) {
    let g = (progress - start) / (1.0 - start);
    if g <= 0.0 {
        return;
    }
    let k = (g.clamp(0.0, 1.0) * std::f64::consts::PI).sin() * GLOW_PEAK;
    if k <= 0.0 {
        return;
    }
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_fg(raise(theme, cell.fg, k));
            }
        }
    }
}

/// 色を「目立つ側」へ寄せる。どちらも白へ寄せると、ライトテーマでは文字が地に溶ける。
fn raise(theme: &Theme, color: Color, k: f64) -> Color {
    if theme.light {
        Theme::lerp(color, Color::Rgb(0, 0, 0), k)
    } else {
        Theme::lighten(color, k)
    }
}

/// 索引の実行中を示す不定バー。
///
/// 進行度が取れないので割合ではなく「流れ続ける光」で表す。埋め尽くすと残り時間として
/// 読まれるので、先端から数セルだけを尾として残す。
fn apply_index_bar(buf: &mut Buffer, panel: Rect, phase: f64, theme: &Theme) {
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

/// 索引の完了。枠を一度 accent へ沸かせてから通常のボーダー色へ落とす。
fn apply_index_done(buf: &mut Buffer, panel: Rect, progress: f64, theme: &Theme) {
    if progress >= 1.0 {
        return;
    }
    let flash = (1.0 - progress / 0.35).max(0.0);
    let settle = ease(((progress - 0.3) / 0.7).max(0.0));
    let peak = Theme::lerp(theme.accent, theme.fg, flash);
    let color = Theme::lerp(peak, theme.border_unfocused, settle);
    for y in panel.y..panel.bottom() {
        for x in panel.x..panel.right() {
            if on_edge(panel, x, y)
                && let Some(cell) = buf.cell_mut((x, y))
            {
                cell.set_fg(color);
            }
        }
    }
}

enum Part {
    /// panels[i] の枠線上。
    Edge(usize),
    Inside,
    /// どのパネルにも属さない (タイトルバー、メニュー、ストリップ、ステータスバー)。
    Chrome,
}

fn locate(panels: &[Rect], x: u16, y: u16) -> Part {
    for (i, pan) in panels.iter().enumerate() {
        if !contains(*pan, x, y) {
            continue;
        }
        return if on_edge(*pan, x, y) {
            Part::Edge(i)
        } else {
            Part::Inside
        };
    }
    Part::Chrome
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

fn on_edge(r: Rect, x: u16, y: u16) -> bool {
    x == r.x || x == r.right().saturating_sub(1) || y == r.y || y == r.bottom().saturating_sub(1)
}

/// パネル i にとっての進捗。offsets のぶん遅れて始まる。
fn staggered(progress: f64, i: usize, offsets: &[f64]) -> f64 {
    let offset = offsets.get(i).copied().unwrap_or(0.0);
    let last = offsets.last().copied().unwrap_or(0.0);
    ((progress - offset) / (1.0 - last).max(0.1)).clamp(0.0, 1.0)
}

/// 枠セルの、四隅からの距離と辺の半分の長さ。
fn edge_distance(pan: Rect, x: u16, y: u16) -> (f64, f64) {
    if y == pan.y || y == pan.bottom().saturating_sub(1) {
        let d = (x - pan.x).min(pan.right().saturating_sub(1) - x);
        (f64::from(d), f64::from(pan.width) / 2.0 + 1.0)
    } else {
        let d = (y - pan.y).min(pan.bottom().saturating_sub(1) - y);
        (f64::from(d), f64::from(pan.height) / 2.0 + 1.0)
    }
}

fn is_corner(pan: Rect, x: u16, y: u16) -> bool {
    (x == pan.x || x == pan.right().saturating_sub(1))
        && (y == pan.y || y == pan.bottom().saturating_sub(1))
}

fn paint_edge(
    buf: &mut Buffer,
    pan: Rect,
    x: u16,
    y: u16,
    local: f64,
    overall: f64,
    theme: &Theme,
) {
    let Some(cell) = buf.cell_mut((x, y)) else {
        return;
    };
    // 溜めのあいだは四隅だけが先に出る。ここが起動のたびに同じ位置に落ちる。
    if is_corner(pan, x, y) {
        let pop = ease(local / HOLD_RATIO.max(0.06));
        cell.set_fg(Theme::lerp(theme.border_unfocused, theme.fg, pop));
        return;
    }
    let t = ease((local - HOLD_RATIO) / (1.0 - HOLD_RATIO));
    if t <= 0.0 {
        cell.reset();
        return;
    }
    let (dist, span) = edge_distance(pan, x, y);
    let head = t * span;
    if dist > head {
        cell.reset();
        return;
    }
    let back = (head - dist) / f64::from(EDGE_TRAIL.max(1));
    let color = if t >= 1.0 {
        Theme::lerp(
            theme.accent,
            theme.border_unfocused,
            ease((overall - 0.78) / 0.22),
        )
    } else if back < 0.14 {
        theme.fg
    } else {
        Theme::lerp(theme.accent, theme.border_unfocused, (back - 0.14) / 0.86)
    };
    cell.set_fg(color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn theme() -> Theme {
        Theme::from_name("dracula")
    }

    /// 前景色まで入れて埋める。Color::Reset のままだと lighten も lerp も素通しになり、
    /// 色を動かす側のテストが素通りしてしまう。
    fn filled(area: Rect) -> Buffer {
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

    const AREA: Rect = Rect::new(0, 0, 58, 15);

    fn panels() -> Vec<Rect> {
        vec![
            Rect::new(0, 3, 17, 11),
            Rect::new(17, 3, 24, 11),
            Rect::new(41, 3, 16, 8),
        ]
    }

    #[test]
    fn 設定で切られていれば起動演出は始まらない() {
        let e = Entrance::new(false);
        assert_eq!(e.boot_progress(), None);
        assert!(!e.is_animating());
    }

    #[test]
    fn 時計は最初のフレームまで動かない() {
        let mut e = Entrance::new(true);
        assert_eq!(e.boot_progress(), Some(0.0));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(e.boot_progress(), Some(0.0), "描く前に進んでいる");

        e.start_if_pending();
        std::thread::sleep(Duration::from_millis(30));
        assert!(e.boot_progress().unwrap() > 0.0, "描き始めても進まない");
    }

    #[test]
    fn 時計は一度始めたら打ち直さない() {
        let mut e = Entrance::new(true);
        e.start_if_pending();
        std::thread::sleep(Duration::from_millis(30));
        let p = e.boot_progress().unwrap();
        e.start_if_pending();
        assert!(e.boot_progress().unwrap() >= p, "2 枚目で先頭へ戻った");
    }

    #[test]
    fn 入力で完成状態へ飛ぶ() {
        let mut e = Entrance::new(true);
        assert!(e.is_animating());
        e.skip();
        assert_eq!(e.boot_progress(), None);
        assert!(!e.is_animating());
    }

    /// 開始と完了は縁でだけ動く。毎フレーム true を渡して開始時刻が巻き戻ると、
    /// バーがいつまでも先頭から出直してしまう。
    #[test]
    fn 索引の開始は縁でだけ動く() {
        let mut e = Entrance::new(false);
        e.note_index_building(true);
        let first = e.index_since;
        e.note_index_building(true);
        assert_eq!(e.index_since, first);
        assert!(e.is_animating());

        e.note_index_building(false);
        assert!(e.index_since.is_none());
        assert!(e.index_done_progress().is_some());
    }

    #[test]
    fn ゆらぎは指定した幅に収まる() {
        let span = JITTER_MS as f64 / ENTRANCE_MS as f64;
        let got = jitter();
        assert_eq!(got.len(), PANELS);
        for j in got {
            assert!(j.abs() <= span, "{j} が ±{span} を超えた");
        }
    }

    /// ゆらぎがずらしより広くても左から順のまま。入れ替わると「毎回違う順で開く」
    /// ことになり、固定順を選んだ意図と食い違う。
    #[test]
    fn ゆらぎが広くてもパネルの順序は入れ替わらない() {
        let wide = PANEL_STAGGER * 4.0;
        for case in [
            vec![wide, -wide, wide, -wide],
            vec![-wide, wide, -wide, wide],
            vec![0.0, 0.0, 0.0, 0.0],
        ] {
            let got = offsets(&case);
            for pair in got.windows(2) {
                assert!(
                    pair[0] <= pair[1],
                    "順序が入れ替わった: {got:?} (jitter={case:?})"
                );
            }
        }
    }

    /// 四隅は進捗によらず同じ位置に居続ける。ここが動くと枠が「組み上がる」ではなく
    /// 「泳ぐ」ように見える。
    #[test]
    fn 四隅は進捗のどの時点でも同じ位置にある() {
        let corners: Vec<(u16, u16)> = panels()
            .iter()
            .flat_map(|p| {
                [
                    (p.x, p.y),
                    (p.right() - 1, p.y),
                    (p.x, p.bottom() - 1),
                    (p.right() - 1, p.bottom() - 1),
                ]
            })
            .collect();

        for step in 0..=20 {
            let p = f64::from(step) / 20.0;
            let mut buf = filled(AREA);
            apply_entrance(&mut buf, AREA, &panels(), p, &[0.0; PANELS], &theme());
            for &(x, y) in &corners {
                assert_eq!(
                    buf.cell((x, y)).unwrap().symbol(),
                    "x",
                    "四隅 ({x},{y}) が progress={p} で消えた"
                );
            }
        }
    }

    #[test]
    fn 完了時は一切伏せない() {
        let mut buf = filled(AREA);
        apply_entrance(&mut buf, AREA, &panels(), 1.0, &[0.0; PANELS], &theme());
        for y in 0..AREA.height {
            for x in 0..AREA.width {
                assert_eq!(buf.cell((x, y)).unwrap().symbol(), "x");
            }
        }
    }

    #[test]
    fn 開始時はパネルの内側が伏せられる() {
        let mut buf = filled(AREA);
        apply_entrance(&mut buf, AREA, &panels(), 0.0, &[0.0; PANELS], &theme());
        assert_eq!(buf.cell((5, 6)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((25, 8)).unwrap().symbol(), " ");
    }

    /// 中身は最後のパネルの枠が閉じるまで出ない。固定値で判定していると、ずらしを
    /// 増やしたときに枠より先に本文が読める。
    #[test]
    fn 中身はずらしを増やしても枠の完成を待つ() {
        let wide = vec![0.0, 0.2, 0.4, 0.6];
        let closed = frames_closed_at(&wide);
        assert!(closed > FRAME_SHARE, "ずらしを増やしても待ち時間が伸びない");

        let mut buf = filled(AREA);
        apply_entrance(&mut buf, AREA, &panels(), closed - 0.05, &wide, &theme());
        assert_eq!(
            buf.cell((25, 8)).unwrap().symbol(),
            " ",
            "枠が閉じる前に中身が出ている"
        );
    }

    /// ひと呼吸なので、終わったあとに色が残ってはいけない。残ると起動のたびに画面が
    /// じわっと明るいままになる。
    #[test]
    fn ひと呼吸の光は終端で残らない() {
        let th = theme();
        let base = filled(AREA);

        let mut peak_seen = false;
        for step in 0..=20 {
            let p = f64::from(step) / 20.0;
            let mut buf = filled(AREA);
            apply_glow(&mut buf, AREA, p, 0.55, &th);
            let changed = buf.cell((25, 8)).unwrap().fg != base.cell((25, 8)).unwrap().fg;
            if (0.7..0.85).contains(&p) {
                peak_seen |= changed;
            }
            if p >= 1.0 {
                assert!(!changed, "progress={p} で色が残っている");
            }
        }
        assert!(peak_seen, "山が立っていない");
    }

    /// 不定バーは進捗ではないので、どの時点でも辺を埋め尽くさない。埋まって見えると
    /// 「あと少し」と読まれてしまう。
    #[test]
    fn 不定バーは辺を埋め尽くさない() {
        let panel = Rect::new(17, 3, 24, 11);
        for step in 0..20 {
            let phase = f64::from(step) / 20.0;
            let mut buf = filled(AREA);
            apply_index_bar(&mut buf, panel, phase, &theme());
            let lit = (1..panel.width - 1)
                .filter(|i| buf.cell((panel.x + i, panel.y)).unwrap().symbol() == "█")
                .count();
            assert!(
                lit <= usize::from(BAR_TAIL) + 1,
                "phase={phase} で {lit} セルが点灯した"
            );
        }
    }

    #[test]
    fn 索引完了は枠だけに触る() {
        let panel = Rect::new(17, 3, 24, 11);
        let mut buf = filled(AREA);
        let before = buf.cell((25, 8)).unwrap().style();
        apply_index_done(&mut buf, panel, 0.5, &theme());
        assert_eq!(buf.cell((25, 8)).unwrap().style(), before, "内側が変わった");
        assert_ne!(
            buf.cell((17, 3)).unwrap().style(),
            Style::default(),
            "枠が変わっていない"
        );
    }

    #[test]
    fn 起動演出中は索引の合図を重ねない() {
        let bar = Rect::new(17, 3, 24, 11);
        let lit = |buf: &Buffer| {
            (1..bar.width - 1)
                .filter(|i| buf.cell((bar.x + i, bar.y)).unwrap().symbol() == "█")
                .count()
        };
        let mut booting = Entrance {
            boot: Boot::Running(Instant::now()),
            offsets: offsets(&[0.0; PANELS]),
            index_since: Some(Instant::now() - Duration::from_millis(300)),
            index_done: None,
        };

        let mut buf = filled(AREA);
        apply(&booting, &mut buf, AREA, &panels(), Some(bar), &theme());
        assert_eq!(lit(&buf), 0, "起動演出の上にバーが乗っている");

        booting.skip();
        let mut buf = filled(AREA);
        apply(&booting, &mut buf, AREA, &panels(), Some(bar), &theme());
        assert!(lit(&buf) > 0, "演出が終わってもバーが出ない");
    }
}
