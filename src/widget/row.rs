//! 行ベースのリストで共有する hover/selection の状態とスタイリング。
//!
//! selection/focus/hover の優先順位ルールを
//! 各パネルで再導出させて食い違わせるのではなく、一箇所に集約するためにここに置く。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::{Duration, Instant};

/// ポインタが離れてから行がフェードアウトし続ける時間（ミリ秒）。
/// アニメーションするのは離れる側の行だけ（D2）: 行に入るときは即座に
/// ライトアップすることでポインタ追従が遅く感じないようにし、逆に離れた側の
/// 行はイーズアウトすることで行を素早く横切っても滑らかな動きに見えるようにする。
const HOVER_FADE_MS: u64 = 120;

/// 1つの行ベースリストにおける hover 状態: 現在ポインタの下にある行と、
/// 直近にホバーされていた行（離れた瞬間の時刻付き）を保持し、そのハイライトが
/// 唐突に消えるのではなくフェードアウトできるようにする。
#[derive(Debug, Default)]
pub struct HoverRow {
    row: Option<usize>,
    left: Option<(usize, Instant)>,
}

/// [HoverRow::phase] が返す、1行分の hover 状態。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HoverPhase {
    /// ポインタが現在この行の上にある。
    On,
    /// ポインタがこの行から離れた直後。f64 はハイライトの残り強度で [0.0, 1.0]
    /// （離れた直後は1.0、フェードが完了すると0.0）。
    FadingOut(f64),
}

impl HoverRow {
    /// 現在ホバーされている行を更新する。None を渡すのは、ポインタが
    /// このリストのどの行の上にもないという意味（例えば別のパネルへ
    /// 移動した場合）。
    ///
    /// 直前の行が left に記録される（フェードが始まる）のは、ホバーが
    /// 実際に *別の* 行へ移動したときだけである。マウス移動イベントのたびに
    /// 同じ行をセットし直しても（ポインタが静止している間によく起きる）
    /// フェードアニメーションをその都度リスタートしてはならない。
    pub fn set(&mut self, row: Option<usize>) {
        if self.row == row {
            return;
        }
        if let Some(prev) = self.row {
            self.left = Some((prev, Instant::now()));
        }
        self.row = row;
    }

    /// row の hover phase。ホバー中でもフェードアウト中でもなければ None。
    pub fn phase(&self, row: usize) -> Option<HoverPhase> {
        if self.row == Some(row) {
            return Some(HoverPhase::On);
        }
        if let Some((left_row, left_at)) = self.left
            && left_row == row
        {
            let remaining = 1.0 - crate::anim::eased_progress(left_at.elapsed(), HOVER_FADE_MS);
            if remaining > 0.0 {
                return Some(HoverPhase::FadingOut(remaining));
            }
        }
        None
    }

    /// このリストで現在フェードアウト中の行があるかどうか。メインループが
    /// 再描画ポンプがまだ必要かどうかを判断するのに使う
    /// （App::has_active_transition と src/anim.rs を参照）。
    pub fn is_animating(&self) -> bool {
        self.left.is_some_and(|(row, left_at)| {
            self.row != Some(row) && left_at.elapsed() < Duration::from_millis(HOVER_FADE_MS)
        })
    }

    /// テスト専用のコンストラクタ。既に経過済みの時刻で left を仕込めるので、
    /// テストスレッドをスリープさせずにフェード完了時の挙動を検証できる。
    #[cfg(test)]
    fn with_left_at(row: usize, left_at: Instant) -> Self {
        Self {
            row: None,
            left: Some((row, left_at)),
        }
    }
}

/// selection、パネルのフォーカス、hover 状態から、1つのリスト行の [Style] を組み立てる。
///
/// 優先順位は selection が hover に勝る: 選択中の行は hover の有無にかかわらず
/// 選択色を保つ。selection の方がより重要な状態であり、hover の色味で薄めてしまうと
/// ポインタでリストをなぞる間にどの行が選択されているか追いにくくなるため。
///
/// ADR D1（改訂版）に従い、この行ベースリストの hover は前景色のみで表現する
/// （背景色は使わない）。これは Viewer の行 hover（src/viewer/render/code_line.rs）
/// の既存の前例に合わせたもの。背景色で表現する方式も試したが却下した:
/// 11テーマ中7テーマで selected_bg_inactive と区別が付かなかった。これはまさに
/// hover 中だがフォーカスされていない行が置かれる状態そのものである。
pub fn row_style(
    theme: &crate::theme::Theme,
    base_fg: ratatui::style::Color,
    selected: bool,
    panel_focused: bool,
    hover: Option<HoverPhase>,
) -> Style {
    if selected {
        return if panel_focused {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme.selected_fg_inactive)
                .bg(theme.selected_bg_inactive)
                .add_modifier(Modifier::BOLD)
        };
    }

    let target = hover_emphasis(theme, base_fg);
    let fg = match hover {
        Some(HoverPhase::On) => target,
        Some(HoverPhase::FadingOut(t)) => crate::theme::Theme::lerp(base_fg, target, t),
        None => base_fg,
    };
    let style = Style::default().fg(fg);
    // hover のための2つ目の、色以外のチャンネル（D1改訂版）: 色だけでは
    // 11パレット全てと行が取り得るすべての基本色をカバーしなければならず、
    // 圧倒的に最も多い行の色である theme.fg では最も効果が弱くなる。
    // 下線はパレットに依存せず色覚特性の影響も受けず、このコードベースでは
    // 既に hover の語彙として使われている: Viewer はポインタの下にある
    // ジャンプ可能なシンボルに下線を引く（src/viewer/render/code_line.rs）。
    // また背景色 + BOLD で表現される selection と混同することもない。
    //
    // FadingOut には意図的に持ち越さない: 下線は「ポインタが *ここにある*」
    // ことを示すものであり、離れた瞬間に真ではなくなる。またモディファイアは
    // 補間できないので、どのみちフェードの途中のどこかで唐突に消えるしかない。
    // 色のフェードだけで退出をなめらかにする。
    if matches!(hover, Some(HoverPhase::On)) {
        style.add_modifier(Modifier::UNDERLINED)
    } else {
        style
    }
}

/// hover の下線を除いた同じ行スタイル: 行の名前部分以外
/// （先頭のインデント、展開矢印、アイコン、origin マーカー、行数）に使う。
///
/// 下線は「あなたが指しているのはこれだ」という印であり、その対象はファイルで
/// あって手前のツリー装飾ではない。インデントの下にも下線を引くと、
/// ネストされた行の下線がクリック可能な要素よりずっと左から始まってしまい、
/// ポインタのアフォーダンスではなく行全体に渡るテキスト入力の下線のように
/// 見えていた。色は行全体に引き続き適用されるので、行全体としては見た目に
/// hover していることが分かる。
pub fn decoration_style(style: Style) -> Style {
    style.remove_modifier(Modifier::UNDERLINED)
}

/// hover 中の行の色が、静止時の色からどれだけ離れている必要があるか。
/// 単位は [Theme::perceptual_distance]（0が同一、黒対白が約765）。
///
/// この下限が存在するのは、素朴な変換ではこれを満たせないため。行の色を
/// 白（ダークテーマ）や黒（ライトテーマ）へ寄せる方式では、色が既にその
/// 極値付近にあると余地が残っていない — そしてまさに theme.fg、圧倒的多数の
/// 行が持つ色は設計上その状態にある。以前の lighten(base, 0.45) と比較すると、
/// theme.fg の行は約53しか動かなかったのに対し theme.hint（untracked）の行は
/// 約237動いた: hover は最も頻繁に発火する箇所でちょうど4倍弱くなっており、
/// これが「控えめ」ではなく「あてにならない」と受け取られる原因になっていた。
const HOVER_MIN_DISTANCE: f64 = 120.0;

/// hover 中の色を寄せる先となり得る明度値。テーマの極性ごとに優先順で並べる。
/// 2つあるのは、片方が常に到達可能とは限らないため: 既に明るい方のターゲット
/// （catppuccin の info、L 0.73 のほぼ完全に彩度を持つシアン）に位置している
/// 行の色は、theme.fg が白へ向かって余地がないのとまったく同じように、
/// そちらへ向かう余地がない。2つ目のターゲットは反対側にあるので、
/// どちらか一方には常に余地がある。
///
/// ペアの両方の値とも、テーマの背景色から見て遠い側に置く —
/// ダークテーマでは明るい側、ライトテーマでは深い側 — ので、
/// 探索がどちらを選んでも十分に判読できる。
const HOVER_TARGET_L_DARK: [f64; 2] = [0.85, 0.52];
const HOVER_TARGET_L_LIGHT: [f64; 2] = [0.34, 0.14];

/// 下の探索が辿るステップ数。強調の粒度を決めるだけのもの。
const HOVER_STEPS: u32 = 20;

/// 固定トークンではなく行自身の色から導く。以前は theme.accent を使っていたが、
/// solarized-dark と gruvbox は accent == warning なので、未ステージの行を hover すると
/// ステージ済みと同じ色になり working tree について嘘をついていた。押し出し量も固定に
/// しないのは、固定だと変化の大きさが出発点の色に依存してしまうため。
fn hover_emphasis(
    theme: &crate::theme::Theme,
    base_fg: ratatui::style::Color,
) -> ratatui::style::Color {
    let targets = if theme.light {
        HOVER_TARGET_L_LIGHT
    } else {
        HOVER_TARGET_L_DARK
    };
    let mut best = base_fg;
    let mut best_distance = 0.0;
    for step in 1..=HOVER_STEPS {
        let amount = f64::from(step) / f64::from(HOVER_STEPS);
        for target_l in targets {
            let candidate = crate::theme::Theme::vivify(base_fg, theme.accent, amount, target_l);
            let distance = crate::theme::Theme::perceptual_distance(base_fg, candidate);
            if distance >= HOVER_MIN_DISTANCE {
                return candidate;
            }
            if distance > best_distance {
                best = candidate;
                best_distance = distance;
            }
        }
    }
    best
}

/// 装飾の一片。`fg` は行の色より優先するが、選択行では行の色に譲る。
///
/// 太字だけを個別に持てるようにしてあり、下線は持てない。下線は hover が
/// 「あなたが指しているのはこれだ」を示すために専有していて、装飾に乗ると
/// ポインタの印ではなく行全体のテキスト入力の下線に見える。
pub struct Segment<'a> {
    pub text: std::borrow::Cow<'a, str>,
    pub fg: Option<Color>,
    pub bold: bool,
}

impl<'a> Segment<'a> {
    pub fn plain(text: impl Into<std::borrow::Cow<'a, str>>) -> Self {
        Self {
            text: text.into(),
            fg: None,
            bold: false,
        }
    }

    pub fn colored(text: impl Into<std::borrow::Cow<'a, str>>, fg: Color) -> Self {
        Self {
            text: text.into(),
            fg: Some(fg),
            bold: false,
        }
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
}

/// 一覧の 1 行。
///
/// 名前を装飾から分けて持つのは、hover の下線を名前だけに乗せるため。分けずに
/// 組むこともできるが、そうすると各リストが規約として覚えていなければならず、
/// 実際コメント一覧は覚えていない。型にしておけば忘れられない。
pub struct Row<'a> {
    /// 名前より前。インデント、展開矢印、アイコン。
    pub lead: Vec<Segment<'a>>,
    /// この行が指しているもの。
    pub name: std::borrow::Cow<'a, str>,
    /// 名前より後。変更行数、バッジ、印。
    pub trail: Vec<Segment<'a>>,
    /// 名前の地の色。hover の強調はここから導く。
    pub name_fg: Color,
    /// 名前を太字にする。選択行では選択の強調が既に太字なので効果はない。
    pub name_bold: bool,
}

impl<'a> Row<'a> {
    pub fn new(name: impl Into<std::borrow::Cow<'a, str>>, name_fg: Color) -> Self {
        Self {
            lead: Vec::new(),
            name: name.into(),
            trail: Vec::new(),
            name_fg,
            name_bold: false,
        }
    }

    pub fn bold_name(mut self) -> Self {
        self.name_bold = true;
        self
    }

    pub fn lead(mut self, segments: impl IntoIterator<Item = Segment<'a>>) -> Self {
        self.lead.extend(segments);
        self
    }

    pub fn trail(mut self, segments: impl IntoIterator<Item = Segment<'a>>) -> Self {
        self.trail.extend(segments);
        self
    }

    /// 行の状態を当てて描ける形にする。
    pub fn into_line(
        self,
        theme: &crate::theme::Theme,
        selected: bool,
        panel_focused: bool,
        hover: Option<HoverPhase>,
    ) -> Line<'a> {
        let style = row_style(theme, self.name_fg, selected, panel_focused, hover);
        let decoration = decoration_style(style);
        // 選択行では種別色を捨てる。選択の背景の上で読める保証が 11 テーマぶんは無い。
        let paint = |s: Segment<'a>| {
            let mut style = match s.fg {
                Some(fg) if !selected => decoration.fg(fg),
                _ => decoration,
            };
            if s.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            Span::styled(s.text, style)
        };

        let name_style = if self.name_bold {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        };
        let mut spans: Vec<Span<'a>> = self.lead.into_iter().map(paint).collect();
        spans.push(Span::styled(self.name, name_style));
        spans.extend(self.trail.into_iter().map(paint));
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use ratatui::style::Color;

    fn test_theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn row_style_cases_are_pairwise_distinct() {
        let theme = test_theme();
        let base_fg = theme.fg;

        // selected x panel_focused x hover の行列を代表するケース。
        // （個別に手で決めた期待 Style と比較するのではなく）すべてのペアを
        // 比較することがこのテストを意味あるものにしている:
        // 固定の期待値と比較するだけでは実装をそのまま繰り返すだけで、
        // 自明に通ってしまう。
        let cases = [
            ("normal", row_style(&theme, base_fg, false, true, None)),
            (
                "hover_on",
                row_style(&theme, base_fg, false, true, Some(HoverPhase::On)),
            ),
            (
                "selected_focused",
                row_style(&theme, base_fg, true, true, None),
            ),
            (
                "selected_unfocused",
                row_style(&theme, base_fg, true, false, None),
            ),
        ];

        for i in 0..cases.len() {
            for j in (i + 1)..cases.len() {
                assert_ne!(
                    cases[i].1, cases[j].1,
                    "expected `{}` and `{}` to differ",
                    cases[i].0, cases[j].0
                );
            }
        }
    }

    #[test]
    fn selected_row_ignores_hover() {
        let theme = test_theme();
        let base_fg = theme.fg;

        let no_hover = row_style(&theme, base_fg, true, true, None);
        let with_hover = row_style(&theme, base_fg, true, true, Some(HoverPhase::On));
        assert_eq!(no_hover, with_hover);

        let no_hover_inactive = row_style(&theme, base_fg, true, false, None);
        let with_hover_inactive = row_style(&theme, base_fg, true, false, Some(HoverPhase::On));
        assert_eq!(no_hover_inactive, with_hover_inactive);
    }

    #[test]
    fn hover_row_phase_reflects_current_row() {
        let mut hover = HoverRow::default();
        hover.set(Some(3));
        assert_eq!(hover.phase(3), Some(HoverPhase::On));
        assert_eq!(hover.phase(4), None);
    }

    #[test]
    fn moving_hover_starts_fade_on_previous_row() {
        let mut hover = HoverRow::default();
        hover.set(Some(3));
        hover.set(Some(4));

        assert_eq!(hover.phase(4), Some(HoverPhase::On));
        match hover.phase(3) {
            Some(HoverPhase::FadingOut(t)) => assert!(t >= 0.9, "expected t >= 0.9, got {t}"),
            other => panic!("expected FadingOut close to 1.0, got {other:?}"),
        }
    }

    #[test]
    fn fade_completes_after_duration_elapses() {
        let hover = HoverRow::with_left_at(3, Instant::now() - Duration::from_millis(200));
        assert_eq!(hover.phase(3), None);
    }

    #[test]
    fn resetting_same_row_does_not_restart_fade() {
        let mut hover = HoverRow::default();
        hover.set(Some(3));
        hover.set(Some(4));
        let first_left = hover.left;

        // 既に現在の行を再セットしても left にとっては no-op でなければならない。
        hover.set(Some(4));
        assert_eq!(hover.left, first_left);
    }

    #[test]
    fn is_animating_reflects_active_fade() {
        let mut hover = HoverRow::default();
        assert!(!hover.is_animating());

        hover.set(Some(3));
        assert!(!hover.is_animating(), "no previous row to fade yet");

        hover.set(Some(4));
        assert!(hover.is_animating());

        let done = HoverRow::with_left_at(3, Instant::now() - Duration::from_millis(200));
        assert!(!done.is_animating());
    }

    /// フェードは hover 中の色 *から* base の色 *へ* 戻る向きでなければならず、
    /// 逆であってはならない。lerp の2つの色引数を入れ替えてもコンパイルは通り
    /// アニメーションもする — ただし、ポインタが離れた後に行が明るくなるという
    /// 逆の動きが再生されてしまう。方向を検証してこそこれを検知できる。
    #[test]
    fn fading_out_starts_lit_and_ends_at_base() {
        let theme = test_theme();
        let base = theme.fg;
        let lit = row_style(&theme, base, false, true, Some(HoverPhase::On)).fg;

        assert_eq!(
            row_style(&theme, base, false, true, Some(HoverPhase::FadingOut(1.0))).fg,
            lit,
            "full strength must match the hovered colour, or the row jumps when the pointer leaves"
        );
        assert_eq!(
            row_style(&theme, base, false, true, Some(HoverPhase::FadingOut(0.0))).fg,
            Some(base),
            "zero strength must be back at the row's own colour"
        );
        let mid = row_style(&theme, base, false, true, Some(HoverPhase::FadingOut(0.5))).fg;
        assert_ne!(mid, lit);
        assert_ne!(mid, Some(base));
    }

    /// hover が *嘘をつく* 不具合の回帰防止: solarized-dark と gruvbox では
    /// hover 色として使っていた theme.accent が theme.warning
    /// （ステージ済みの色）と等しく、未ステージのファイルを hover すると
    /// ステージ済みに見えていた。強調は今では行自身の色から導出されるので、
    /// これはすべてのテーマ、意味を持つすべての基本色について成り立つ。
    #[test]
    fn hover_never_repaints_a_row_as_another_meaningful_token() {
        for &name in crate::theme::Theme::all_names() {
            let theme = crate::theme::Theme::from_name(name);
            // hover 中の行が成りすましてはならない意味を持つトークン群:
            // D6 の4つのステージ色に加え、ツリーのディレクトリ色。
            let meaningful = [
                ("error/unstaged", theme.error),
                ("warning/staged", theme.warning),
                ("success/committed", theme.success),
                ("hint/untracked", theme.hint),
                ("info/directory", theme.info),
            ];
            for (base_name, base) in meaningful {
                let hovered = row_style(&theme, base, false, true, Some(HoverPhase::On))
                    .fg
                    .expect("row_style always sets a foreground");
                assert_ne!(
                    hovered, base,
                    "{name}: hovering {base_name} produced no visible change"
                );
                for (other_name, other) in meaningful {
                    if other == base {
                        continue;
                    }
                    assert_ne!(
                        hovered, other,
                        "{name}: hovering {base_name} makes it look like {other_name}"
                    );
                }
            }
        }
    }

    /// 強調ロジックを作り直した狙い: 行がどの色から始まっていても hover は
    /// *等しく* 見えなければならない。以前の変換は暗い行ではこの下限を
    /// クリアしていたが、普通の theme.fg の行では2〜3倍不足していた。
    /// そのため同じ操作でも、ファイルの git 状態によって明らかに異なる
    /// フィードバックが生じていた。
    #[test]
    fn hover_clears_the_visibility_floor_on_every_theme_and_base_colour() {
        for &name in crate::theme::Theme::all_names() {
            let theme = crate::theme::Theme::from_name(name);
            let bases = [
                ("fg/tracked file", theme.fg),
                ("hint/untracked", theme.hint),
                ("info/directory", theme.info),
                ("error/unstaged", theme.error),
                ("warning/staged", theme.warning),
                ("success/committed", theme.success),
                ("accent/summary", theme.accent),
            ];
            for (base_name, base) in bases {
                let hovered = row_style(&theme, base, false, true, Some(HoverPhase::On))
                    .fg
                    .expect("row_style always sets a foreground");
                let distance = Theme::perceptual_distance(base, hovered);
                assert!(
                    distance >= HOVER_MIN_DISTANCE,
                    "{name}: hovering {base_name} only moves it by {distance:.0}, \
                     below the {HOVER_MIN_DISTANCE:.0} floor"
                );
            }
        }
    }

    /// hover はパレットや色覚特性の影響を受けないよう、色以外の2つ目の
    /// チャンネルを持つ。selection はそれを借用してはならない:
    /// hover 中の行が選択中の行の隣にあるとき、両者は区別できなければならない。
    #[test]
    fn only_the_hovered_row_is_underlined() {
        let theme = test_theme();
        let base = theme.fg;
        let underlined = |style: Style| style.add_modifier.contains(Modifier::UNDERLINED);

        assert!(underlined(row_style(
            &theme,
            base,
            false,
            true,
            Some(HoverPhase::On)
        )));
        assert!(!underlined(row_style(&theme, base, false, true, None)));
        assert!(!underlined(row_style(&theme, base, true, true, None)));
        assert!(!underlined(row_style(
            &theme,
            base,
            true,
            true,
            Some(HoverPhase::On)
        )));
        assert!(
            !underlined(row_style(
                &theme,
                base,
                false,
                true,
                Some(HoverPhase::FadingOut(1.0))
            )),
            "the underline marks where the pointer *is*, so it must not linger \
             into the fade-out"
        );
    }

    /// 下線は行ではなく名前を示すマークである: インデント/矢印/アイコンの
    /// プレフィックス部分は hover の *色* は保持する（行全体としては引き続き
    /// 明るくなる）が下線は落とす。これにより、深くネストされた行の左端から
    /// 下線が始まってしまうことを防いでいる。
    #[test]
    fn decoration_keeps_the_hover_colour_but_not_the_underline() {
        let theme = test_theme();
        let hovered = row_style(&theme, theme.fg, false, true, Some(HoverPhase::On));
        let decoration = decoration_style(hovered);

        assert_eq!(decoration.fg, hovered.fg);
        assert!(hovered.add_modifier.contains(Modifier::UNDERLINED));
        assert!(!decoration.add_modifier.contains(Modifier::UNDERLINED));
    }

    /// 選択中の行の BOLD は hover のアフォーダンスではないので、分割後も
    /// 保持されなければならない。さもないとプレフィックス部分がそれが属する
    /// 名前より薄く描画されてしまう。
    #[test]
    fn decoration_preserves_selection_styling() {
        let theme = test_theme();
        let selected = row_style(&theme, theme.fg, true, true, None);
        let decoration = decoration_style(selected);

        assert_eq!(decoration.fg, selected.fg);
        assert_eq!(decoration.bg, selected.bg);
        assert!(decoration.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn lerp_dummy_color_is_used_directly_when_hover_is_none_or_non_rgb() {
        // hover がないとき row_style が誤って lerp を呼び出さないことの
        // 健全性チェック。非RGBの base_fg を使うことで検証する（lerp は
        // Theme::lerp の契約上いずれにせよそれをそのまま返すが、こうしておく
        // ことで後から読む人にとって分岐の形が明示的になる）。
        let theme = test_theme();
        let style = row_style(&theme, Color::Reset, false, true, None);
        assert_eq!(style.fg, Some(Color::Reset));
    }
}
