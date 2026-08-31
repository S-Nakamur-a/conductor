//! ガターグリフの幅対策についてのテスト。
//!
//! 独立した2つの半分に分かれている。どちらか片方だけでは誤解を招くため:
//!
//! * [marks_the_hole] — ビルダーが正しいセルに印を付けているか。
//! * [skipping_forces_an_absolute_move] — そのセルに印を付けることで、実際の
//!   crossterm バックエンドが絶対位置のカーソル移動を発行するか。
//!
//! これらが示せないことに注意: 端末が ⏺ を1カラムで描くか2カラムで描くかは分からない。
//! ratatui::buffer::Buffer::set_stringn はビルダーと同じ unicode-width クレートで
//! 計測するので、プロセス内テストではこのモデルから逃れられない。この仕組みの狙いは
//! その答えを無関係にすることである — 本文はどちらの場合も絶対位置に配置される —
//! だが「見た目が正しいか」の確認には結局のところ実端末上での人間による確認が要る。

use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use syntect::highlighting::ThemeSet;

use crate::reflow::log::{DisplayBlock, LogEntry, Role};
use crate::ui::markdown::MarkdownCache;

use super::build::{BuildCtx, MAX_GUTTER_GLYPH_COL, build_lines};
use super::glyphs::{ASSISTANT_MARKER, TOOL_RESULT_GLYPH};

/// prev → next を実際の CrosstermBackend に通して描画し、それが書き出した
/// バイト列を返す。
fn flush(prev: &Buffer, next: &Buffer) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut backend = CrosstermBackend::new(&mut out);
        backend.draw(prev.diff(next).into_iter()).unwrap();
    }
    out
}

fn one_row(glyph: &str, skip: bool) -> Buffer {
    let mut b = Buffer::empty(Rect::new(0, 0, 20, 1));
    b[(0u16, 0u16)].set_symbol(glyph);
    b[(1u16, 0u16)].set_symbol(" ");
    b[(1u16, 0u16)].set_skip(skip);
    for (i, c) in "hello".chars().enumerate() {
        b[(2 + i as u16, 0u16)].set_char(c);
    }
    b
}

/// グリフの直後のセルをスキップすると、バックエンドはグリフからそのまま書き続けるのではなく、
/// 絶対位置で（1始まりの）カラム3へジャンプしなければならない。スキップしないケースも
/// 検証する: それが無いと、MoveTo が無条件に発行されている場合でもこのテストは通って
/// しまい、set_skip について何も証明しないことになる。
#[test]
fn 飛ばすときは絶対位置指定になる() {
    // 前フレームは検証対象のセルで実際に違いを持たなければならない。空のバッファ2つを
    // 比較すると、カラム1は変化なしのままなので diff がそこを省略し、いずれにせよ
    // 絶対位置の移動が発行されてしまう — 対照群として成立するには、上書きすべき何かが
    // 実際にそこに存在している必要がある。
    let mut prev = Buffer::empty(Rect::new(0, 0, 20, 1));
    prev[(1u16, 0u16)].set_char('X');

    let without = flush(&prev, &one_row(ASSISTANT_MARKER, false));
    let with = flush(&prev, &one_row(ASSISTANT_MARKER, true));

    const ABSOLUTE_MOVE_TO_COL3: &[u8] = b"\x1b[1;3H";
    assert!(
        !without.windows(6).any(|w| w == ABSOLUTE_MOVE_TO_COL3),
        "control case should write contiguously, got {:?}",
        String::from_utf8_lossy(&without)
    );
    assert!(
        with.windows(6).any(|w| w == ABSOLUTE_MOVE_TO_COL3),
        "skipped cell should force an absolute move, got {:?}",
        String::from_utf8_lossy(&with)
    );
}

/// スキップされたセルは決して消去もされない — だからこそ、その下に無関係なコンテンツを
/// 残しうるすべての経路は強制的な再描画を行う必要がある
/// （App::open_reflow と super::render を参照）。
#[test]
fn 飛ばしたセルは塗り直さない() {
    let mut prev = Buffer::empty(Rect::new(0, 0, 20, 1));
    prev[(1u16, 0u16)].set_char('X');

    let bytes = flush(&prev, &one_row(ASSISTANT_MARKER, true));

    assert!(
        !String::from_utf8_lossy(&bytes).contains('X'),
        "stale cell is left alone, so nothing overwrites it"
    );
}

fn build(entries: &[LogEntry], expanded: bool) -> Vec<Option<u16>> {
    let theme = crate::theme::Theme::default();
    let syntax_set = two_face::syntax::extra_newlines();
    let syntect_theme = ThemeSet::load_defaults()
        .themes
        .remove("base16-ocean.dark")
        .unwrap();
    let cache = MarkdownCache::new();
    let ctx = BuildCtx {
        entries,
        cache: &cache,
        theme: &theme,
        syntax_set: &syntax_set,
        syntect_theme: &syntect_theme,
        expanded,
    };
    build_lines(&ctx, 60)
        .meta
        .into_iter()
        .map(|m| m.skip_col)
        .collect()
}

fn entry(role: Role, blocks: Vec<DisplayBlock>) -> LogEntry {
    LogEntry { role, blocks }
}

/// 穴はグリフがどこにあってもその直後に置かれる — グリフの位置は常にカラム0とは
/// 限らない: ⎿ の結果行はカラム2までインデントされる。
#[test]
fn 穴に印を付ける() {
    // assistant のプロース: ⏺ は col0 にあるので、穴は col1。
    let assistant = build(
        &[entry(
            Role::Assistant,
            vec![DisplayBlock::Text("hello".into())],
        )],
        false,
    );
    assert_eq!(assistant.first().copied().flatten(), Some(1));

    // 展開された tool result:   ⎿   はグリフを col2 に置くので、穴は col3。
    let result = build(
        &[entry(
            Role::User,
            vec![DisplayBlock::ToolResult {
                kind: crate::reflow::log::ResultKind::Inline,
                lines: vec!["out".into()],
                is_error: false,
            }],
        )],
        true,
    );
    assert_eq!(result.first().copied().flatten(), Some(3));
}

/// user ターンはフル幅の背景ブロックなので、その内側に未書き込みのセルがあると
/// 欠けとして見えてしまう。そもそも ❯ は幅が曖昧なグリフでもない。
#[test]
fn ユーザのターンには穴を空けない() {
    let holes = build(
        &[entry(Role::User, vec![DisplayBlock::Text("hi".into())])],
        false,
    );
    assert!(holes.iter().all(Option::is_none), "{holes:?}");
}

/// 定数自体をガードする: もしグリフが穴のロジックの知らないものに差し替えられたら、
/// この対策は黙って効かなくなってしまう。
#[test]
fn 幅の曖昧なガターのグリフは全部登録されている() {
    for glyph in [
        ASSISTANT_MARKER,
        TOOL_RESULT_GLYPH,
        super::glyphs::THINKING_GLYPH,
    ] {
        let ch = glyph.chars().next().unwrap();
        assert!(
            super::glyphs::is_width_ambiguous(ch),
            "{glyph:?} (U+{:04X}) is drawn in the gutter but not registered as width-ambiguous",
            ch as u32
        );
    }
    // ……そして意図的にそうでない2つ。
    for glyph in [
        super::glyphs::USER_MARKER,
        super::glyphs::TEAMMATE_MESSAGE_GLYPH,
    ] {
        let ch = glyph.chars().next().unwrap();
        assert!(!super::glyphs::is_width_ambiguous(ch), "{glyph:?}");
    }
}

/// 本文テキスト内の ⏺/⎿/✻ はコンテンツであって、ガターのマーカーではない。以前は
/// 走査が行全体に対して行われていたため、Claude Code の出力を引用したトランスクリプト
/// （このアプリ自身のトランスクリプトが常にそうしている）は文の途中でセルが空白化
/// されてしまっていた: 未書き込みのセルは決して塗られないので、その文字が消え、
/// 前フレームがそこに描いていたものがそのまま残ってしまう。
#[test]
fn 本文中のグリフには穴を空けない() {
    // ⏺ が継続行に落ちるくらい十分長くする。継続行はマーカーではなく空白インデントを
    // 持つので、その行の他の何もそこに穴を望まない。
    let text = format!("{} \u{23fa} tail", "word ".repeat(20));
    let holes = build(
        &[entry(Role::Assistant, vec![DisplayBlock::Text(text)])],
        false,
    );
    // 最初の行は正当に穴を持つ（列0に自分自身の ⏺ マーカーがある）。
    // それ以外の行は持ってはならない。
    assert_eq!(holes.first().copied().flatten(), Some(1), "{holes:?}");
    assert!(
        holes.iter().skip(1).all(Option::is_none),
        "a glyph in body text punched a hole into content: {holes:?}"
    );
}

/// 穴はガターのグリフに属するものであり、本文が幅1でない文字（全角 CJK、ZWJ
/// シーケンス、異体字セレクタ、肌色修飾子、結合文字）を含んでいても動いてはならない。
///
/// これが証明しないことに注意: 走査は今やガターで止まるので、width_risk_hole の
/// 1文字単位から1グラフェム単位への変更はここでは単独では観測できない（ガターは
/// ASCII のスペースとマーカーのみで、両者の間に差は無い）。この確認は
/// helpers::truncate_to_width と user_text::wrap_plain_text との整合性のために
/// 残してある。このテストは、その2つが揃って守るべき不変条件を固定するものである。
#[test]
fn 全角や複数文字のクラスタでも穴はずれない() {
    // ⎿ は続くものが何であれ   ⎿   プレフィックスのカラム2に位置するので、
    // これらすべての本文について穴はカラム3にある。
    for body in [
        "plain",
        "日本語の全角テキスト", // width-2 CJK
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} family", // ZWJ, 7 chars / 2 cols
        "\u{26a0}\u{fe0f} warn",       // emoji-presentation selector
        "\u{1f44b}\u{1f3fd} wave",     // skin-tone modifier
        "e\u{0301}\u{0301} combining", // combining marks
    ] {
        let holes = build(
            &[entry(
                Role::User,
                vec![DisplayBlock::ToolResult {
                    kind: crate::reflow::log::ResultKind::Inline,
                    lines: vec![body.to_string()],
                    is_error: false,
                }],
            )],
            true,
        );
        assert_eq!(
            holes.first().copied().flatten(),
            Some(3),
            "hole moved off the gutter glyph for body {body:?}: {holes:?}"
        );
    }
}

/// 穴はその行が他に使っていないセルに置かれなければならず、構築後の各行はパネルに
/// 収まっていなければならない — この2つの不変条件をまとめて、幅広文字のコンテンツで
/// 検証する。これこそがこの仕組みの存在理由である。
#[test]
fn 幅広の中身でも穴は行の中に収まる() {
    use unicode_width::UnicodeWidthStr;

    let theme = crate::theme::Theme::default();
    let syntax_set = two_face::syntax::extra_newlines();
    let syntect_theme = ThemeSet::load_defaults()
        .themes
        .remove("base16-ocean.dark")
        .unwrap();
    let cache = MarkdownCache::new();
    let entries = vec![
        entry(
            Role::User,
            vec![DisplayBlock::Text(
                "日本語の全角テキストと絵文字 \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} と \u{26a0}\u{fe0f}".into(),
            )],
        ),
        entry(
            Role::Assistant,
            vec![DisplayBlock::Text(
                "全角 日本語日本語日本語日本語日本語 and \u{1f600} tail".into(),
            )],
        ),
    ];
    for width in [20usize, 40, 60, 80] {
        let ctx = BuildCtx {
            entries: &entries,
            cache: &cache,
            theme: &theme,
            syntax_set: &syntax_set,
            syntect_theme: &syntect_theme,
            expanded: true,
        };
        let built = build_lines(&ctx, width);
        for (line, meta) in built.lines.iter().zip(built.meta.iter()) {
            let w: usize = line
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(w <= width, "line overflows at width {width}: {w} cols");
            if let Some(col) = meta.skip_col {
                assert!(
                    (col as usize) <= MAX_GUTTER_GLYPH_COL + 1,
                    "hole at column {col} is past the gutter (width {width})"
                );
            }
        }
    }
}
// 離脱バッジ
//
// このバッジは、ビューが最新のターンを表示していないことを示す唯一の合図であり、
// そこへ戻るための唯一のポインタ経路でもある。そのため、表示されるかどうかのルールも
// 報告されるヒット領域も、目視ではなくアサーションで確認する。

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use unicode_width::UnicodeWidthStr;

use super::frame::{JUMP_BADGE_LABELS, render_jump_badge};

/// width x height のフレームにバッジを描画し、報告された値と描画後の画面を返す。
fn draw_badge(width: u16, height: u16, following: bool) -> (Option<Rect>, Buffer) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut hit = None;
    terminal
        .draw(|f| {
            hit = render_jump_badge(f, Rect::new(0, 0, width, height), following);
        })
        .unwrap();
    (hit, terminal.backend().buffer().clone())
}

fn screen_text(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 最新のターンに追従している状態は静かな状態である: バッジは無く、そしてそれと
/// 同じくらい重要なこととして、ヒット領域も無い — そうでないと、その場所への
/// クリックが動作し続けてしまう。
#[test]
fn 追従中はバッジを出さない() {
    let (hit, buf) = draw_badge(40, 6, true);
    assert_eq!(hit, None);
    assert!(!screen_text(&buf).contains("(G)"), "{}", screen_text(&buf));
}

#[test]
fn 離れて読むときは右下にバッジを描き矩形を返す() {
    let (hit, buf) = draw_badge(40, 6, false);
    let rect = hit.expect("detached view must offer a way back");

    // 右下の角、右端に接するように配置される。
    assert_eq!(rect.y, 5, "badge belongs on the last row");
    assert_eq!(rect.x + rect.width, 40, "badge is right-aligned");
    assert_eq!(rect.height, 1);

    // 報告される矩形はテキストが実際にある場所である — これはクリックハンドラが
    // 依存している契約である。
    let text = screen_text(&buf);
    let last_row = text.lines().last().unwrap();
    assert!(last_row.contains("Jump to latest (G)"), "{last_row:?}");
    assert_eq!(
        rect.width as usize,
        UnicodeWidthStr::width(JUMP_BADGE_LABELS[0])
    );
}

/// バッジは読めないほど切り詰められるのではなく、より短いラベルへと段階的に縮んでいき、
/// 最短のものすら収まらない場合には完全に消える — Claude 用のカラムは狭くなりうる。
#[test]
fn バッジはパネルに合わせて縮み最後は諦める() {
    // フルラベルに十分な幅（20カラム + 余裕1）。
    assert_eq!(draw_badge(21, 3, false).0.map(|r| r.width), Some(20));
    // それより1カラム足りない: " Latest (G) "（12）にフォールバック。
    assert_eq!(draw_badge(20, 3, false).0.map(|r| r.width), Some(12));
    // " (G) "（5）が入るだけの余地しかない。
    assert_eq!(draw_badge(12, 3, false).0.map(|r| r.width), Some(5));
    // それすら入らない。
    assert_eq!(draw_badge(5, 3, false).0, None);
}

/// すべてのラベルは unicode-width が示す通りに正確に計測されなければならない。
/// バッジはパネルの右端に接して配置されるため、端末がそれより広く描くグリフがあると
/// 末尾が境界線にはみ出してしまう。
#[test]
fn バッジのラベルは素のasciiだけ() {
    for label in JUMP_BADGE_LABELS {
        assert!(
            label.is_ascii(),
            "{label:?} is not ASCII; see the note on JUMP_BADGE_LABELS"
        );
        assert_eq!(UnicodeWidthStr::width(label), label.len());
    }
}

/// ラベルは長い順に並んでいる。render_jump_badge は収まる最初のものを選ぶので、
/// 並び順を誤ると、黙ってより短いものが優先されてしまう。
#[test]
fn バッジのラベルは長い順に並ぶ() {
    let widths: Vec<usize> = JUMP_BADGE_LABELS
        .iter()
        .map(|l| UnicodeWidthStr::width(*l))
        .collect();
    assert!(
        widths.windows(2).all(|w| w[0] > w[1]),
        "labels must strictly shrink, got {widths:?}"
    );
}

// リフローをまたぐスクロール位置
//
// 純粋な算術部分は event::reflow でカバーされている。ここでは実際の行ビルダーを
// 2つの幅で実行し、読者にとって重要な2つの結果を検証する: 離脱している読者は自分の
// 行に留まり、追従している読者は最新に留まる。

use crate::reflow::input::{at_bottom, scroll_after_reflow};

use super::build::BuiltLines;
use super::frame::anchor_index;

/// テスト対象の2つの幅で折り返し位置が実際に異なるくらい長いプロース — この
/// フィクスチャの仕事はそれだけである。
fn reflow_fixture() -> Vec<LogEntry> {
    // 20行のビューポートがそのほんの一部でしかないくらい長くする — ログが短いと、
    // 「4分の3まで下がった位置」がすでに末尾になってしまい、読者の着地点を決めるのが
    // アンカーではなくクランプになってしまう。
    (0..40)
        .map(|i| {
            entry(
                if i % 2 == 0 {
                    Role::Assistant
                } else {
                    Role::User
                },
                vec![DisplayBlock::Text(format!(
                    "Turn {i}: {}",
                    "the quick brown fox jumps over the lazy dog ".repeat(4)
                ))],
            )
        })
        .collect()
}

fn build_at(entries: &[LogEntry], width: usize) -> BuiltLines {
    let theme = crate::theme::Theme::default();
    let syntax_set = two_face::syntax::extra_newlines();
    let syntect_theme = ThemeSet::load_defaults()
        .themes
        .remove("base16-ocean.dark")
        .unwrap();
    let cache = MarkdownCache::new();
    let ctx = BuildCtx {
        entries,
        cache: &cache,
        theme: &theme,
        syntax_set: &syntax_set,
        syntect_theme: &syntect_theme,
        expanded: false,
    };
    build_lines(&ctx, width)
}

/// 報告されたバグをエンドツーエンドで再現する: 履歴のどこかに留まっている読者は、
/// 幅の変更後も同じターンを見ていなければならない — 最新のターンでもなく、
/// 古い行番号を偶然引き継いだ無関係なテキストでもない。
#[test]
fn 狭くしても離れて読む人は同じターンに留まる() {
    const INNER: usize = 20;
    let entries = reflow_fixture();

    let before = build_at(&entries, 80);
    // 4分の3の位置: 狭い幅によって追加された折り返し行が実際のズレとして蓄積する
    // くらい十分に下の方。
    let scroll = before.meta.len() * 3 / 4;
    let anchor = before.meta[scroll];

    let after = build_at(&entries, 50);
    assert!(
        after.lines.len() > before.lines.len(),
        "fixture must wrap into more lines at the narrower width"
    );

    // 生の行番号をそのまま引き継ぐのが、置き換えようとしている挙動である —
    // それが実際に誤りであったことを検証する。そうしないとこのテストは何も
    // 証明しない。
    let naive = after.meta[scroll];
    assert_ne!(
        (naive.entry, naive.block, naive.offset),
        (anchor.entry, anchor.block, anchor.offset),
        "fixture no longer renumbers lines; the test would pass vacuously"
    );

    let placed = scroll_after_reflow(
        false,
        Some(anchor_index(&after.meta, anchor)),
        scroll,
        after.lines.len(),
        INNER,
    );
    let landed = after.meta[placed];
    assert_eq!(
        (landed.entry, landed.block),
        (anchor.entry, anchor.block),
        "reader was moved off their turn: {anchor:?} -> {landed:?}"
    );
    assert!(
        !at_bottom(placed, after.lines.len(), INNER),
        "a reader in the middle of the log must not end up at the live tail"
    );
}

/// もう半分のケース: 最新のターンに乗っていた人は、その後も乗っていなければならない。
/// さもないと、この修正は1つの壊れたケースを別の壊れたケースに置き換えただけになる。
#[test]
fn 狭くしても追従中は最新のターンに留まる() {
    const INNER: usize = 20;
    let entries = reflow_fixture();

    let before = build_at(&entries, 80);
    let scroll = before.lines.len().saturating_sub(INNER);
    assert!(at_bottom(scroll, before.lines.len(), INNER));
    let anchor = before.meta[scroll];

    let after = build_at(&entries, 50);
    let placed = scroll_after_reflow(
        true,
        Some(anchor_index(&after.meta, anchor)),
        scroll,
        after.lines.len(),
        INNER,
    );

    assert!(at_bottom(placed, after.lines.len(), INNER));
    assert_eq!(
        placed + INNER,
        after.lines.len(),
        "the last built line must sit on the last visual row"
    );
    // そしてアンカーだけに従っていたら、最新の行は画面外に置かれてしまっていた —
    // だからこそ follow がそれを上書きする。
    let anchored_only = anchor_index(&after.meta, anchor);
    assert!(
        anchored_only < placed,
        "fixture does not exercise the follow override"
    );
}

/// パネルを広げるのは鏡合わせのケースである: 折り返し行が減り、追従している読者は
/// 末尾より手前に取り残されてはならない。
#[test]
fn 広くしても追従中は最新のターンに留まる() {
    const INNER: usize = 20;
    let entries = reflow_fixture();

    let before = build_at(&entries, 50);
    let scroll = before.lines.len().saturating_sub(INNER);
    let anchor = before.meta[scroll];

    let after = build_at(&entries, 100);
    assert!(after.lines.len() < before.lines.len());

    let placed = scroll_after_reflow(
        true,
        Some(anchor_index(&after.meta, anchor)),
        scroll,
        after.lines.len(),
        INNER,
    );
    assert_eq!(placed + INNER, after.lines.len());
}
