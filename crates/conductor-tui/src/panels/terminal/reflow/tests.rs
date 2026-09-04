//! 旧 src/reflow/ のテストの移植。振り分けは docs/rewrite-ports/reflow.md。

use conductor_core::claude_log::{CountedBucket, DisplayBlock, LogEntry, ResultKind, Role};
use conductor_core::config::Config;
use conductor_core::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::{Backend, CrosstermBackend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::{Frame, Terminal};
use serde_json::json;
use unicode_width::UnicodeWidthStr;

use super::build::{Built, MAX_GUTTER_COL, build};
use super::render::{JUMP_LABELS, anchor_index, badge};
use super::style::{
    ASSISTANT_MARKER, INACTIVE, MARKER_COLS, TEAMMATE_GLYPH, THINKING_GLYPH, TOOL_RESULT_GLYPH,
    USER_BG, USER_MARKER, USER_MARKER_FG, USER_TEXT, is_width_ambiguous, pad_glyph_to, with_marker,
};
use super::wrap::{pad_to_width, render_user_text, wrap_plain_text};
use super::*;

/// 構文定義の構築は重いので 1 度だけ。テストは読むだけで書き換えない。
fn highlighter() -> &'static Highlighter {
    static ONCE: std::sync::OnceLock<Highlighter> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| Highlighter::new(&Config::default()))
}

fn built(entries: &[LogEntry], expanded: bool, width: usize) -> Built {
    let theme = Theme::default();
    let ctx = build::Ctx {
        theme: &theme,
        highlighter: highlighter(),
        expanded,
    };
    build(&ctx, entries, width)
}

fn lines_of(entries: &[LogEntry], expanded: bool) -> Vec<Line<'static>> {
    built(entries, expanded, 80).lines
}

fn text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// 空行 (エントリの区切り) を落とした本文。大半のテストが見たいのはこの形。
fn visible(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(text)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn only_line<'a>(lines: &'a [Line<'a>]) -> &'a Line<'a> {
    let shown: Vec<&Line<'_>> = lines
        .iter()
        .filter(|l| !text(l).trim().is_empty())
        .collect();
    assert_eq!(shown.len(), 1, "expected one visible line, got {shown:?}");
    shown[0]
}

fn entry(role: Role, blocks: Vec<DisplayBlock>) -> LogEntry {
    LogEntry { role, blocks }
}

fn tool_use(name: &str, input: serde_json::Value) -> DisplayBlock {
    DisplayBlock::ToolUse {
        name: name.to_string(),
        input,
        errored: false,
    }
}

fn tool_result(kind: ResultKind, lines: &[&str], is_error: bool) -> DisplayBlock {
    DisplayBlock::ToolResult {
        kind,
        lines: lines.iter().map(|s| s.to_string()).collect(),
        is_error,
    }
}

fn counted(bucket: CountedBucket, from_bash: bool) -> ResultKind {
    ResultKind::Counted { bucket, from_bash }
}

/// (kind, is_error) ごとに 1 件ずつ結果を持つユーザのエントリ。
fn results(kinds: &[(ResultKind, bool)]) -> LogEntry {
    entry(
        Role::User,
        kinds
            .iter()
            .map(|(k, e)| tool_result(*k, &["out"], *e))
            .collect(),
    )
}

fn annotation(text: &str) -> DisplayBlock {
    DisplayBlock::Annotation {
        lines: vec![text.to_string()],
    }
}

// スクロールの算術

#[test]
fn クランプは範囲に収める() {
    // (scroll, total, inner, 期待値)。上限は total - inner で、ログが区画に収まれば 0。
    let cases = [
        (0, 100, 20, 0),
        (200, 100, 20, 80),
        (80, 100, 20, 80),
        (40, 100, 20, 40),
        (5, 10, 20, 0),
        (1, 20, 20, 0),
        (0, 0, 20, 0),
    ];
    for (scroll, total, inner, want) in cases {
        assert_eq!(
            clamp_scroll(scroll, total, inner),
            want,
            "clamp({scroll}, {total}, {inner})"
        );
    }
}

#[test]
fn 最下部は上限に達しているかで決まる() {
    let cases = [
        (80, 100, 20, true),
        (90, 100, 20, true),
        (79, 100, 20, false),
        (0, 10, 20, true),
    ];
    for (scroll, total, inner, want) in cases {
        assert_eq!(
            at_bottom(scroll, total, inner),
            want,
            "at_bottom({scroll}, {total}, {inner})"
        );
    }
    // 固定位置はクランプの上限と一致する。
    let pinned = bottom_scroll(150, 30);
    assert_eq!(pinned, 120);
    assert_eq!(clamp_scroll(pinned, 150, 30), pinned);
    assert!(at_bottom(pinned, 150, 30));
}

#[test]
fn 追従はジオメトリが動いても最下部へ戻る() {
    // 幅が狭まって行が増えた: anchor をそのまま使うと最新の行が画面外へ落ちる。
    assert_eq!(scroll_after_reflow(true, Some(80), 80, 140, 20), 120);
    // 高さだけの変更: 組み直しが無いので anchored は None。クランプだけでは足りない。
    assert_eq!(scroll_after_reflow(true, None, 80, 100, 10), 90);
    // ログが区画に収まるなら先頭。
    assert_eq!(scroll_after_reflow(true, None, 0, 10, 40), 0);
    for (total, inner) in [(100usize, 20usize), (10, 40), (0, 5), (41, 7)] {
        let placed = scroll_after_reflow(true, None, 0, total, inner);
        assert!(
            at_bottom(placed, total, inner),
            "total={total} inner={inner}"
        );
    }
}

#[test]
fn 離れて読む人は固定した行に着地する() {
    assert_eq!(scroll_after_reflow(false, Some(57), 40, 200, 20), 57);
    // 組み直しが無ければ位置はそのまま。
    assert_eq!(scroll_after_reflow(false, None, 40, 200, 20), 40);
    // 縮んだブロックは末尾を越えて解決しうる。回り込まずクランプする。
    assert_eq!(scroll_after_reflow(false, Some(9_999), 40, 200, 20), 180);
    // 短いログはどちらでも先頭。
    assert_eq!(scroll_after_reflow(false, Some(5), 3, 10, 40), 0);
}

#[test]
fn 離れて読む人は最下部へ引きずられない() {
    let (total, inner) = (300usize, 25usize);
    let bottom = bottom_scroll(total, inner);
    for anchored in [Some(0), Some(11), Some(120), None] {
        let placed = scroll_after_reflow(false, anchored, 33, total, inner);
        assert!(
            placed < bottom,
            "anchored={anchored:?} landed on the live tail"
        );
    }
}

#[test]
fn 遷移は単調で単位区間に収まり両端は厳密() {
    assert_eq!(eased(0.0), 0.0);
    assert_eq!(eased(1.0), 1.0);
    // 3(0.5)^2 - 2(0.5)^3 = 0.5。対称な曲線は中心を通る。
    assert!((eased(0.5) - 0.5).abs() < 1e-10);
    assert_eq!(eased(-0.5), 0.0);
    assert_eq!(eased(1.5), 1.0);

    let mut prev = 0.0;
    for i in 0..=100 {
        let v = eased(f64::from(i) / 100.0);
        assert!((0.0..=1.0).contains(&v), "eased out of range: {v}");
        assert!(v >= prev - 1e-12, "eased must not decrease");
        prev = v;
    }
}

#[test]
fn 始めたばかりのスイープはほぼ0() {
    assert!(progress(std::time::Instant::now()) < 0.1);
}

// マーカーと字下げ

#[test]
fn 字形は目標幅まで詰め既に広ければ触らない() {
    assert_eq!(pad_glyph_to(">", 2), "> ");
    assert_eq!(pad_glyph_to("=>", 2), "=>");
    assert_eq!(pad_glyph_to("abc", 2), "abc");
    assert_eq!(
        UnicodeWidthStr::width(pad_glyph_to(ASSISTANT_MARKER, MARKER_COLS).as_str()),
        MARKER_COLS
    );
}

/// 全行の本文予算が width - MARKER_COLS で決まるので、ここが 1 を超えると各行が
/// 1 カラムぶん足りなくなり最後の文字がはみ出す。「1 と測れる」ことと「1 として
/// 描かれる」ことは別で、後者は width_risk_hole が引き受ける。
#[test]
fn ガターの字形はちょうど1カラムと測れる() {
    for glyph in [
        ASSISTANT_MARKER,
        TOOL_RESULT_GLYPH,
        THINKING_GLYPH,
        USER_MARKER,
        TEAMMATE_GLYPH,
    ] {
        assert_eq!(UnicodeWidthStr::width(glyph), 1, "{glyph:?}");
    }
}

#[test]
fn マーカーは先頭行だけに付き継続行は空の字下げ() {
    let style = Style::default().fg(Color::Green);
    let two = with_marker(vec![Line::from("hello"), Line::from("world")], ">", style);
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].spans[0].content, "> ");
    assert_eq!(two[1].spans[0].content, "  ");

    let one = with_marker(vec![Line::from("only")], ">", style);
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].spans[0].content, "> ");

    assert!(with_marker(vec![], ">", style).is_empty());
}

// ユーザのターンの折り返しと背景

#[test]
fn 素朴な折り返しは単語と元の改行を守る() {
    let cases: [(&str, usize, &[&str]); 4] = [
        ("hello world", 20, &["hello world"]),
        ("one two three four", 9, &["one two", "three", "four"]),
        (
            "first line\nsecond line",
            40,
            &["first line", "second line"],
        ),
        ("a\n\nb", 10, &["a", "", "b"]),
    ];
    for (text, width, want) in cases {
        assert_eq!(wrap_plain_text(text, width), want, "{text:?} @ {width}");
    }
}

#[test]
fn 長すぎる単語はカラム境界で割る() {
    assert_eq!(
        wrap_plain_text("supercalifragilistic", 5),
        vec!["super", "calif", "ragil", "istic"]
    );
    let chunks = wrap_plain_text(&"W".repeat(150), 57);
    let widths: Vec<usize> = chunks.iter().map(|c| c.chars().count()).collect();
    assert_eq!(widths, vec![57, 57, 36]);
}

/// 1 文字ずつの合計は文字列全体の幅とずれる (⚠ + U+FE0F は 2 カラムだがセレクタは幅 0)。
/// 書記素クラスタで歩かないと、行が予算より広くなるか絵文字が半分に割れる。
#[test]
fn 割り方は書記素クラスタを壊さない() {
    let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
    let warn = "\u{26a0}\u{fe0f}";
    assert_eq!(UnicodeWidthStr::width(warn), 2);
    for (source, budget) in [
        ("あ".repeat(5), 5),
        (warn.repeat(5), 4),
        (family.repeat(3), 4),
    ] {
        let wrapped = wrap_plain_text(&source, budget);
        assert_eq!(wrapped.concat(), source, "a cluster was dropped");
        for line in &wrapped {
            assert!(
                UnicodeWidthStr::width(line.as_str()) <= budget,
                "{line:?} exceeds {budget}"
            );
        }
    }
}

#[test]
fn 詰めは目標幅までで既に広ければ触らない() {
    assert_eq!(pad_to_width("hi", 5), "hi   ");
    assert_eq!(pad_to_width("hello", 5), "hello");
    assert_eq!(pad_to_width("hello world", 5), "hello world");
}

fn user_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    render_user_text(
        text,
        width,
        USER_MARKER,
        Style::default().fg(USER_MARKER_FG).bg(USER_BG),
        Style::default().fg(USER_TEXT).bg(USER_BG),
    )
}

#[test]
fn ユーザの行は全幅を背景で埋めマーカーは先頭だけ() {
    let width = 20;
    for lines in [
        user_lines("one two three four five six seven", 12),
        user_lines("first\nsecond", width),
    ] {
        assert!(lines.len() > 1);
        assert_eq!(lines[0].spans[0].content, "\u{276f} ");
        assert_eq!(lines[1].spans[0].content, "  ");
    }
    for line in &user_lines("short", width) {
        let cols: usize = line
            .spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(cols, width, "背景が行全体に届かない: {line:?}");
        for span in &line.spans {
            assert_eq!(span.style.bg, Some(USER_BG));
        }
    }
}

/// 折り返し幅は width - MARKER_COLS。ここが変わると本文の予算が黙ってずれる。
#[test]
fn ユーザの本文はマーカーの幅を引いた幅で折り返す() {
    assert_eq!(MARKER_COLS, 2);
    assert!(user_lines("abcdefgh ijkl", 10).len() >= 2);
}

// ブロックの描画

#[test]
fn 空のログは何も出さない() {
    let empty = built(&[], false, 80);
    assert!(empty.lines.is_empty() && empty.meta.is_empty());
}

#[test]
fn 集計対象の結果はエントリごとに1行へ畳まれる() {
    // 節の順序・単複・先頭の動詞だけ大文字・シェル由来のフォールバック。全部実測。
    let cases: [(&[(ResultKind, bool)], &str); 8] = [
        (
            &[
                (counted(CountedBucket::Read, false), false),
                (counted(CountedBucket::Read, false), false),
                (counted(CountedBucket::Read, false), false),
            ],
            "Read 3 files",
        ),
        (
            &[(counted(CountedBucket::Read, false), false)],
            "Read 1 file",
        ),
        (
            &[
                (counted(CountedBucket::Search, false), false),
                (counted(CountedBucket::Search, false), false),
            ],
            "Searched for 2 patterns",
        ),
        (
            &[
                (counted(CountedBucket::List, true), false),
                (counted(CountedBucket::List, true), false),
            ],
            "Listed 2 directories",
        ),
        (
            &[
                (counted(CountedBucket::List, true), false),
                (counted(CountedBucket::List, true), false),
                (counted(CountedBucket::Search, false), false),
                (counted(CountedBucket::Read, false), false),
            ],
            "Searched for 1 pattern, read 1 file, listed 2 directories",
        ),
        (
            &[
                (counted(CountedBucket::List, true), false),
                (counted(CountedBucket::Read, false), false),
            ],
            "Read 1 file, listed 1 directory",
        ),
        // 失敗した Read でもエラー表示なしの普通のサマリに畳まれる。
        (
            &[(counted(CountedBucket::Read, false), true)],
            "Read 1 file",
        ),
        // 対応するネイティブのツールが 1 件でも寄与すると、シェル側の近似は捨てられる。
        (
            &[
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, false), false),
            ],
            "Read 1 file",
        ),
    ];
    for (kinds, want) in cases {
        assert_eq!(
            visible(&lines_of(&[results(kinds)], false)),
            vec![format!("{want} (ctrl+o to expand)")],
            "for {kinds:?}"
        );
    }
}

#[test]
fn シェルのcatはreadが無いときだけ数える() {
    let cases: [(&[(ResultKind, bool)], &str); 3] = [
        (
            &[(counted(CountedBucket::Read, true), false)],
            "Read 1 file",
        ),
        (
            &[
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, true), false),
            ],
            "Read 2 files",
        ),
        (
            &[
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, false), false),
                (counted(CountedBucket::Read, false), false),
            ],
            "Read 2 files",
        ),
    ];
    for (kinds, want) in cases {
        assert_eq!(
            visible(&lines_of(&[results(kinds)], false)),
            vec![format!("{want} (ctrl+o to expand)")],
            "for {kinds:?}"
        );
    }
}

#[test]
fn 集計の行はエラー色を持たず件数だけが太字() {
    let lines = lines_of(
        &[results(&[(counted(CountedBucket::Read, false), true)])],
        false,
    );
    let line = only_line(&lines);
    for span in &line.spans {
        assert_ne!(span.style, Style::default().fg(super::style::ERROR));
    }
}

#[test]
fn インラインの呼び出しは別名で名前が太字_引数は本文色() {
    let lines = lines_of(
        &[entry(
            Role::Assistant,
            vec![tool_use("Edit", json!({"file_path": "/tmp/out.txt"}))],
        )],
        false,
    );
    assert_eq!(
        visible(&lines),
        vec![format!("{ASSISTANT_MARKER} Update(/tmp/out.txt)")]
    );

    let lines = lines_of(
        &[entry(
            Role::Assistant,
            vec![tool_use("Write", json!({"file_path": "/tmp/out.txt"}))],
        )],
        false,
    );
    let line = only_line(&lines);
    assert_eq!(line.spans.len(), 3, "marker + name + arg: {line:?}");
    assert_eq!(
        line.spans[0].style,
        Style::default().fg(super::style::SUCCESS)
    );
    assert_eq!(line.spans[1].content.as_ref(), "Write");
    assert!(line.spans[1].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(line.spans[2].content.as_ref(), "(/tmp/out.txt)");
    // 実測では引数は薄くならず本文と同色。
    assert_eq!(line.spans[2].style, Style::default().fg(super::style::TEXT));
}

#[test]
fn 展開表示では別名ではなく素のツール名を出す() {
    assert_eq!(
        visible(&lines_of(
            &[entry(
                Role::Assistant,
                vec![tool_use("Edit", json!({"file_path": "/tmp/out.txt"}))],
            )],
            true,
        )),
        vec![format!("{ASSISTANT_MARKER} Edit(/tmp/out.txt)")]
    );
}

#[test]
fn 失敗した呼び出しのマーカーはエラー色になる() {
    let lines = lines_of(
        &[entry(
            Role::Assistant,
            vec![DisplayBlock::ToolUse {
                name: "Bash".into(),
                input: json!({"command": "false"}),
                errored: true,
            }],
        )],
        false,
    );
    assert_eq!(
        only_line(&lines).spans[0].style,
        Style::default().fg(super::style::ERROR)
    );
}

#[test]
fn 隠す結果もエラーでないインラインの結果も何も描かない() {
    let cases = [
        entry(
            Role::Assistant,
            vec![tool_use("TodoWrite", json!({"todos": []}))],
        ),
        // is_error を持つ TodoWrite でも出力は 1 行も無かった。
        results(&[(ResultKind::Hidden, true)]),
        entry(
            Role::User,
            vec![tool_result(ResultKind::Inline, &["all good"], false)],
        ),
    ];
    for entry in cases {
        assert!(visible(&lines_of(&[entry], false)).is_empty());
    }
}

/// 失敗した Bash(false) の実測: 1 行目は col2 に ⎿、本文は "Error: " 付きで col5 から。
/// 継続行は接頭辞なしで col5 に揃う。
#[test]
fn インラインのエラーは複数行のブロックで描く() {
    let lines = lines_of(
        &[entry(
            Role::User,
            vec![tool_result(
                ResultKind::Inline,
                &[
                    "bash: command failed with exit code 1",
                    "second line of the error",
                ],
                true,
            )],
        )],
        false,
    );
    let shown: Vec<&Line<'_>> = lines
        .iter()
        .filter(|l| !text(l).trim().is_empty())
        .collect();
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert_eq!(
        text(shown[0]),
        format!("  {TOOL_RESULT_GLYPH}  Error: bash: command failed with exit code 1")
    );
    assert_eq!(shown[0].spans[0].content.as_ref(), " ");
    assert_ne!(
        shown[0].spans[0].style,
        Style::default().fg(super::style::ERROR)
    );
    assert_eq!(text(shown[1]), "     second line of the error");
    assert!(!text(shown[1]).contains("Error:"), "接頭辞は先頭行だけ");
}

#[test]
fn 展開表示では結果の行を上限なく全部出す() {
    let raw: Vec<String> = (0..12).map(|i| format!("line{i}")).collect();
    let refs: Vec<&str> = raw.iter().map(String::as_str).collect();
    let lines = lines_of(
        &[entry(
            Role::User,
            vec![tool_result(
                counted(CountedBucket::Read, false),
                &refs,
                false,
            )],
        )],
        true,
    );
    let shown = visible(&lines);
    assert_eq!(shown.len(), 12, "{shown:?}");
    for (i, line) in shown.iter().enumerate() {
        assert!(line.ends_with(&format!("line{i}")), "{line:?}");
    }
}

#[test]
fn thinkingは折りたたみで1行_展開で見出しと本文() {
    let entries = [entry(
        Role::Assistant,
        vec![DisplayBlock::Thinking {
            text: "let me reason".into(),
            duration_secs: 12,
        }],
    )];

    let lines = lines_of(&entries, false);
    let line = only_line(&lines);
    // 字形なしで col2 から。展開時の見出しが使う ✻ は出ない。
    assert_eq!(text(line), "  Thought for 12s (ctrl+o to expand)");
    for span in &line.spans {
        let is_duration = span.content.as_ref() == "12s";
        assert_eq!(
            span.style.add_modifier.contains(Modifier::BOLD),
            is_duration,
            "太字なのは時間だけ: {span:?}"
        );
        if !span.content.trim().is_empty() {
            assert_eq!(span.style.fg, Some(INACTIVE));
        }
    }

    let shown = visible(&lines_of(&entries, true));
    assert_eq!(shown[0], format!("{THINKING_GLYPH} Thinking\u{2026}"));
    assert!(
        shown.iter().any(|t| t.contains("let me reason")),
        "{shown:?}"
    );
    assert!(
        !shown.iter().any(|t| t.contains("Thought for")),
        "{shown:?}"
    );
}

#[test]
fn teammateは折りたたみで本文を見ず展開で本文を出す() {
    let entries = [entry(
        Role::User,
        vec![DisplayBlock::TeammateMessage {
            id: "alice".into(),
            body: "please review PR 42".into(),
        }],
    )];

    let lines = lines_of(&entries, false);
    let shown = visible(&lines);
    assert_eq!(
        shown,
        vec![format!(
            "{TEAMMATE_GLYPH} Message from @alice (ctrl+o to expand)"
        )]
    );
    assert!(!shown[0].contains("review"), "本文が要約に漏れている");
    for span in &only_line(&lines).spans {
        assert_eq!(span.style.fg, Some(INACTIVE));
        assert_eq!(
            span.style.bg, None,
            "ユーザのターンと違い背景ブロックは無い"
        );
    }

    let shown = visible(&lines_of(&entries, true));
    assert_eq!(shown[0], format!("{TEAMMATE_GLYPH} Message from @alice"));
    assert!(shown.iter().any(|t| t.contains("please review PR 42")));
}

#[test]
fn ユーザの本文は専用のマーカーと背景で_markdownを通さない() {
    let lines = lines_of(
        &[entry(
            Role::User,
            vec![DisplayBlock::Text("**not bold** # not a heading".into())],
        )],
        false,
    );
    let line = only_line(&lines);
    assert_eq!(line.spans[0].content, "\u{276f} ");
    assert_eq!(line.spans[0].style.fg, Some(USER_MARKER_FG));
    assert_eq!(line.spans[1].style.fg, Some(USER_TEXT));
    for span in &line.spans {
        assert_eq!(span.style.bg, Some(USER_BG), "{span:?}");
    }
    assert!(text(line).contains("**not bold** # not a heading"));
}

#[test]
fn ユーザの本文の改行はそれぞれ別の行になる() {
    assert_eq!(
        visible(&lines_of(
            &[entry(
                Role::User,
                vec![DisplayBlock::Text("first line\nsecond line".into())],
            )],
            false,
        )),
        vec!["\u{276f} first line", "second line"]
    );
}

/// 見えるものを出さないブロックは、自分の区切りの空行も出さない。
#[test]
fn 見える中身が無いエントリは余計な空行を出さない() {
    for silent in [
        tool_use("TodoWrite", json!({"todos": []})),
        tool_use("Read", json!({"file_path": "/a.txt"})),
    ] {
        let entries = [
            entry(Role::User, vec![DisplayBlock::Text("hello".into())]),
            entry(Role::Assistant, vec![silent]),
            entry(Role::User, vec![DisplayBlock::Text("world".into())]),
        ];
        let texts: Vec<String> = lines_of(&entries, false).iter().map(text).collect();
        let at = |needle: &str| texts.iter().position(|t| t.contains(needle)).unwrap();
        assert_eq!(at("world") - at("hello"), 2, "{texts:?}");
    }
}

/// /compact のまとまりを、再開した本物のトランスクリプトと同じ形で固定する。
/// 注釈だけのエントリの手前に区切りが入らないのがこの並びの要点。
#[test]
fn compactのまとまりは本物と同じ形になる() {
    let entries = [
        entry(Role::Assistant, vec![DisplayBlock::CompactBoundary]),
        entry(Role::User, vec![DisplayBlock::Text("/compact".into())]),
        entry(
            Role::User,
            vec![annotation("Compacted (ctrl+o to see full summary)")],
        ),
        entry(Role::User, vec![annotation("Read alpha.rs (42 lines)")]),
        entry(Role::User, vec![annotation("Referenced file beta.yml")]),
        entry(Role::Assistant, vec![DisplayBlock::Text("done".into())]),
    ];
    let rendered: Vec<String> = lines_of(&entries, false)
        .iter()
        .map(|l| text(l).trim_end().to_string())
        .collect();
    assert_eq!(
        rendered,
        vec![
            format!("{THINKING_GLYPH} Conversation compacted (ctrl+o for history)"),
            String::new(),
            format!("{USER_MARKER} /compact"),
            format!("  {TOOL_RESULT_GLYPH}  Compacted (ctrl+o to see full summary)"),
            format!("  {TOOL_RESULT_GLYPH}  Read alpha.rs (42 lines)"),
            format!("  {TOOL_RESULT_GLYPH}  Referenced file beta.yml"),
            String::new(),
            format!("{ASSISTANT_MARKER} done"),
            String::new(),
        ]
    );
}

#[test]
fn 注記は新しいターンを始めない() {
    let entries = [
        entry(Role::Assistant, vec![DisplayBlock::Text("reply".into())]),
        entry(Role::User, vec![annotation("Read delta.rs (13 lines)")]),
    ];
    let rendered: Vec<String> = lines_of(&entries, false)
        .iter()
        .map(|l| text(l).trim_end().to_string())
        .collect();
    assert_eq!(
        rendered,
        vec![
            format!("{ASSISTANT_MARKER} reply"),
            format!("  {TOOL_RESULT_GLYPH}  Read delta.rs (13 lines)"),
            String::new(),
        ]
    );
}

#[test]
fn お知らせはassistantの点を出す() {
    let lines = lines_of(
        &[entry(
            Role::User,
            vec![DisplayBlock::Notice(
                "Background command \"x\" completed (exit code 0)".into(),
            )],
        )],
        false,
    );
    assert_eq!(
        text(only_line(&lines)).trim_end(),
        format!("{ASSISTANT_MARKER} Background command \"x\" completed (exit code 0)")
    );
}

#[test]
fn 同じターンの中の2つの本文は分けて描く() {
    let rendered: Vec<String> = lines_of(
        &[entry(
            Role::User,
            vec![
                DisplayBlock::Text("my actual question".into()),
                DisplayBlock::Text("<system-reminder>note</system-reminder>".into()),
            ],
        )],
        false,
    )
    .iter()
    .map(|l| text(l).trim_end().to_string())
    .collect();
    assert_eq!(
        rendered,
        vec![
            format!("{USER_MARKER} my actual question"),
            String::new(),
            format!("{USER_MARKER} <system-reminder>note</system-reminder>"),
            String::new(),
        ]
    );
}

/// 実測: worktree の外を指すパスは切り詰められず、本文の下に揃えた継続行へ流れる。
/// 改行はパスの途中に入り、Read という動詞は自分の行の残りを保持する。
#[test]
fn 長い注記は省略ではなく折り返す() {
    let path = format!("../../../../private/tmp/{}/out.txt", "x".repeat(120));
    let shown = visible(&lines_of(
        &[entry(
            Role::User,
            vec![annotation(&format!("Read {path} (7 lines)"))],
        )],
        false,
    ));
    let raw: Vec<String> = lines_of(
        &[entry(
            Role::User,
            vec![annotation(&format!("Read {path} (7 lines)"))],
        )],
        false,
    )
    .iter()
    .map(|l| text(l).trim_end().to_string())
    .filter(|t| !t.is_empty())
    .collect();

    assert!(raw.len() > 1, "{raw:?}");
    assert!(
        raw[0].starts_with(&format!("  {TOOL_RESULT_GLYPH}  Read ../../")),
        "{:?}",
        raw[0]
    );
    for line in raw.iter().skip(1) {
        assert!(
            line.starts_with("     ") && !line.starts_with("      "),
            "継続行は col5 に揃う: {line:?}"
        );
    }
    let joined: String = shown.iter().map(|t| t.trim_start()).collect();
    assert!(joined.contains(&"x".repeat(120)), "パスが切られた");
    assert!(!joined.contains('\u{2026}'), "省略記号が出た");
}

#[test]
fn どの幅でも行はパネルに収まる() {
    let entries = [
        entry(Role::User, vec![annotation(&"p".repeat(300))]),
        entry(Role::User, vec![DisplayBlock::Notice("n".repeat(300))]),
        entry(Role::Assistant, vec![DisplayBlock::CompactBoundary]),
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
    for expanded in [false, true] {
        for width in [10usize, 20, 40, 80] {
            let out = built(&entries, expanded, width);
            assert_eq!(out.lines.len(), out.meta.len());
            for (line, meta) in out.lines.iter().zip(out.meta.iter()) {
                let cols = UnicodeWidthStr::width(text(line).as_str());
                assert!(
                    cols <= width,
                    "{cols} cols at width {width}: {:?}",
                    text(line)
                );
                if let Some(col) = meta.skip_col {
                    assert!(
                        (col as usize) <= MAX_GUTTER_COL + 1,
                        "穴が溝の外 (col {col}, width {width})"
                    );
                }
            }
        }
    }
}

// 未書き込みセル (幅の曖昧な字形の対策)

fn holes(entries: &[LogEntry], expanded: bool) -> Vec<Option<u16>> {
    built(entries, expanded, 60)
        .meta
        .into_iter()
        .map(|m| m.skip_col)
        .collect()
}

fn inline_result(body: &str) -> LogEntry {
    entry(
        Role::User,
        vec![tool_result(ResultKind::Inline, &[body], false)],
    )
}

/// 穴は字形の直後に置く。位置は固定ではない — ⎿ の結果行は col2 まで字下げされる。
#[test]
fn 穴は溝の字形の直後に開く() {
    let assistant = holes(
        &[entry(
            Role::Assistant,
            vec![DisplayBlock::Text("hello".into())],
        )],
        false,
    );
    assert_eq!(assistant.first().copied().flatten(), Some(1));
    assert_eq!(
        holes(&[inline_result("out")], true)
            .first()
            .copied()
            .flatten(),
        Some(3)
    );
}

/// 本文が幅 1 でない文字 (全角、ZWJ、異体字セレクタ、肌色修飾子、結合文字) を
/// 含んでいても、穴は溝の字形に属したまま動かない。
#[test]
fn 幅の広い本文でも穴はずれない() {
    for body in [
        "plain",
        "日本語の全角テキスト",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} family",
        "\u{26a0}\u{fe0f} warn",
        "\u{1f44b}\u{1f3fd} wave",
        "e\u{0301}\u{0301} combining",
    ] {
        assert_eq!(
            holes(&[inline_result(body)], true)
                .first()
                .copied()
                .flatten(),
            Some(3),
            "body {body:?}"
        );
    }
}

/// ユーザのターンは全幅の背景ブロックなので、未書き込みのセルは背景の切れ込みに見える。
/// そもそも ❯ は幅の曖昧な字形でもない。
#[test]
fn ユーザのターンには穴を空けない() {
    let holes = holes(
        &[entry(Role::User, vec![DisplayBlock::Text("hi".into())])],
        false,
    );
    assert!(holes.iter().all(Option::is_none), "{holes:?}");
}

#[test]
fn 本文中の字形には穴を空けない() {
    let text = format!("{} \u{23fa} tail", "word ".repeat(20));
    let holes = holes(
        &[entry(Role::Assistant, vec![DisplayBlock::Text(text)])],
        false,
    );
    assert_eq!(holes.first().copied().flatten(), Some(1), "{holes:?}");
    assert!(holes.iter().skip(1).all(Option::is_none), "{holes:?}");
}

/// 字形の定数を差し替えたときに、対策が黙って効かなくなるのを防ぐ番人。
#[test]
fn 幅の曖昧な字形は全部登録されている() {
    for glyph in [ASSISTANT_MARKER, TOOL_RESULT_GLYPH, THINKING_GLYPH] {
        let ch = glyph.chars().next().unwrap();
        assert!(is_width_ambiguous(ch), "{glyph:?} は溝に出るのに未登録");
    }
    for glyph in [USER_MARKER, TEAMMATE_GLYPH] {
        assert!(
            !is_width_ambiguous(glyph.chars().next().unwrap()),
            "{glyph:?}"
        );
    }
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

fn flush(prev: &Buffer, next: &Buffer) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut backend = CrosstermBackend::new(&mut out);
        backend.draw(prev.diff(next).into_iter()).unwrap();
    }
    out
}

/// 印を付けたセルを飛ばすと、バックエンドは字形から書き続けるのではなく絶対位置で
/// カラム 3 へ跳ぶ。飛ばさない側も見るのは、無条件に MoveTo が出ていた場合に
/// このテストが何も証明しなくなるため。飛ばしたセルは塗り直されないことも同時に固定する。
#[test]
fn 飛ばすと絶対位置指定になり古いセルは残る() {
    let mut prev = Buffer::empty(Rect::new(0, 0, 20, 1));
    prev[(1u16, 0u16)].set_char('X');

    const MOVE_TO_COL3: &[u8] = b"\x1b[1;3H";
    let without = flush(&prev, &one_row(ASSISTANT_MARKER, false));
    let with = flush(&prev, &one_row(ASSISTANT_MARKER, true));

    assert!(
        !without.windows(6).any(|w| w == MOVE_TO_COL3),
        "対照群は連続して書くはず: {:?}",
        String::from_utf8_lossy(&without)
    );
    assert!(
        with.windows(6).any(|w| w == MOVE_TO_COL3),
        "飛ばしたら絶対移動が要る: {:?}",
        String::from_utf8_lossy(&with)
    );
    assert!(
        !String::from_utf8_lossy(&with).contains('X'),
        "飛ばしたセルは誰も上書きしない"
    );
}

// 「最新へ」チップ

fn draw_badge(width: u16, height: u16, following: bool) -> (Option<Rect>, Buffer) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let area = Rect::new(0, 0, width, height);
    let mut hit = None;
    terminal
        .draw(|frame: &mut Frame| {
            hit = badge(area, following).map(|(rect, _)| rect);
            let reflow = detached(width, height, following);
            super::render::render(frame, area, &reflow);
        })
        .unwrap();
    (hit, terminal.backend().buffer().clone())
}

/// 追従が外れた状態のビュー。チップだけを見るので中身は 1 行でよい。
fn detached(width: u16, height: u16, following: bool) -> Reflow {
    let mut reflow = Reflow::opening("s".into());
    reflow.install(vec![entry(
        Role::Assistant,
        vec![DisplayBlock::Text("x".into())],
    )]);
    reflow.prepare(&Theme::default(), highlighter(), (height, width), false);
    reflow.follow = following;
    reflow
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

#[test]
fn 追従中はチップも当たり判定も出さない() {
    let (hit, buf) = draw_badge(40, 6, true);
    assert_eq!(hit, None);
    assert!(!screen_text(&buf).contains("(G)"), "{}", screen_text(&buf));
}

#[test]
fn 離れて読むときは右下にチップを描き矩形を返す() {
    let (hit, buf) = draw_badge(40, 6, false);
    let rect = hit.expect("戻る手段が要る");
    assert_eq!(rect.y, 5, "最終行に置く");
    assert_eq!(rect.x + rect.width, 40, "右端に接する");
    assert_eq!(rect.height, 1);
    assert_eq!(rect.width as usize, UnicodeWidthStr::width(JUMP_LABELS[0]));

    let text = screen_text(&buf);
    let last = text.lines().last().unwrap();
    assert!(last.contains("Jump to latest (G)"), "{last:?}");
}

#[test]
fn チップは幅に合わせて縮み最後は諦める() {
    let width_of = |w: u16| badge(Rect::new(0, 0, w, 3), false).map(|(rect, _)| rect.width);
    assert_eq!(width_of(21), Some(20));
    assert_eq!(width_of(20), Some(12));
    assert_eq!(width_of(12), Some(5));
    assert_eq!(width_of(5), None);
}

#[test]
fn チップのラベルは素のasciiで長い順() {
    let widths: Vec<usize> = JUMP_LABELS
        .iter()
        .map(|l| {
            assert!(l.is_ascii(), "{l:?}");
            assert_eq!(UnicodeWidthStr::width(*l), l.len());
            UnicodeWidthStr::width(*l)
        })
        .collect();
    assert!(widths.windows(2).all(|w| w[0] > w[1]), "{widths:?}");
}

// 幅をまたいだスクロール位置

/// 2 つの幅で折り返し位置が実際に変わるくらい長いログ。20 行の窓がその一部でしかない
/// 長さにしないと、4 分の 3 の位置が既に末尾になってアンカーではなくクランプで決まる。
fn long_log() -> Vec<LogEntry> {
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

const INNER: usize = 20;

#[test]
fn 狭くしても離れて読む人は同じターンに留まる() {
    let entries = long_log();
    let before = built(&entries, false, 80);
    let scroll = before.meta.len() * 3 / 4;
    let anchor = before.meta[scroll];

    let after = built(&entries, false, 50);
    assert!(
        after.lines.len() > before.lines.len(),
        "狭い幅で行が増えていない"
    );
    // 生の行番号をそのまま引き継ぐのが置き換えたい挙動。実際に誤りであることを見る。
    let naive = after.meta[scroll];
    assert_ne!(
        (naive.entry, naive.block, naive.offset),
        (anchor.entry, anchor.block, anchor.offset),
        "行番号が振り直されていない。テストが空振りする"
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
        "読み手がターンから外れた: {anchor:?} -> {landed:?}"
    );
    assert!(!at_bottom(placed, after.lines.len(), INNER));
}

#[test]
fn 幅が変わっても追従中は最新のターンに留まる() {
    let entries = long_log();
    for (from, to) in [(80usize, 50usize), (50, 100)] {
        let before = built(&entries, false, from);
        let scroll = before.lines.len().saturating_sub(INNER);
        let anchor = before.meta[scroll];

        let after = built(&entries, false, to);
        let anchored = anchor_index(&after.meta, anchor);
        let placed = scroll_after_reflow(true, Some(anchored), scroll, after.lines.len(), INNER);

        assert!(at_bottom(placed, after.lines.len(), INNER));
        assert_eq!(placed + INNER, after.lines.len(), "{from} -> {to}");
        if to < from {
            // アンカーだけに従うと最新の行が画面外に落ちる。follow がそれを上書きする。
            assert!(anchored < placed, "follow の上書きを踏んでいない");
        }
    }
}

// ビューの状態遷移

fn opened(entries: Vec<LogEntry>) -> Reflow {
    let mut reflow = Reflow::opening("session-a".into());
    reflow.install(entries);
    reflow.prepare(&Theme::default(), highlighter(), (INNER as u16, 60), false);
    reflow
}

fn press(reflow: &mut Reflow, code: KeyCode) -> Handled {
    reflow.key(KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn 追従中に行が届いても末尾に居続ける() {
    let mut reflow = opened(long_log());
    assert!(reflow.follow);
    let before = reflow.scroll;

    let mut longer = long_log();
    longer.push(entry(
        Role::Assistant,
        vec![DisplayBlock::Text("newest turn".into())],
    ));
    reflow.entries = longer;
    // 行が増えたので組み直す。
    reflow.needs_rebuild = true;
    reflow.prepare(&Theme::default(), highlighter(), (INNER as u16, 60), false);

    assert!(reflow.scroll > before, "末尾が伸びたのに位置が動いていない");
    assert!(at_bottom(reflow.scroll, reflow.lines.len(), INNER));
    let last = text(&reflow.lines[reflow.scroll + INNER - 1]);
    assert!(
        reflow.lines[reflow.scroll..]
            .iter()
            .any(|l| text(l).contains("newest turn")),
        "{last:?}"
    );
}

#[test]
fn 上へスクロールすると追従が外れチップで戻る() {
    let mut reflow = opened(long_log());
    let bottom = reflow.scroll;

    press(&mut reflow, KeyCode::Up);
    assert!(!reflow.follow, "上へ動いたら追従は外れる");
    assert!(reflow.scroll < bottom);
    assert!(
        reflow
            .badge_rect(Rect::new(0, 0, 60, INNER as u16))
            .is_some()
    );

    reflow.jump_to_latest();
    assert!(reflow.follow);
    assert_eq!(reflow.scroll, bottom);
    assert!(
        reflow
            .badge_rect(Rect::new(0, 0, 60, INNER as u16))
            .is_none()
    );
}

#[test]
fn 最下部でさらに下へ押すとライブへ戻る() {
    let mut reflow = opened(long_log());
    assert!(matches!(press(&mut reflow, KeyCode::Down), Handled::Close));
    assert!(matches!(press(&mut reflow, KeyCode::Esc), Handled::Close));

    press(&mut reflow, KeyCode::Home);
    assert_eq!(reflow.scroll, 0);
    assert!(matches!(
        press(&mut reflow, KeyCode::Down),
        Handled::Consumed
    ));
}

#[test]
fn 最下部でさらに下へ回すとライブへ戻る() {
    let mut reflow = opened(long_log());
    assert!(matches!(reflow.wheel(3), Handled::Close));

    assert!(matches!(reflow.wheel(-3), Handled::Consumed));
    assert!(!reflow.follow, "遡ったら追従は外れる");
    assert!(
        matches!(reflow.wheel(3), Handled::Consumed),
        "最下部に戻るまでは畳まない"
    );
    assert!(reflow.follow);
    assert!(matches!(reflow.wheel(3), Handled::Close));
}

#[test]
fn 知らないキーも消費する() {
    let mut reflow = opened(long_log());
    press(&mut reflow, KeyCode::Home);
    let before = reflow.scroll;
    assert!(matches!(
        press(&mut reflow, KeyCode::Char('x')),
        Handled::Consumed
    ));
    assert_eq!(reflow.scroll, before);
}

#[test]
fn 展開のトグルは組み直しを頼む() {
    let mut reflow = opened(vec![entry(
        Role::Assistant,
        vec![DisplayBlock::Thinking {
            text: "reasoning".into(),
            duration_secs: 3,
        }],
    )]);
    assert!(
        visible(&reflow.lines)
            .iter()
            .any(|t| t.contains("Thought for"))
    );

    reflow.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(reflow.needs_rebuild);
    reflow.prepare(&Theme::default(), highlighter(), (INNER as u16, 60), false);
    assert!(
        visible(&reflow.lines)
            .iter()
            .any(|t| t.contains("Thinking"))
    );
}

/// コーパススイープ: 本物のログに対して、幅ごとの不変条件だけを見る。手書きでは
/// 思いつかない入力 (壊れた UTF-8、ネストしたフェンス、数 MB の結果) が出てくる。
/// ログはリポジトリに入れない — パスもプロンプトも出力も含むため。
#[test]
fn 実際のトランスクリプトでもレイアウトの不変条件が保たれる() {
    let Some(dir) = std::env::var_os("CONDUCTOR_TRANSCRIPT_CORPUS") else {
        eprintln!("CONDUCTOR_TRANSCRIPT_CORPUS unset — skipping");
        return;
    };
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("corpus directory")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= 5 * 1024 * 1024))
        .collect();
    files.sort();
    files.truncate(30);
    assert!(!files.is_empty(), "corpus holds no .jsonl files");

    for path in &files {
        let entries = conductor_core::claude_log::load_session(path);
        if entries.is_empty() {
            continue;
        }
        for expanded in [false, true] {
            for width in [20usize, 40, 60, 80, 120, 200] {
                let out = built(&entries, expanded, width);
                assert_eq!(out.lines.len(), out.meta.len(), "{}", path.display());
                for (i, line) in out.lines.iter().enumerate() {
                    let cols = UnicodeWidthStr::width(text(line).as_str());
                    assert!(
                        cols <= width,
                        "{}: line {i} is {cols} cols at width {width}",
                        path.display()
                    );
                }
            }
        }
    }
}
