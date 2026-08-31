//! [super::build::build_lines] のテスト — セッションログの [LogEntry] 一覧を
//! トランスクリプトの行に変換する tool レンダリングパイプライン。手組みの
//! [BuildCtx] に対して直接検証しており、App は一切登場しない。BuildCtx を
//! 分離した狙いはまさにこれで、アプリケーション状態を構築しなくてもこの
//! パイプラインをテストできるようにするためである。
//!
//! 以下の tool-call のケースは crate::claude_log::tool_class の分類テーブルに
//! 対して検証している。このテーブルは Claude Code 自身のトランスクリプトの
//! 生バイトキャプチャから再構成したものであり、推測ではない。

use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use serde_json::json;
use syntect::highlighting::ThemeSet;

use crate::reflow::log::{CountedBucket, DisplayBlock, LogEntry, ResultKind, Role};
use crate::ui::markdown::MarkdownCache;

use super::build::{BuildCtx, build_lines};
use super::glyphs::{ASSISTANT_MARKER, THINKING_GLYPH, TOOL_RESULT_GLYPH, USER_MARKER};
use super::palette;

fn fixtures() -> (
    crate::theme::Theme,
    syntect::parsing::SyntaxSet,
    syntect::highlighting::Theme,
) {
    let theme = crate::theme::Theme::default();
    let syntax_set = two_face::syntax::extra_newlines();
    let syntect_theme = ThemeSet::load_defaults()
        .themes
        .remove("base16-ocean.dark")
        .unwrap();
    (theme, syntax_set, syntect_theme)
}

/// build_lines は借用したフィクスチャだけで呼び出せなければならない — このテストの
/// どこにも App は構築されない。空の entry リストは退化しているが有効な入力である。
#[test]
fn 行の構築はappを立てずに走る() {
    let (theme, syntax_set, syntect_theme) = fixtures();
    let cache = MarkdownCache::new();
    let entries: Vec<LogEntry> = Vec::new();
    let ctx = BuildCtx {
        entries: &entries,
        cache: &cache,
        theme: &theme,
        syntax_set: &syntax_set,
        syntect_theme: &syntect_theme,
        expanded: false,
    };

    let built = build_lines(&ctx, 80);

    assert!(built.lines.is_empty());
    assert!(built.meta.is_empty());
}

// 以下の tool-call レンダリングテーブル用フィクスチャヘルパー

fn tool_use(name: &str, input: serde_json::Value) -> DisplayBlock {
    tool_use_errored(name, input, false)
}

fn tool_use_errored(name: &str, input: serde_json::Value, errored: bool) -> DisplayBlock {
    DisplayBlock::ToolUse {
        name: name.to_string(),
        input,
        errored,
    }
}

/// kind は解決済みのペアリングマップの値。ここで解決済みにしておくことで、これらのテストは
/// build_lines の描画規則だけを見て、ペアリングマップを再テストしないで済む。
fn tool_result(kind: ResultKind, lines: &[&str], is_error: bool) -> DisplayBlock {
    DisplayBlock::ToolResult {
        kind,
        lines: lines.iter().map(|s| s.to_string()).collect(),
        is_error,
    }
}

fn thinking(text: &str, duration_secs: u64) -> DisplayBlock {
    DisplayBlock::Thinking {
        text: text.to_string(),
        duration_secs,
    }
}

fn teammate_message(id: &str, body: &str) -> DisplayBlock {
    DisplayBlock::TeammateMessage {
        id: id.to_string(),
        body: body.to_string(),
    }
}

fn entry(role: Role, blocks: Vec<DisplayBlock>) -> LogEntry {
    LogEntry { role, blocks }
}

fn build(entries: &[LogEntry], expanded: bool) -> Vec<Line<'static>> {
    let (theme, syntax_set, syntect_theme) = fixtures();
    let cache = MarkdownCache::new();
    let ctx = BuildCtx {
        entries,
        cache: &cache,
        theme: &theme,
        syntax_set: &syntax_set,
        syntect_theme: &syntect_theme,
        expanded,
    };
    build_lines(&ctx, 80).lines
}

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// レンダリングされたすべての行のテキストを trim し、空行（entry の区切り）を
/// 除いたもの — これらのテストの大半が関心を持つ形である。
fn non_blank_texts(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(line_text)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// ちょうど 1 つでなければ panic する。スタイル検証のテストは、テキストではなく実際の
/// Line が要るのでこれを通す。
fn only_visible_line<'a>(lines: &'a [Line<'a>]) -> &'a Line<'a> {
    let visible: Vec<&Line<'_>> = lines
        .iter()
        .filter(|l| !line_text(l).trim().is_empty())
        .collect();
    assert_eq!(
        visible.len(),
        1,
        "expected exactly one visible line, got {visible:?}"
    );
    visible[0]
}

// Counted カテゴリ: 1つのサマリー行への集約

#[test]
fn readの結果は件数の1行にまとまる() {
    let entries = vec![
        entry(
            Role::Assistant,
            vec![tool_use("Read", json!({"file_path": "/a"}))],
        ),
        results_entry(&[
            (counted(CountedBucket::Read, false), false),
            (counted(CountedBucket::Read, false), false),
            (counted(CountedBucket::Read, false), false),
        ]),
    ];
    assert_eq!(
        non_blank_texts(&build(&entries, false)),
        vec!["Read 3 files (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn 結果が1件のときは単数形になる() {
    let entries = vec![results_entry(&[(
        counted(CountedBucket::Read, false),
        false,
    )])];
    assert_eq!(
        non_blank_texts(&build(&entries, false)),
        vec!["Read 1 file (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn 検索の結果はパターンの要約1行にまとまる() {
    let entries = vec![results_entry(&[
        (counted(CountedBucket::Search, false), false),
        (counted(CountedBucket::Search, false), false),
    ])];
    assert_eq!(
        non_blank_texts(&build(&entries, false)),
        vec!["Searched for 2 patterns (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn bashのlsは一覧の要約にまとまる() {
    let entries = vec![results_entry(&[
        (counted(CountedBucket::List, true), false),
        (counted(CountedBucket::List, true), false),
    ])];
    assert_eq!(
        non_blank_texts(&build(&entries, false)),
        vec!["Listed 2 directories (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn 集計対象はis_errorを無視して普通にまとまる() {
    // 実測: Counted は is_error を無視する。失敗した Read でもエラー色は付かず、
    // 普通のサマリー行へ折り込まれる。
    let entries = vec![results_entry(&[(
        counted(CountedBucket::Read, false),
        true,
    )])];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);

    assert_eq!(line_text(line).trim(), "Read 1 file (ctrl+o to expand)");
    for span in &line.spans {
        assert_ne!(span.style, Style::default().fg(palette::ERROR));
    }
}

// Inline カテゴリ: 呼び出しごとの ⏺ Name(arg) 行

#[test]
fn editはupdateという表示名にまとまる() {
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use("Edit", json!({"file_path": "/tmp/out.txt"}))],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec![format!("{ASSISTANT_MARKER} Update(/tmp/out.txt)")]
    );
}

#[test]
fn インラインの呼び出しは名前が太字で引数は本文色() {
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use("Write", json!({"file_path": "/tmp/out.txt"}))],
    )];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);

    assert_eq!(
        line.spans.len(),
        3,
        "marker + name + arg spans, got {line:?}"
    );
    assert_eq!(line.spans[0].style, Style::default().fg(palette::SUCCESS));
    assert_eq!(line.spans[1].content.as_ref(), "Write");
    assert_eq!(
        line.spans[1].style,
        Style::default()
            .fg(palette::TEXT)
            .add_modifier(Modifier::BOLD)
    );
    assert_eq!(line.spans[2].content.as_ref(), "(/tmp/out.txt)");
    // ネイティブのキャプチャでは引数は暗く表示されるのではなく本文テキストと
    // 同じ色で表示される — これは palette::INACTIVE であってはならない。
    assert_eq!(line.spans[2].style, Style::default().fg(palette::TEXT));
    assert_ne!(line.spans[2].style, Style::default().fg(palette::INACTIVE));
}

// Hidden カテゴリ: どちらの位置でも何も描画しない

#[test]
fn 折りたたみ表示ではtodowriteは何も描かない() {
    let entries = vec![
        entry(
            Role::Assistant,
            vec![tool_use("TodoWrite", json!({"todos": []}))],
        ),
        entry(
            Role::User,
            vec![tool_result(ResultKind::Inline, &["ok"], false)],
        ),
    ];
    let lines = build(&entries, false);
    assert!(non_blank_texts(&lines).is_empty());
}

// エラー: 結果行を描画するのは Inline カテゴリだけ。Counted は上で
// カバー済み（is_error を完全に無視する）

#[test]
fn インラインのエラーは複数行のブロックで描く() {
    // 失敗した Bash(false) 呼び出しの実測カラムレイアウト: ⎿ は col2、本文は
    // （先頭に "Error: " を付けて）1行目は col4 から、継続行は col5 から。
    // "Error: " のプレフィックスは最初の行にしか付かない。
    let entries = vec![entry(
        Role::User,
        vec![tool_result(
            ResultKind::Inline,
            &[
                "bash: command failed with exit code 1",
                "second line of the error",
            ],
            true,
        )],
    )];
    let lines = build(&entries, false);
    let visible: Vec<&Line<'_>> = lines
        .iter()
        .filter(|l| !line_text(l).trim().is_empty())
        .collect();
    assert_eq!(
        visible.len(),
        2,
        "first + continuation error line, got {visible:?}"
    );

    let first = visible[0];
    assert_eq!(
        line_text(first),
        format!("  {TOOL_RESULT_GLYPH}  Error: bash: command failed with exit code 1")
    );
    assert_eq!(first.spans[0].content.as_ref(), " ");
    assert_ne!(first.spans[0].style, Style::default().fg(palette::ERROR));
    assert_eq!(first.spans[1].style, Style::default().fg(palette::ERROR));
    assert_eq!(first.spans[2].style, Style::default().fg(palette::ERROR));

    let cont = visible[1];
    assert_eq!(line_text(cont), "     second line of the error");
    assert!(
        !line_text(cont).contains("Error:"),
        "only the first error line gets the \"Error: \" prefix"
    );
    for span in &cont.spans {
        if !span.content.trim().is_empty() {
            assert_eq!(span.style, Style::default().fg(palette::ERROR));
        }
    }
}

#[test]
fn エラーでないインラインの結果は何も描かない() {
    let entries = vec![entry(
        Role::User,
        vec![tool_result(ResultKind::Inline, &["all good"], false)],
    )];
    let lines = build(&entries, false);
    assert!(non_blank_texts(&lines).is_empty());
}

#[test]
fn 失敗した呼び出しのマーカーはエラー色になる() {
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use_errored("Bash", json!({"command": "false"}), true)],
    )];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);

    assert_eq!(line.spans[0].style, Style::default().fg(palette::ERROR));
    assert_ne!(line.spans[0].style, Style::default().fg(palette::SUCCESS));
}

// Thinking ブロック: 折り畳み時の1行 vs 展開時のヘッダー+本文

#[test]
fn thinkingは折りたたみで1行の要約になる() {
    let entries = vec![entry(Role::Assistant, vec![thinking("let me reason", 12)])];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec!["Thought for 12s (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn 折りたたみのthinkingはグリフ無しで2桁目から始まる() {
    // 仕様: col2、グリフ無し — 展開時のヘッダーが使う * マーカーではなく、
    // ただの2スペースインデント。
    let entries = vec![entry(Role::Assistant, vec![thinking("let me reason", 3)])];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);
    assert_eq!(line_text(line), "  Thought for 3s (ctrl+o to expand)");
}

#[test]
fn 折りたたみのthinkingは時間だけを太字にする() {
    let entries = vec![entry(Role::Assistant, vec![thinking("let me reason", 12)])];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);

    let bold_span = line
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "12s")
        .expect("a span with exactly the duration text");
    assert!(bold_span.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(bold_span.style.fg, Some(palette::INACTIVE));

    for span in &line.spans {
        if span.content.as_ref() != "12s" {
            assert!(
                !span.style.add_modifier.contains(Modifier::BOLD),
                "only the duration span should be bold, got bold: {span:?}"
            );
            // 先頭のガターは空白でスタイルを持たない — ネイティブはそこに一切
            // 何も書かず、直接カラム3へジャンプする — そのため色の検証は
            // テキストの span だけに対して行う。
            if !span.content.trim().is_empty() {
                assert_eq!(span.style.fg, Some(palette::INACTIVE));
            }
        }
    }
}

#[test]
fn 展開したthinkingは見出しと本文をそのまま出す() {
    let entries = vec![entry(Role::Assistant, vec![thinking("let me reason", 12)])];
    let lines = build(&entries, true);
    let texts = non_blank_texts(&lines);
    assert_eq!(texts[0], format!("{THINKING_GLYPH} Thinking\u{2026}"));
    assert!(
        texts.iter().any(|t| t.contains("let me reason")),
        "expanded mode must still render the reasoning body: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("Thought for")),
        "expanded mode must not show the collapsed one-liner: {texts:?}"
    );
}

// Teammate-message ブロック: 折り畳み時のサマリー vs 展開時の本文

#[test]
fn teammateのメッセージは折りたたみで1行の要約になる() {
    let entries = vec![entry(
        Role::User,
        vec![teammate_message("alice", "please review PR 42")],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec!["\u{203a} Message from @alice (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn 折りたたみのteammateの行は操作対象にならない() {
    let entries = vec![entry(
        Role::User,
        vec![teammate_message("alice", "please review PR 42")],
    )];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);
    for span in &line.spans {
        assert_eq!(span.style.fg, Some(palette::INACTIVE));
        assert_eq!(
            span.style.bg, None,
            "spec: no background block, unlike user turns"
        );
    }
}

#[test]
fn 折りたたみのteammateは本文を一切見ない() {
    // 折り畳みモードでは id しかレンダリングされない — 本文テキストは短くても
    // サマリー行に漏れてはならない。
    let entries = vec![entry(Role::User, vec![teammate_message("alice", "hi")])];
    let lines = build(&entries, false);
    let texts = non_blank_texts(&lines);
    assert_eq!(texts.len(), 1);
    assert!(!texts[0].contains("hi"));
}

#[test]
fn 展開したteammateは見出しの後に本文を出す() {
    let entries = vec![entry(
        Role::User,
        vec![teammate_message("alice", "please review PR 42")],
    )];
    let lines = build(&entries, true);
    let texts = non_blank_texts(&lines);
    assert_eq!(texts[0], "\u{203a} Message from @alice");
    assert!(
        !texts[0].contains("ctrl+o to expand"),
        "expanded header must drop the collapsed-mode toggle hint: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("please review PR 42")),
        "expanded mode must render the full body: {texts:?}"
    );
}

// Expanded モード（conductor 独自の ctrl+o 相当のトグル）

#[test]
fn 展開表示では別名ではなく素のツール名を出す() {
    // 折り畳みモードは Edit を Update として表示するが、展開モードは各呼び出しを
    // 個別に描画するので、代わりに tool 自身の生の名前を表示しなければならない。
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use("Edit", json!({"file_path": "/tmp/out.txt"}))],
    )];
    let lines = build(&entries, true);
    assert_eq!(
        non_blank_texts(&lines),
        vec![format!("{ASSISTANT_MARKER} Edit(/tmp/out.txt)")]
    );
}

// user ターンはフル幅の背景ブロックとして描画される

#[test]
fn ユーザの本文はassistantの点ではなく専用のマーカーで描く() {
    let entries = vec![entry(
        Role::User,
        vec![DisplayBlock::Text("hi".to_string())],
    )];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);
    assert_eq!(line.spans[0].content, "\u{276f} ");
}

#[test]
fn ユーザの本文はマーカーも中身も背景色を持つ() {
    let entries = vec![entry(
        Role::User,
        vec![DisplayBlock::Text("hi".to_string())],
    )];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);
    for span in &line.spans {
        assert_eq!(
            span.style.bg,
            Some(palette::USER_BG),
            "span {span:?} missing background fill"
        );
    }
    assert_eq!(line.spans[0].style.fg, Some(palette::USER_MARKER_FG));
    assert_eq!(line.spans[1].style.fg, Some(palette::USER_TEXT));
}

#[test]
fn ユーザの本文はmarkdownとして描かない() {
    // user のプロンプト内の Markdown 構文は、文字通りの文字として描画されなければ
    // ならない — 太字や見出しなどのパースはしない。user 入力は文章ではなく生の
    // テキストであるため。
    let entries = vec![entry(
        Role::User,
        vec![DisplayBlock::Text(
            "**not bold** # not a heading".to_string(),
        )],
    )];
    let lines = build(&entries, false);
    let texts = non_blank_texts(&lines);
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].contains("**not bold** # not a heading"),
        "expected literal markdown syntax, got: {texts:?}"
    );
}

#[test]
fn ユーザの本文の改行はそれぞれ別の行になる() {
    let entries = vec![entry(
        Role::User,
        vec![DisplayBlock::Text("first line\nsecond line".to_string())],
    )];
    let lines = build(&entries, false);
    let texts = non_blank_texts(&lines);
    assert_eq!(texts, vec!["\u{276f} first line", "second line"]);
}

// 可視ブロックが1つも無い entry のために余計な空行は生まれない

#[test]
fn 見える中身が無いエントリは余計な空行を出さない() {
    // TodoWrite だけの entry（Hidden カテゴリ）が2つの可視テキストターンの間に
    // ある。それは何も生成してはならない — 自身の空行区切りすら含めて —
    // そのため "hello" と "world" の間には空行がちょうど1つあり、2つではない。
    let entries = vec![
        entry(Role::User, vec![DisplayBlock::Text("hello".to_string())]),
        entry(
            Role::Assistant,
            vec![tool_use("TodoWrite", json!({"todos": []}))],
        ),
        entry(Role::User, vec![DisplayBlock::Text("world".to_string())]),
    ];
    let lines = build(&entries, false);
    let texts: Vec<String> = lines.iter().map(line_text).collect();

    let hello_idx = texts
        .iter()
        .position(|t| t.contains("hello"))
        .expect("hello line present");
    let world_idx = texts
        .iter()
        .position(|t| t.contains("world"))
        .expect("world line present");
    assert_eq!(
        world_idx - hello_idx,
        2,
        "expected exactly one blank line between entries, got: {texts:?}"
    );
}

#[test]
fn 集計だけの呼び出しも余計な空行を出さない() {
    // Read の tool_use（Counted カテゴリ）は tool_use の位置には何も描画しない —
    // 代わりに集約されたサマリーが、対になる tool_result の位置に描画される
    // （上の Counted 集約テストを参照）。そのような呼び出しだけを持つ entry も
    // 空行区切りを生成してはならない。
    let entries = vec![
        entry(Role::User, vec![DisplayBlock::Text("hello".to_string())]),
        entry(
            Role::Assistant,
            vec![tool_use("Read", json!({"file_path": "/a.txt"}))],
        ),
        entry(Role::User, vec![DisplayBlock::Text("world".to_string())]),
    ];
    let lines = build(&entries, false);
    let texts: Vec<String> = lines.iter().map(line_text).collect();

    let hello_idx = texts
        .iter()
        .position(|t| t.contains("hello"))
        .expect("hello line present");
    let world_idx = texts
        .iter()
        .position(|t| t.contains("world"))
        .expect("world line present");
    assert_eq!(
        world_idx - hello_idx,
        2,
        "expected exactly one blank line between entries, got: {texts:?}"
    );
}

#[test]
fn 展開表示では結果の行を上限なく全部出す() {
    let raw_lines: Vec<String> = (0..12).map(|i| format!("line{i}")).collect();
    let raw_refs: Vec<&str> = raw_lines.iter().map(String::as_str).collect();
    let entries = vec![entry(
        Role::User,
        vec![tool_result(
            ResultKind::Counted {
                bucket: CountedBucket::Read,
                from_bash: false,
            },
            &raw_refs,
            false,
        )],
    )];
    let lines = build(&entries, true);
    let texts = non_blank_texts(&lines);

    assert_eq!(
        texts.len(),
        12,
        "no cap on expanded result lines: {texts:?}"
    );
    for (i, text) in texts.iter().enumerate() {
        assert!(
            text.ends_with(&format!("line{i}")),
            "line {i} should end with its own content, got {text:?}"
        );
    }
}

// Claude Code に対して実測した集約ルール

/// (kind, is_error) のペアごとに1つの結果を持つ user entry を組み立てる。
fn results_entry(kinds: &[(ResultKind, bool)]) -> LogEntry {
    LogEntry {
        role: Role::User,
        blocks: kinds
            .iter()
            .map(|(k, e)| tool_result(*k, &["out"], *e))
            .collect(),
    }
}

fn counted(bucket: CountedBucket, from_bash: bool) -> ResultKind {
    ResultKind::Counted { bucket, from_bash }
}

#[test]
fn 隠す結果は失敗しても何も描かない() {
    // 実測: is_error を持つ結果の TodoWrite は、ネイティブの出力を1行も
    // 生成しなかった。Hidden は失敗時も隠れたままである。
    let entries = vec![results_entry(&[(ResultKind::Hidden, true)])];
    assert!(non_blank_texts(&build(&entries, false)).is_empty());
}

#[test]
fn 複数のバケットは読点で繋いだ1行にまとまる() {
    // 実測: ls×2 + Grep + Read は1行としてレンダリングされ、節は
    // search -> read -> list の順で並び、先頭の動詞だけが大文字始まりになる。
    let entries = vec![results_entry(&[
        (counted(CountedBucket::List, true), false),
        (counted(CountedBucket::List, true), false),
        (counted(CountedBucket::Search, false), false),
        (counted(CountedBucket::Read, false), false),
    ])];
    assert_eq!(
        non_blank_texts(&build(&entries, false)),
        vec!["Searched for 1 pattern, read 1 file, listed 2 directories (ctrl+o to expand)"]
    );
}

#[test]
fn バケットの並びと大小は実測どおりに保つ() {
    // 実測: ls + Read は "Read 1 file, listed 1 directory" とレンダリングされる。
    let entries = vec![results_entry(&[
        (counted(CountedBucket::List, true), false),
        (counted(CountedBucket::Read, false), false),
    ])];
    assert_eq!(
        non_blank_texts(&build(&entries, false)),
        vec!["Read 1 file, listed 1 directory (ctrl+o to expand)"]
    );
}

#[test]
fn シェルのcatはreadが無いときだけ数える() {
    // Bash(cat ...) と Read の実測された5通りの組み合わせ。
    let cases: [(&[(ResultKind, bool)], &str); 5] = [
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
            ],
            "Read 1 file",
        ),
        (
            &[
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, false), false),
                (counted(CountedBucket::Read, false), false),
            ],
            "Read 2 files",
        ),
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
    for (kinds, expected) in cases {
        let entries = vec![results_entry(kinds)];
        assert_eq!(
            non_blank_texts(&build(&entries, false)),
            vec![format!("{expected} (ctrl+o to expand)")],
            "for {kinds:?}"
        );
    }
}

// Compact 境界 / annotation（実測、claude_log::tests を参照）

fn annotation(text: &str) -> DisplayBlock {
    DisplayBlock::Annotation {
        lines: vec![text.to_string()],
    }
}

/// /compact のグループ全体を、再開されたネイティブトランスクリプトがバイト単位で
/// 描くのと同じように検証する — コマンドとその annotation の間に空行が無いことも
/// 含めて。これが区切り抑制ルールの理由である。
#[test]
fn compactのまとまりは本物と同じ形になる() {
    let entries = vec![
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
    let rendered: Vec<String> = build(&entries, false)
        .iter()
        .map(|l| line_text(l).trim_end().to_string())
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
    // entry の後に通常付く空行区切りは、次の entry が annotation のみの場合は
    // 抑制される。そのため ⏺ reply と、CLI がそれに付随させた ⎿ 行は
    // くっついたままになる。
    let entries = vec![
        entry(Role::Assistant, vec![DisplayBlock::Text("reply".into())]),
        entry(Role::User, vec![annotation("Read delta.rs (13 lines)")]),
    ];
    let rendered: Vec<String> = build(&entries, false)
        .iter()
        .map(|l| line_text(l).trim_end().to_string())
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
    let entries = vec![entry(
        Role::User,
        vec![DisplayBlock::Notice(
            "Background command \"x\" completed (exit code 0)".into(),
        )],
    )];
    assert_eq!(
        line_text(only_visible_line(&build(&entries, false))).trim_end(),
        format!("{ASSISTANT_MARKER} Background command \"x\" completed (exit code 0)")
    );
}

#[test]
fn 長い注記もお知らせもパネルの中に収まる() {
    // どちらの形式も CLI から渡される長さに上限の無いテキストを持つ
    // （worktree の外を指す ../../.. パスは長くなる）ため、どちらも
    // クリップされなければならない。
    let entries = vec![
        entry(Role::User, vec![annotation(&"p".repeat(300))]),
        entry(Role::User, vec![DisplayBlock::Notice("n".repeat(300))]),
        entry(Role::Assistant, vec![DisplayBlock::CompactBoundary]),
    ];
    for width in [10usize, 20, 40, 80] {
        let (theme, syntax_set, syntect_theme) = fixtures();
        let cache = MarkdownCache::new();
        let ctx = BuildCtx {
            entries: &entries,
            cache: &cache,
            theme: &theme,
            syntax_set: &syntax_set,
            syntect_theme: &syntect_theme,
            expanded: false,
        };
        for line in build_lines(&ctx, width).lines {
            let w = unicode_width::UnicodeWidthStr::width(line_text(&line).as_str());
            assert!(
                w <= width,
                "{w} cols at width {width}: {:?}",
                line_text(&line)
            );
        }
    }
}

#[test]
fn 長い注記は省略ではなく折り返す() {
    // 実測: worktree の外から compact をまたいで持ち越されたファイルは、どんな
    // パネルにも収まらないほど長い ../../../… パスを持つことがある。Claude Code は
    // それを切り詰めるのではなく、本文の下に揃えた継続行へ流し込む。改行はパスの
    // 途中（カラム単位の強制分割）で起こり、Read という動詞は単独で行に残るの
    // ではなく、自分の行の残りを保持し続ける。
    let path = format!("../../../../private/tmp/{}/out.txt", "x".repeat(120));
    let entries = vec![entry(
        Role::User,
        vec![annotation(&format!("Read {path} (7 lines)"))],
    )];
    let lines = build(&entries, false);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| line_text(l).trim_end().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    assert!(texts.len() > 1, "expected a wrap, got {texts:?}");
    assert!(
        texts[0].starts_with(&format!("  {TOOL_RESULT_GLYPH}  Read ../../")),
        "the verb must keep its line: {:?}",
        texts[0]
    );
    for t in texts.iter().skip(1) {
        assert!(
            t.starts_with("     ") && !t.starts_with("      "),
            "continuations align under the body at col5: {t:?}"
        );
    }
    // 何も省略されない: パスのすべての文字がどこかに残っている。
    let joined: String = texts.iter().map(|t| t.trim_start()).collect();
    assert!(joined.contains(&"x".repeat(120)), "path was cut: {joined}");
    assert!(
        !joined.contains('\u{2026}'),
        "unexpected ellipsis: {joined}"
    );
}

#[test]
fn 同じターンの中の2つの本文は分けて描く() {
    // 実測: 例えばプロンプトと付加された <system-reminder> のように、2つの
    // テキストブロックを持つ user メッセージは、詰まった1組としてではなく、
    // 間に空行を挟んだ2つの ❯ ターンとして描画される。entry レベルの区切りは
    // entry 間でしか発生しないので、このテストは1つの entry の内側の隙間を
    // カバーする。
    let entries = vec![entry(
        Role::User,
        vec![
            DisplayBlock::Text("my actual question".into()),
            DisplayBlock::Text("<system-reminder>note</system-reminder>".into()),
        ],
    )];
    let rendered: Vec<String> = build(&entries, false)
        .iter()
        .map(|l| line_text(l).trim_end().to_string())
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
