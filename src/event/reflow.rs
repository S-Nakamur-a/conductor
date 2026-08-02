//! reflow トランスクリプトビューの純粋なスクロール計算。
//!
//! App の全状態から切り離して単体テストできるよう、モジュールとして
//! 抽出してある。中心となる不変条件は「論理行1行 == 表示行1行」
//! (Paragraph::wrap を使わない) であり、これにより
//! max_scroll == total_lines.saturating_sub(inner_height) が成り立つ。

/// scroll を有効範囲 [0, total.saturating_sub(inner)] にクランプする。
///
/// inner はパネルの可視領域の高さ (terminal の行数)。総行数が inner 以下
/// (ログ全体がパネルに収まる) の場合は 0 を返し、最終行の下に空行を残さず
/// 先頭に固定した表示にする。
pub fn clamp_scroll(scroll: usize, total: usize, inner: usize) -> usize {
    scroll.min(total.saturating_sub(inner))
}

/// scroll がコンテンツの論理的な最下部に達しているか、それを超えていたら
/// true を返す。
///
/// inner は可視領域の高さ。scroll が total - inner の位置にあるとき、最終行の
/// 内容が最後の表示行にある — これが「最下部にいる」状態。
pub fn at_bottom(scroll: usize, total: usize, inner: usize) -> bool {
    scroll >= total.saturating_sub(inner)
}

/// 最新行に対応するスクロールオフセット — [at_bottom] が最下部として報告する
/// 位置であり、follower が固定される位置でもある。
pub fn bottom_scroll(total: usize, inner: usize) -> usize {
    total.saturating_sub(inner)
}

/// 下地のジオメトリが変わったあと、ビューポートがどこに位置すべきか。
///
/// 毎フレーム呼ばれるが、anchored がセットされるのは行リストが直前に
/// 再構築されたとき (幅変更や expand トグル) だけ。2つの分岐で
/// 「読んでいた位置を保つ」というルール全体を表現している。
///
/// * following — 読者は最新のターンに乗っているので、最下部へ再固定する。
///   これがないと、リサイズで最新の出力がビューポートの下に取り残されて
///   しまう: パネルが狭くなると同じテキストがより多くの行に折り返される
///   ため、古いオフセット (正しく再アンカーされたものであっても) はもはや
///   末尾に届かない。
/// * それ以外 — 読者は履歴のどこかに留まっているので、anchored (再構築後の
///   同じ論理行 = エントリ/ブロック/オフセットのインデックス) を尊重する。
///   再構築が起きず anchored が意味を持たないフレームは previous にフォール
///   バックする。
///
/// 再構築をまたいで生のオフセットを引き継ぐことは決してしない: 行の
/// インデックスは幅ごとに意味が変わるので、それこそが以前の reflow が
/// 読者を無関係な場所へ飛ばしていた原因。
pub fn scroll_after_reflow(
    following: bool,
    anchored: Option<usize>,
    previous: usize,
    total: usize,
    inner: usize,
) -> usize {
    let target = if following {
        bottom_scroll(total, inner)
    } else {
        anchored.unwrap_or(previous)
    };
    clamp_scroll(target, total, inner)
}

// トランジションアニメーション

/// 入場/退場トランジションアニメーションの総時間 (ミリ秒)。
///
/// 500ms なら、境界線の色相が accent から補色へ (退場時はその逆へ) 単一の
/// 滑らかなグラデーションで移り変わる — 以前の急なちらつきに比べて落ち着いた
/// 印象になる — それでいて read モードを抜ける操作がもたつかない程度に短い。
pub const TRANSITION_DURATION_MS: u64 = 500;

/// トランジションアニメーションがどこまで進んだかを [0.0, 1.0] にクランプ
/// して計算する。
///
/// start はアニメーションが始まった Instant。生成した瞬間は 0.0 を返し、
/// duration_ms ミリ秒が経過した時点以降は 1.0 を返す。duration_ms が
/// ゼロの場合はゼロ除算を避けるため即座に完了 (1.0) として扱う。
pub fn sweep_progress(start: &std::time::Instant, duration_ms: u64) -> f64 {
    if duration_ms == 0 {
        return 1.0;
    }
    let elapsed_ms = start.elapsed().as_millis() as f64;
    (elapsed_ms / duration_ms as f64).clamp(0.0, 1.0)
}

/// 入場/退場の境界線カラートランジション用の smoothstep イージング。
///
/// [0, 1] の線形な進捗を、両端で傾きがゼロになる古典的な 3p² − 2p³ 曲線で
/// [0, 1] のイージング済みの値へ写像する。呼び出し側はこの結果を使って
/// accent 色とその補色のあいだで境界線を補間するので、色相の変化はちらつく
/// ことなく穏やかに始まり穏やかに収まる — 0.0 は開始色のまま、1.0 は目標色
/// に達する。
pub fn transition_eased(progress: f64) -> f64 {
    let p = progress.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

#[cfg(test)]
mod tests {
    use super::*;

    // clamp_scroll

    #[test]
    fn clamp_allows_zero() {
        assert_eq!(clamp_scroll(0, 100, 20), 0);
    }

    #[test]
    fn clamp_pins_to_max() {
        // max = 100 - 20 = 80。
        assert_eq!(clamp_scroll(200, 100, 20), 80);
    }

    #[test]
    fn clamp_at_exact_max() {
        assert_eq!(clamp_scroll(80, 100, 20), 80);
    }

    #[test]
    fn clamp_within_range_unchanged() {
        assert_eq!(clamp_scroll(40, 100, 20), 40);
    }

    #[test]
    fn clamp_when_log_shorter_than_panel_returns_zero() {
        // total(10) < inner(20): ログ全体が収まる → max_scroll = 0
        assert_eq!(clamp_scroll(5, 10, 20), 0);
        assert_eq!(clamp_scroll(0, 10, 20), 0);
    }

    #[test]
    fn clamp_when_total_equals_inner_returns_zero() {
        assert_eq!(clamp_scroll(0, 20, 20), 0);
        assert_eq!(clamp_scroll(1, 20, 20), 0);
    }

    #[test]
    fn clamp_total_zero_returns_zero() {
        assert_eq!(clamp_scroll(0, 0, 20), 0);
    }

    // at_bottom

    #[test]
    fn at_bottom_when_scroll_equals_max() {
        assert!(at_bottom(80, 100, 20));
    }

    #[test]
    fn at_bottom_when_scroll_exceeds_max() {
        assert!(at_bottom(90, 100, 20));
    }

    #[test]
    fn not_at_bottom_when_scroll_below_max() {
        assert!(!at_bottom(79, 100, 20));
    }

    #[test]
    fn at_bottom_when_log_fits_in_panel() {
        // total(10) <= inner(20): max_scroll = 0 なので scroll >= 0 なら常に最下部
        assert!(at_bottom(0, 10, 20));
    }

    // scroll_after_reflow

    #[test]
    fn follower_repins_to_bottom_when_a_narrower_width_adds_lines() {
        // このフラグ全体が存在する理由となったリグレッション: 読者は
        // 100行/20行のパネル (scroll 80) で最新ターンに乗っていた。パネルが
        // 狭くなり、同じテキストが140行に折り返される。anchor をそのまま
        // 使うと古い先頭行が再び先頭に来てしまい、最新の40行がビューポートの
        // 下に取り残される。
        let anchored = Some(80); // anchor が解決した先はどこでも構わない
        assert_eq!(scroll_after_reflow(true, anchored, 80, 140, 20), 120);
    }

    #[test]
    fn follower_repins_to_bottom_when_the_panel_gets_shorter() {
        // 高さだけの変更: 再構築は起きないので anchored は None。クランプ
        // だけでは scroll が80のままになり、最後の10行が下にはみ出て
        // 切れてしまう。
        assert_eq!(scroll_after_reflow(true, None, 80, 100, 10), 90);
    }

    #[test]
    fn detached_reader_lands_on_the_anchored_line() {
        // 履歴の途中に留まっている: anchor が古い生オフセットにも最下部にも
        // 優先する。
        assert_eq!(scroll_after_reflow(false, Some(57), 40, 200, 20), 57);
    }

    #[test]
    fn detached_reader_keeps_its_offset_when_nothing_was_rebuilt() {
        assert_eq!(scroll_after_reflow(false, None, 40, 200, 20), 40);
    }

    #[test]
    fn detached_reader_never_gets_dragged_to_the_bottom() {
        // 個々の数値より重要な性質: following していない読者に対しては、
        // anchor が本当に最下部を指している場合を除き、anchor とジオメトリの
        // どんな組み合わせも最下部に解決してはならない。
        let total = 300;
        let inner = 25;
        let bottom = bottom_scroll(total, inner);
        for anchored in [Some(0), Some(11), Some(120), None] {
            let got = scroll_after_reflow(false, anchored, 33, total, inner);
            assert!(
                got < bottom,
                "anchored={anchored:?} resolved to {got}, i.e. the live tail"
            );
        }
    }

    #[test]
    fn anchored_index_past_the_end_is_clamped_not_wrapped() {
        // 縮んだブロックは最後の有効オフセットを超えて解決することがある。
        // その場合は後段で範囲外インデックスになるのではなく、最下部に
        // クランプされなければならない。
        assert_eq!(scroll_after_reflow(false, Some(9_999), 40, 200, 20), 180);
    }

    #[test]
    fn short_log_collapses_to_top_for_follower_and_reader_alike() {
        assert_eq!(scroll_after_reflow(true, None, 0, 10, 40), 0);
        assert_eq!(scroll_after_reflow(false, Some(5), 3, 10, 40), 0);
    }

    #[test]
    fn following_result_always_reports_at_bottom() {
        for (total, inner) in [(100usize, 20usize), (10, 40), (0, 5), (41, 7)] {
            let s = scroll_after_reflow(true, None, 0, total, inner);
            assert!(at_bottom(s, total, inner), "total={total} inner={inner}");
        }
    }

    // sweep_progress

    #[test]
    fn sweep_progress_zero_duration_returns_complete() {
        let t = std::time::Instant::now();
        assert_eq!(sweep_progress(&t, 0), 1.0);
    }

    #[test]
    fn sweep_progress_fresh_instant_is_near_zero() {
        let t = std::time::Instant::now();
        let p = sweep_progress(&t, TRANSITION_DURATION_MS);
        // 始まったばかりのアニメーションは10%を大きく下回っていなければならない。
        assert!(p < 0.1, "expected near 0.0, got {p}");
    }

    // transition_eased

    #[test]
    fn transition_eased_endpoints_are_exact() {
        // smoothstep は開始を0、終了を1に固定するので、境界線は accent から
        // 始まりぴったり補色に落ち着く。
        assert_eq!(transition_eased(0.0), 0.0);
        assert_eq!(transition_eased(1.0), 1.0);
    }

    #[test]
    fn transition_eased_midpoint_is_half() {
        // 3(0.5)² − 2(0.5)³ = 0.5 — 対称な曲線は中心を通る。
        assert!((transition_eased(0.5) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn transition_eased_is_monotonic_within_unit_range() {
        // 単一の滑らかなランプ: 減少することはなく、常に [0, 1] の範囲内。
        let mut prev = 0.0;
        for i in 0..=100 {
            let p = i as f64 / 100.0;
            let v = transition_eased(p);
            assert!((0.0..=1.0).contains(&v), "transition_eased({p}) = {v} out of range");
            assert!(v >= prev - 1e-12, "transition_eased must be monotonic non-decreasing");
            prev = v;
        }
    }

    #[test]
    fn transition_eased_clamps_out_of_range_input() {
        assert_eq!(transition_eased(-0.5), 0.0);
        assert_eq!(transition_eased(1.5), 1.0);
    }

    // 統合: pending_bottom の固定

    #[test]
    fn pending_bottom_pin_matches_clamp_max() {
        let total = 150usize;
        let inner = 30usize;
        let pinned = total.saturating_sub(inner); // 120
        assert_eq!(clamp_scroll(pinned, total, inner), pinned);
        assert!(at_bottom(pinned, total, inner));
    }
}
