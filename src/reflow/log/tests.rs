//! claude_log の単体テスト: ラッパータグの正規化、content からブロックへの
//! 変換、load_session の結合テスト。

use std::collections::{HashMap, HashSet};

use super::convert::{content_to_display_blocks, result_lines};
use super::model::{DisplayBlock, Role};
use super::schema::{LogRecord, TextOnly, ToolResultContent};
use super::session::load_session;
use super::tool_class::{CountedBucket, ResultKind};

fn parse_msg_content(json: &str) -> Vec<DisplayBlock> {
    let r: LogRecord = serde_json::from_str(json).expect("valid test json");
    let msg = r.message.unwrap();
    let is_user = msg.role.as_deref() == Some("user");
    // このヘルパを使うテストのうち duration の値が問題になるものは無い。
    // 例外は thinking_text_is_captured だが、それも duration ではなく
    // text フィールドを見ている — 1 は適当なプレースホルダ。
    content_to_display_blocks(
        msg.content,
        is_user,
        &mut HashMap::new(),
        &HashSet::new(),
        1,
    )
}

// 隠しコンテキストの正規化 (isMeta / ラッパー)

#[test]
fn メタのレコードは飛ばす() {
    // skill 呼び出しは SKILL.md 全体を isMeta な user ターンとしてダンプする。
    // Claude Code はこれを一切表示しないので、トランスクリプトも表示しては
    // いけない。
    let f = write_jsonl(&[
        r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Base directory for this skill: ... 20k chars of SKILL.md"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"real prompt"}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].blocks[0], DisplayBlock::Text(t) if t == "real prompt"));
}

#[test]
fn コマンドの包みはスラッシュ呼び出しとして描く() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<command-name>/merge-pr</command-name>\n<command-message>merge-pr</command-message>\n<command-args>--admin</command-args>"}}"#,
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == "/merge-pr --admin"));
}

#[test]
fn 引数の無い包みは素のコマンド名を出す() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>"}}"#,
    );
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == "/clear"));
}

#[test]
fn ローカルコマンドのstdoutは包みを外して整える() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>\u001b[2mCompacted\u001b[22m</local-command-stdout>"}}"#,
    );
    match &blocks[0] {
        DisplayBlock::Annotation { lines } => assert_eq!(lines, &["Compacted".to_string()]),
        other => panic!("expected Annotation, got {other:?}"),
    }
}

#[test]
fn タスクの通知は要約に畳まれる() {
    // 実測: ラッパー全体が、<summary> のテキストだけを持つ ⏺ 行に置き換わる。
    // タスク id、出力パス、ステータスは一切表示されない。
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<task-notification>\n<task-id>bh15vvqha</task-id>\n<output-file>/private/tmp/x.output</output-file>\n<status>completed</status>\n<summary>Background command \"Run brew audit\" completed (exit code 0)</summary>\n</task-notification>"}}"#,
    );
    match &blocks[0] {
        DisplayBlock::Notice(t) => {
            assert_eq!(
                t,
                "Background command \"Run brew audit\" completed (exit code 0)"
            );
        }
        other => panic!("expected Notice, got {other:?}"),
    }
}

#[test]
fn 要約の無いタスクの通知は何も描かない() {
    // 周りの文章も含めて実測済み: 使える summary が無い場合、生の XML を
    // ダンプする方にフォールバックせず、メッセージ全体が消える。
    for content in [
        "<task-notification>\\n<task-id>abc</task-id>\\n</task-notification>",
        "look:\\n<task-notification>\\n<summary></summary>\\n</task-notification>\\nthoughts?",
    ] {
        let blocks = parse_msg_content(&format!(
            r#"{{"type":"user","message":{{"role":"user","content":"{content}"}}}}"#,
        ));
        assert!(blocks.is_empty(), "expected nothing drawn for {content:?}");
    }
}

#[test]
fn タスクの通知はどこにあっても畳まれる() {
    // 実測: タグはメッセージのどこにあってもマッチし、畳み込むと周りに
    // 打たれた文章は捨てられる。これにより、手動で貼り付けた画面ダンプが
    // CLI 自身の通知と全く同じように描画される — Claude Code はタグの位置も
    // レコードの書き手も確認しない。
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"here is what I saw:\n\n<task-notification>\n<task-id>zz1</task-id>\n<summary>Background command \"Install\" completed (exit code 0)</summary>\n</task-notification>\n\nwhat do you think?"}}"#,
    );
    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        DisplayBlock::Notice(t) => {
            assert_eq!(t, "Background command \"Install\" completed (exit code 0)");
        }
        other => panic!("expected Notice, got {other:?}"),
    }
}

#[test]
fn 通知が二重でも要約は最初の1つだけ残る() {
    // 実測: 1メッセージ内に2つの通知があっても、⏺ 行は1つだけ描画される。
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<task-notification>\n<summary>FIRST done</summary>\n</task-notification>\n<task-notification>\n<summary>SECOND done</summary>\n</task-notification>"}}"#,
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], DisplayBlock::Notice(t) if t == "FIRST done"));
}

#[test]
fn ローカルコマンドのstdoutが空なら落とす() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout></local-command-stdout>"}}"#,
    );
    assert!(blocks.is_empty());
}

#[test]
fn teammateの包みはteammateのブロックになる() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<teammate-message teammate_id=\"alice\">please review PR 42</teammate-message>"}}"#,
    );
    match &blocks[0] {
        DisplayBlock::TeammateMessage { id, body } => {
            assert_eq!(id, "alice");
            assert_eq!(body, "please review PR 42");
        }
        other => panic!("expected TeammateMessage, got {other:?}"),
    }
}

#[test]
fn teammateのsummary属性は無視する() {
    // summary は常に無視される — 属性の順序によらず、読むのは teammate_id
    // と本文のテキストだけ。
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<teammate-message summary=\"short\" teammate_id=\"bob\">the real body</teammate-message>"}}"#,
    );
    match &blocks[0] {
        DisplayBlock::TeammateMessage { id, body } => {
            assert_eq!(id, "bob");
            assert_eq!(body, "the real body");
        }
        other => panic!("expected TeammateMessage, got {other:?}"),
    }
}

#[test]
fn 閉じていないteammateの本文は末尾まで取る() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<teammate-message teammate_id=\"carol\">truncated body"}}"#,
    );
    match &blocks[0] {
        DisplayBlock::TeammateMessage { id, body } => {
            assert_eq!(id, "carol");
            assert_eq!(body, "truncated body");
        }
        other => panic!("expected TeammateMessage, got {other:?}"),
    }
}

#[test]
fn teammate_idが無ければ地の文として扱う() {
    // 壊れたラッパー（このパーサが読む唯一の属性が無い）— 黙って捨てるのでは
    // なく、通常のテキストとして残す。
    let raw = "<teammate-message>no id attribute</teammate-message>";
    let blocks = parse_msg_content(&format!(
        r#"{{"type":"user","message":{{"role":"user","content":"{raw}"}}}}"#,
    ));
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == raw));
}

#[test]
fn 本文の途中のteammateタグへの言及は書き換えない() {
    // ラッパーが認識されるのはメッセージの先頭だけ。ユーザがタグに
    // ついて *言及している* だけなら、プロンプト全文に手を加えない。
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"why did <teammate-message teammate_id=\"x\">hi</teammate-message> show up?"}}"#,
    );
    assert!(matches!(
        &blocks[0],
        DisplayBlock::Text(t) if t == "why did <teammate-message teammate_id=\"x\">hi</teammate-message> show up?"
    ));
}

#[test]
fn system_reminderの範囲はユーザの本文に残す() {
    // 実測: Claude Code はリマインダーを、ターン自身のテキストにインラインで
    // 入っている位置のまま、そのまま描画する。読み手が決して目にしない
    // リマインダーは、1つ上の階層でレコードの isMeta フラグによって隠される。
    let raw = "fix the bug <system-reminder>hidden note</system-reminder>please";
    let blocks = parse_msg_content(&format!(
        r#"{{"type":"user","message":{{"role":"user","content":"{raw}"}}}}"#,
    ));
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == raw));
}

#[test]
fn reminderだけのユーザブロックも1ターンとして描く() {
    // 実測: それ単体のブロックとして届いても隠されない — タグをそのまま
    // 保持した ❯ ターンになる。
    let raw = "<system-reminder>only hidden</system-reminder>";
    let blocks = parse_msg_content(&format!(
        r#"{{"type":"user","message":{{"role":"user","content":"{raw}"}}}}"#,
    ));
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == raw));
}

#[test]
fn 先頭の閉じていないコマンドタグは地の文のまま() {
    // 実際のコマンドレコードは必ず終了タグを伴う。単にそのタグの文字列で
    // *始まる* だけのプロンプトは、手を加えずそのまま残る必要がある。
    let raw = "<command-name> is a wrapper the CLI writes";
    let blocks = parse_msg_content(&format!(
        r#"{{"type":"user","message":{{"role":"user","content":"{raw}"}}}}"#,
    ));
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == raw));
}

#[test]
fn 本文の途中のコマンドタグへの言及は書き換えない() {
    // ラッパーが認識されるのはメッセージの先頭だけ。ユーザがタグについて
    // *言及している* だけならプロンプト全文を保持する。（<task-notification>
    // だけはこの方式に従わない — a_task_notification_collapses_wherever_it_sits
    // を参照。）
    let raw = "why does <command-name>/x</command-name> appear in my log?";
    let blocks = parse_msg_content(&format!(
        r#"{{"type":"user","message":{{"role":"user","content":"{raw}"}}}}"#,
    ));
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == raw));
}

#[test]
fn 包みのタグを引用したassistantの本文には触らない() {
    // assistant はこれらのタグについて正当に議論することがある。ラッパーの
    // 正規化を受けるのは user ターンだけ。
    let raw = "use <system-reminder> and <command-name>/x</command-name> in docs";
    let blocks = parse_msg_content(&format!(
        r#"{{"type":"assistant","message":{{"role":"assistant","content":"{raw}"}}}}"#,
    ));
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == raw));
}

#[test]
fn 文字列のcontentは1つの本文ブロックになる() {
    let blocks =
        parse_msg_content(r#"{"type":"user","message":{"role":"user","content":"hello world"}}"#);
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0], DisplayBlock::Text(_)));
}

#[test]
fn 配列のcontentは複数種類のブロックになる() {
    let blocks = parse_msg_content(
        r#"{
            "type": "assistant",
            "message": {
                "role": "assistant",
                "model": "claude-opus-4",
                "content": [
                    {"type":"text","text":"Here is the plan."},
                    {"type":"thinking","thinking":"","signature":"abc"},
                    {"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"ls -la"}},
                    {"type":"tool_result","tool_use_id":"tu1","content":"file1\nfile2\nfile3"}
                ]
            }
        }"#,
    );
    assert_eq!(blocks.len(), 4);
    assert!(matches!(blocks[0], DisplayBlock::Text(_)));
    assert!(matches!(blocks[1], DisplayBlock::Thinking { .. }));
    assert!(matches!(blocks[2], DisplayBlock::ToolUse { .. }));
    // "ls -la" は List bucket に分類される（§2.1）。ペアリングマップ（同じ
    // 呼び出しの2ブロック前にある tool_use から構築される）が結果側の
    // bucket を解決する。
    assert!(matches!(
        &blocks[3],
        DisplayBlock::ToolResult { kind: ResultKind::Counted { bucket: CountedBucket::List, from_bash: true }, lines, .. }
            if lines.len() == 3
    ));
}

#[test]
fn サイドチェーンの印を読む() {
    let r: LogRecord = serde_json::from_str(
        r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"secret"}}"#,
    )
    .unwrap();
    assert!(r.is_sidechain);
}

// ツール名・引数の分類（コマンドキーの探索、Bash のディスパッチ、
// Counted/Inline/Hidden への分類）は tool_class.rs に、テストごと移した。
// ToolUse が要約済み文字列ではなく生の input を持つようになったため。

#[test]
fn 文字列の結果は行数を数える() {
    let content = ToolResultContent::Text("a\nb\nc".to_string());
    assert_eq!(result_lines(&content).len(), 3);
}

#[test]
fn プレビューの行は描画用に整える() {
    // タブは空白に、ANSI カラーエスケープは除去、制御コードは削除する。これにより
    // 描画されるプレビュー行に幅がずれる文字が残らない。
    let content = ToolResultContent::Text(
        "a\tb\n\u{1b}[31mred\u{1b}[0m\n\u{1b}]8;;http://x\u{07}link\u{1b}]8;;\u{07}\ncr\rlf"
            .to_string(),
    );
    let lines = result_lines(&content);
    assert_eq!(lines[0], "a    b", "tab expands to spaces");
    assert_eq!(lines[1], "red", "ANSI SGR escapes stripped");
    assert_eq!(lines[2], "link", "OSC hyperlink sequences stripped");
    assert_eq!(lines[3], "crlf", "carriage return dropped");
    for l in &lines {
        assert!(
            !l.chars().any(|c| c.is_control()),
            "no control chars remain in {l:?}"
        );
    }
}

#[test]
fn 配列の結果も行数を数える() {
    let content = ToolResultContent::Blocks(vec![
        TextOnly {
            text: "a\nb\nc".to_string(),
        },
        TextOnly {
            text: "d\ne".to_string(),
        },
    ]);
    assert_eq!(result_lines(&content).len(), 5);
}

#[test]
fn 結果の行は上限なく全部残す() {
    // lines は出力行を全て保持する（以前あったプレビュー上限と total_lines の
    // 分離は廃止済み）。展開表示には出力全体が必要になる。
    let body: String = (0..10)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let json = format!(
        r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t","content":{body:?}}}]}}}}"#
    );
    let blocks = parse_msg_content(&json);
    match &blocks[0] {
        DisplayBlock::ToolResult {
            lines, is_error, ..
        } => {
            assert_eq!(lines.len(), 10);
            assert_eq!(lines[0], "line0");
            assert_eq!(lines[9], "line9");
            assert!(!*is_error);
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn 結果のidは同じセッションのレコードをまたいで解決する() {
    // ペアリングマップは load_session のスキャン全体を通して引き継がれる。
    // 1メッセージ内だけではなく、assistant レコードの tool_use が直後の
    // (user) レコードの tool_result から見つけられる必要がある。
    let f = write_jsonl(&[
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu1","name":"Read","input":{"file_path":"/a.txt"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu1","content":"file contents"}]}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 2);
    assert!(matches!(
        &entries[1].blocks[0],
        DisplayBlock::ToolResult {
            kind: ResultKind::Counted {
                bucket: CountedBucket::Read,
                from_bash: false
            },
            ..
        }
    ));
}

#[test]
fn 知らない呼び出しidの結果は隠す() {
    // tool_use_id が一度も見えていない tool_result（ログが途中で切れている、
    // または tool_use レコードが壊れている/欠落している場合）は Hidden に
    // 解決される。カテゴリを推測して迷子のブロックを出すよりは、何も描画しない。
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"nonexistent","content":"x"}
        ]}}"#,
    );
    assert!(matches!(
        &blocks[0],
        DisplayBlock::ToolResult {
            kind: ResultKind::Hidden,
            ..
        }
    ));
}

#[test]
fn 結果のエラー印を拾う() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"t","is_error":true,"content":"boom"}
        ]}}"#,
    );
    assert!(matches!(
        blocks[0],
        DisplayBlock::ToolResult { is_error: true, .. }
    ));
}

#[test]
fn 呼び出しの失敗印は後から来る対の結果で決まる() {
    // tool_use 自身のレコードを構築する時点で errored フラグが分かっている
    // 必要がある。エラーになった tool_result はログ上ではそれより後の
    // レコードなので、これを実現しているのが session.rs の事前スキャンである。
    let f = write_jsonl(&[
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"false"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu1","is_error":true,"content":"boom"}]}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 2);
    assert!(matches!(
        &entries[0].blocks[0],
        DisplayBlock::ToolUse { errored: true, .. }
    ));
}

#[test]
fn 結果が失敗でなければ印は立たない() {
    let f = write_jsonl(&[
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"true"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu1","content":"ok"}]}}"#,
    ]);
    let entries = load_session(f.path());
    assert!(matches!(
        &entries[0].blocks[0],
        DisplayBlock::ToolUse { errored: false, .. }
    ));
}

#[test]
fn 対の結果が無ければ印は立たない() {
    // ログが途中で切れている場合（または tool_result が壊れている/欠落している
    // 場合）に panic したり推測したりしてはいけない。単に未エラーとして解決される。
    let blocks = parse_msg_content(
        r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"false"}}
        ]}}"#,
    );
    assert!(matches!(
        &blocks[0],
        DisplayBlock::ToolUse { errored: false, .. }
    ));
}

#[test]
fn thinkingの本文を拾う() {
    let blocks = parse_msg_content(
        r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"thinking","thinking":"let me reason","signature":"x"}
        ]}}"#,
    );
    match &blocks[0] {
        DisplayBlock::Thinking { text, .. } => assert_eq!(text, "let me reason"),
        other => panic!("expected Thinking, got {other:?}"),
    }
}

#[test]
fn thinkingの秒数は呼び出し側の値をそのまま通す() {
    let r: LogRecord = serde_json::from_str(
        r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"thinking","thinking":"let me reason","signature":"x"}
        ]}}"#,
    )
    .expect("valid test json");
    let msg = r.message.unwrap();
    let blocks =
        content_to_display_blocks(msg.content, false, &mut HashMap::new(), &HashSet::new(), 12);
    match &blocks[0] {
        DisplayBlock::Thinking { duration_secs, .. } => assert_eq!(*duration_secs, 12),
        other => panic!("expected Thinking, got {other:?}"),
    }
}

#[test]
fn 空の本文ブロックは除く() {
    let blocks = parse_msg_content(
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":""}]}}"#,
    );
    assert!(blocks.is_empty());
}

#[test]
fn 知らない種類のブロックは飛ばす() {
    let blocks = parse_msg_content(
        r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"..."}},
            {"type":"text","text":"done"}
        ]}}"#,
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0], DisplayBlock::Text(_)));
}

// load_session の結合テスト（一時 .jsonl を書いて load_session を呼ぶ）

fn write_jsonl(lines: &[&str]) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().expect("tmp file");
    for line in lines {
        writeln!(f, "{line}").expect("write");
    }
    f
}

#[test]
fn 描くターンの無いレコードは落とす() {
    let cases = [
        (
            "user でも assistant でもない type",
            r#"{"type":"system","message":{"role":"system","content":"sys prompt"}}"#,
        ),
        (
            "サイドチェーン",
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"hidden"}}"#,
        ),
        (
            "type と role の食い違い",
            r#"{"type":"user","message":{"role":"system","content":"not a user turn"}}"#,
        ),
        (
            "表示ブロックを 1 つも生まない content",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":""}]}}"#,
        ),
    ];
    for (label, dropped) in cases {
        let f = write_jsonl(&[
            dropped,
            r#"{"type":"user","message":{"role":"user","content":"real user"}}"#,
        ]);
        let entries = load_session(f.path());
        assert_eq!(entries.len(), 1, "{label}");
        assert_eq!(entries[0].role, Role::User, "{label}");
    }
}

#[test]
fn 混在したレコードでも件数が合う() {
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"q1"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":"a1"}}"#,
        r#"{"type":"system","message":{"role":"system","content":"noise"}}"#,
        r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"chain"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"q2"}}"#,
        r#"not-json-at-all"#,
    ]);
    let entries = load_session(f.path());
    // system のノイズ、サイドチェーン、壊れた行が除外され → 3件残る。
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].role, Role::User);
    assert_eq!(entries[1].role, Role::Assistant);
    assert_eq!(entries[2].role, Role::User);
}

#[test]
fn キュー操作のレコードは何も描かない() {
    // 実測: Claude Code は入力キューのジャーナルを完全に無視する。ビジー中に
    // 入力されたプロンプトは受理されると通常の user レコードとして再度出力
    // されるので、enqueue も描画すると同じターンが二重に出てしまう。また、
    // 再出力されなかった enqueue はそもそも一切描画されない。
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"first"}}"#,
        r#"{"type":"queue-operation","operation":"enqueue","content":"typed while busy"}"#,
        r#"{"type":"queue-operation","operation":"remove","content":"typed while busy"}"#,
        r#"{"type":"user","message":{"role":"user","content":"typed while busy"},"promptSource":"queued"}"#,
        r#"{"type":"queue-operation","operation":"enqueue","content":"never re-emitted"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":"reply"}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 3, "queue-operation records must not draw");
    assert!(matches!(&entries[0].blocks[0], DisplayBlock::Text(t) if t == "first"));
    assert!(matches!(&entries[1].blocks[0], DisplayBlock::Text(t) if t == "typed while busy"));
    assert_eq!(entries[2].role, Role::Assistant);
}

#[test]
fn セッションのメタ記録は飛ばす() {
    // 会話ではないその他のレコード種別。いずれも Claude Code は描画しない。
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"first"}}"#,
        r#"{"type":"mode","mode":"default"}"#,
        r#"{"type":"permission-mode","permissionMode":"acceptEdits"}"#,
        r#"{"type":"last-prompt","lastPrompt":"first"}"#,
        r#"{"type":"ai-title","aiTitle":"Some title"}"#,
        r#"{"type":"custom-title","customTitle":"Mine"}"#,
        r#"{"type":"agent-name","agentName":"ivy"}"#,
        r#"{"type":"pr-link","prNumber":297,"prUrl":"https://example.invalid/pr/297"}"#,
        r#"{"type":"file-history-snapshot","messageId":"m1","snapshot":{}}"#,
        r#"{"type":"file-history-delta","messageId":"m1","trackingPath":"src/x.rs"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":"reply"}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].role, Role::User);
    assert_eq!(entries[1].role, Role::Assistant);
}

#[test]
fn ファイルが無ければ空を返す() {
    let entries = load_session(std::path::Path::new("/nonexistent/path.jsonl"));
    assert!(entries.is_empty());
}

#[test]
fn thinkingの長さは時刻から計算する() {
    let f = write_jsonl(&[
        r#"{"type":"user","timestamp":"2026-07-31T00:00:00Z","message":{"role":"user","content":"hi"}}"#,
        r#"{"type":"assistant","timestamp":"2026-07-31T00:00:05Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"reasoning","signature":"x"}]}}"#,
    ]);
    let entries = load_session(f.path());
    match &entries[1].blocks[0] {
        DisplayBlock::Thinking { duration_secs, .. } => assert_eq!(*duration_secs, 5),
        other => panic!("expected Thinking, got {other:?}"),
    }
}

#[test]
fn 時刻が無ければthinkingの長さは1秒に落ちる() {
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"reasoning","signature":"x"}]}}"#,
    ]);
    let entries = load_session(f.path());
    match &entries[1].blocks[0] {
        DisplayBlock::Thinking { duration_secs, .. } => assert_eq!(*duration_secs, 1),
        other => panic!("expected Thinking, got {other:?}"),
    }
}

#[test]
fn 飛ばしたメタのレコードは直前として数えない() {
    // 表示される2ターンの間に、スキップされる isMeta レコードが1つ挟まっている。
    // このレコードのタイムスタンプをそのまま「直前レコード」として差分を取ると
    // 負になってしまう。duration は隠れたこのレコードではなく、直前の
    // *表示される* レコード（最初の user ターン）を基準に計算されなければ
    // ならない — 負・歪んだ差分が生む1秒のフォールバックではなく、5秒になる。
    let f = write_jsonl(&[
        r#"{"type":"user","timestamp":"2026-07-31T00:00:00Z","message":{"role":"user","content":"hi"}}"#,
        r#"{"type":"user","isMeta":true,"timestamp":"2026-07-31T00:05:00Z","message":{"role":"user","content":"skill dump"}}"#,
        r#"{"type":"assistant","timestamp":"2026-07-31T00:00:05Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"reasoning","signature":"x"}]}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(
        entries.len(),
        2,
        "the isMeta record must not produce an entry"
    );
    match &entries[1].blocks[0] {
        DisplayBlock::Thinking { duration_secs, .. } => assert_eq!(*duration_secs, 5),
        other => panic!("expected Thinking, got {other:?}"),
    }
}

// コンパクト境界と CLI が挿入する添付ファイル (実測)

/// /compact が書き込むレコード列を、ログ上の順序どおりに並べたもの。再開後の
/// トランスクリプトに対して端から端まで実測した結果、Claude Code は次のように
/// 描画する。
/// ```text
/// ✻ Conversation compacted (ctrl+o for history)
///
/// ❯ /compact
///   ⎿  Compacted (ctrl+o to see full summary)
///   ⎿  Read alpha.rs (42 lines)
///   ⎿  Referenced file beta.yml
/// ```
/// — 要約本文そのものはどこにも現れない。
const COMPACT_SEQUENCE: &[&str] = &[
    r#"{"type":"system","subtype":"compact_boundary","content":"Conversation compacted"}"#,
    r#"{"type":"user","isVisibleInTranscriptOnly":true,"isCompactSummary":true,"message":{"role":"user","content":"This session is being continued. SUMMARYBODY"}}"#,
    r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>Caveat</local-command-caveat>"}}"#,
    r#"{"type":"user","message":{"role":"user","content":"<command-name>/compact</command-name>\n<command-args></command-args>"}}"#,
    r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>Compacted (ctrl+o to see full summary)</local-command-stdout>"}}"#,
    r#"{"type":"attachment","attachment":{"type":"file","displayPath":"alpha.rs","content":{"type":"text","file":{"numLines":42}}}}"#,
    r#"{"type":"attachment","attachment":{"type":"compact_file_reference","displayPath":"beta.yml"}}"#,
];

#[test]
fn compactの要約本文は表示しない() {
    let f = write_jsonl(COMPACT_SEQUENCE);
    let entries = load_session(f.path());
    let all: String = format!("{:?}", entries);
    assert!(
        !all.contains("SUMMARYBODY"),
        "the compact summary body leaked into the transcript: {all}"
    );
}

#[test]
fn compactの並びは実測どおりのブロックになる() {
    let f = write_jsonl(COMPACT_SEQUENCE);
    let entries = load_session(f.path());
    let blocks: Vec<&DisplayBlock> = entries.iter().flat_map(|e| &e.blocks).collect();
    assert_eq!(blocks.len(), 5, "got {blocks:?}");
    assert!(matches!(blocks[0], DisplayBlock::CompactBoundary));
    assert!(matches!(blocks[1], DisplayBlock::Text(t) if t == "/compact"));
    assert!(matches!(blocks[2], DisplayBlock::Annotation { lines }
            if lines == &["Compacted (ctrl+o to see full summary)".to_string()]));
    assert!(matches!(blocks[3], DisplayBlock::Annotation { lines }
            if lines == &["Read alpha.rs (42 lines)".to_string()]));
    assert!(matches!(blocks[4], DisplayBlock::Annotation { lines }
            if lines == &["Referenced file beta.yml".to_string()]));
}

#[test]
fn 行数の無い添付は件数の節を落とす() {
    let f = write_jsonl(&[
        r#"{"type":"attachment","attachment":{"type":"file","displayPath":"solo.rs"}}"#,
    ]);
    let entries = load_session(f.path());
    assert!(
        matches!(&entries[0].blocks[0], DisplayBlock::Annotation { lines }
            if lines == &["Read solo.rs".to_string()])
    );
}

#[test]
fn 添付が1行なら複数形にしない() {
    let f = write_jsonl(&[
        r#"{"type":"attachment","attachment":{"type":"file","displayPath":"one.rs","content":{"type":"text","file":{"numLines":1}}}}"#,
    ]);
    let entries = load_session(f.path());
    assert!(
        matches!(&entries[0].blocks[0], DisplayBlock::Annotation { lines }
            if lines == &["Read one.rs (1 line)".to_string()])
    );
}

#[test]
fn 表示パスが無ければファイル名に落ちる() {
    let f = write_jsonl(&[
        r#"{"type":"attachment","attachment":{"type":"compact_file_reference","filename":"/abs/path.yml"}}"#,
    ]);
    let entries = load_session(f.path());
    assert!(
        matches!(&entries[0].blocks[0], DisplayBlock::Annotation { lines }
            if lines == &["Referenced file /abs/path.yml".to_string()])
    );
}

#[test]
fn 表示しない種類の添付は何も描かない() {
    // 実データには他に27種類あり、hook_success だけでも約4.7万件ある。
    // どれも描画される様子が観測されなかったので、レンダラは許可リスト方式で動く。
    let f = write_jsonl(&[
        r#"{"type":"attachment","attachment":{"type":"hook_success","hookName":"PreToolUse:Read","stdout":"{}"}}"#,
        r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- daisy: ..."}}"#,
        r#"{"type":"attachment","attachment":{"type":"diagnostics","displayPath":"x.rs"}}"#,
    ]);
    assert!(load_session(f.path()).is_empty());
}

#[test]
fn compact以外のsystemレコードは何も描かない() {
    let f = write_jsonl(&[
        r#"{"type":"system","subtype":"something_else","content":"noise"}"#,
        r#"{"type":"last-prompt","lastPrompt":"/compact"}"#,
        r#"{"type":"custom-title","customTitle":"a title"}"#,
    ]);
    assert!(load_session(f.path()).is_empty());
}
