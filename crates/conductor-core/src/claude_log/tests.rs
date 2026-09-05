//! レコードはビルダーで組む。CLI が書く未文書化の形 (ラッパー文字列、ジャーナル、
//! compact の並び、添付) は実測した JSON をそのまま fixture として持つ。

use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};

use super::model::{DisplayBlock, LogEntry, Role};
use super::session::{load_session, parse_jsonl};
use super::tool_class::{CountedBucket, ResultKind, ToolCategory, classify, unknown_tool_arg};

/// .jsonl の 1 レコード。
struct Rec(Value);

impl Rec {
    fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.0[key] = value.into();
        self
    }

    fn at(self, timestamp: &str) -> Self {
        self.with("timestamp", timestamp)
    }

    fn line(&self) -> String {
        self.0.to_string()
    }
}

fn raw(line: &str) -> Rec {
    Rec(serde_json::from_str(line).expect("fixture is json"))
}

fn record(kind: &str, role: &str, content: Value) -> Rec {
    Rec(json!({"type": kind, "message": {"role": role, "content": content}}))
}

fn user(content: impl Into<Value>) -> Rec {
    record("user", "user", content.into())
}

fn assistant(content: impl Into<Value>) -> Rec {
    record("assistant", "assistant", content.into())
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn thinking(text: &str) -> Value {
    json!({"type": "thinking", "thinking": text, "signature": "x"})
}

fn tool_use(id: &str, name: &str, input: Value) -> Value {
    json!({"type": "tool_use", "id": id, "name": name, "input": input})
}

fn tool_result(id: &str, content: impl Into<Value>) -> Value {
    json!({"type": "tool_result", "tool_use_id": id, "content": content.into()})
}

fn errored(mut block: Value) -> Value {
    block["is_error"] = json!(true);
    block
}

fn jsonl(lines: impl IntoIterator<Item = String>) -> Vec<LogEntry> {
    parse_jsonl(&lines.into_iter().collect::<Vec<_>>().join("\n"))
}

fn parse(records: &[Rec]) -> Vec<LogEntry> {
    jsonl(records.iter().map(Rec::line))
}

fn blocks(records: &[Rec]) -> Vec<DisplayBlock> {
    parse(records).into_iter().flat_map(|e| e.blocks).collect()
}

fn blocks_raw(lines: &[&str]) -> Vec<DisplayBlock> {
    parse_jsonl(&lines.join("\n"))
        .into_iter()
        .flat_map(|e| e.blocks)
        .collect()
}

fn entry(role: Role, blocks: Vec<DisplayBlock>) -> LogEntry {
    LogEntry { role, blocks }
}

fn text(s: &str) -> DisplayBlock {
    DisplayBlock::Text(s.to_string())
}

fn notice(s: &str) -> DisplayBlock {
    DisplayBlock::Notice(s.to_string())
}

fn teammate(id: &str, body: &str) -> DisplayBlock {
    DisplayBlock::TeammateMessage {
        id: id.to_string(),
        body: body.to_string(),
    }
}

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| l.to_string()).collect()
}

fn annotation(rows: &[&str]) -> DisplayBlock {
    DisplayBlock::Annotation { lines: lines(rows) }
}

fn roles(entries: &[LogEntry]) -> Vec<Role> {
    entries.iter().map(|e| e.role.clone()).collect()
}

// user ターンのラッパー

#[test]
fn ユーザターンのラッパーは画面で見えていた形に畳む() {
    let mention_teammate =
        "why did <teammate-message teammate_id=\"x\">hi</teammate-message> show up?";
    let inline_reminder = "fix the bug <system-reminder>hidden note</system-reminder>please";
    let solo_reminder = "<system-reminder>only hidden</system-reminder>";
    let unclosed_command = "<command-name> is a wrapper the CLI writes";
    let mention_command = "why does <command-name>/x</command-name> appear in my log?";
    let cases: Vec<(&str, &str, Vec<DisplayBlock>)> = vec![
        (
            "素の文字列は 1 つの本文",
            "hello world",
            vec![text("hello world")],
        ),
        (
            "command-name と command-args はスラッシュ呼び出し",
            "<command-name>/merge-pr</command-name>\n<command-message>merge-pr</command-message>\n<command-args>--admin</command-args>",
            vec![text("/merge-pr --admin")],
        ),
        (
            "引数が無ければコマンド名だけ",
            "<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>",
            vec![text("/clear")],
        ),
        (
            "local-command-stdout は包みを外して注釈にする",
            "<local-command-stdout>\u{1b}[2mCompacted\u{1b}[22m</local-command-stdout>",
            vec![annotation(&["Compacted"])],
        ),
        (
            "stdout が空なら落とす",
            "<local-command-stdout></local-command-stdout>",
            vec![],
        ),
        (
            "task-notification は summary だけの通知になる",
            "<task-notification>\n<task-id>bh15vvqha</task-id>\n<output-file>/private/tmp/x.output</output-file>\n<status>completed</status>\n<summary>Background command \"Run brew audit\" completed (exit code 0)</summary>\n</task-notification>",
            vec![notice(
                "Background command \"Run brew audit\" completed (exit code 0)",
            )],
        ),
        (
            "summary が無い通知はメッセージごと消える",
            "<task-notification>\n<task-id>abc</task-id>\n</task-notification>",
            vec![],
        ),
        (
            "summary が空でも周りの文章ごと消える",
            "look:\n<task-notification>\n<summary></summary>\n</task-notification>\nthoughts?",
            vec![],
        ),
        (
            "通知は途中にあっても畳まれ、周りの文章は捨てる",
            "here is what I saw:\n\n<task-notification>\n<task-id>zz1</task-id>\n<summary>Background command \"Install\" completed (exit code 0)</summary>\n</task-notification>\n\nwhat do you think?",
            vec![notice(
                "Background command \"Install\" completed (exit code 0)",
            )],
        ),
        (
            "通知が 2 つあっても最初の summary だけ",
            "<task-notification>\n<summary>FIRST done</summary>\n</task-notification>\n<task-notification>\n<summary>SECOND done</summary>\n</task-notification>",
            vec![notice("FIRST done")],
        ),
        (
            "teammate-message は id と本文",
            "<teammate-message teammate_id=\"alice\">please review PR 42</teammate-message>",
            vec![teammate("alice", "please review PR 42")],
        ),
        (
            "teammate の summary 属性は読まない",
            "<teammate-message summary=\"short\" teammate_id=\"bob\">the real body</teammate-message>",
            vec![teammate("bob", "the real body")],
        ),
        (
            "閉じていない teammate は末尾まで本文",
            "<teammate-message teammate_id=\"carol\">truncated body",
            vec![teammate("carol", "truncated body")],
        ),
        (
            "teammate_id が無ければ地の文",
            "<teammate-message>no id attribute</teammate-message>",
            vec![text("<teammate-message>no id attribute</teammate-message>")],
        ),
        (
            "途中の teammate タグへの言及は書き換えない",
            mention_teammate,
            vec![text(mention_teammate)],
        ),
        (
            "system-reminder はインラインのまま残す",
            inline_reminder,
            vec![text(inline_reminder)],
        ),
        (
            "reminder だけでも 1 ターンとして残す",
            solo_reminder,
            vec![text(solo_reminder)],
        ),
        (
            "閉じていない command-name は地の文",
            unclosed_command,
            vec![text(unclosed_command)],
        ),
        (
            "途中の command タグへの言及は書き換えない",
            mention_command,
            vec![text(mention_command)],
        ),
    ];
    for (label, content, want) in cases {
        assert_eq!(blocks(&[user(content)]), want, "{label}");
    }
}

#[test]
fn assistantの本文はラッパーを畳まない() {
    let quoted = "use <system-reminder> and <command-name>/x</command-name> in docs";
    assert_eq!(blocks(&[assistant(quoted)]), vec![text(quoted)]);
}

// content からブロックへ

#[test]
fn 配列のcontentは種類ごとのブロックになる() {
    let got = blocks(&[assistant(json!([
        text_block("Here is the plan."),
        thinking(""),
        tool_use("tu1", "Bash", json!({"command": "ls -la"})),
        tool_result("tu1", "file1\nfile2\nfile3"),
    ]))]);
    assert_eq!(
        got,
        vec![
            text("Here is the plan."),
            DisplayBlock::Thinking {
                text: String::new(),
                duration_secs: 1,
            },
            DisplayBlock::ToolUse {
                name: "Bash".to_string(),
                input: json!({"command": "ls -la"}),
                errored: false,
            },
            DisplayBlock::ToolResult {
                kind: ResultKind::Counted {
                    bucket: CountedBucket::List,
                    from_bash: true,
                },
                lines: lines(&["file1", "file2", "file3"]),
                is_error: false,
            },
        ]
    );
}

#[test]
fn 描くものの無いブロックは飛ばす() {
    let image = json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "..."}});
    assert_eq!(
        blocks(&[assistant(json!([image, text_block("done")]))]),
        vec![text("done")]
    );
    assert!(blocks(&[assistant(json!([text_block("")]))]).is_empty());
}

#[test]
fn 結果の行は上限なく全部残す() {
    let ten: Vec<String> = (0..10).map(|i| format!("line{i}")).collect();
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "文字列",
            tool_result("t", "a\nb\nc"),
            lines(&["a", "b", "c"]),
        ),
        (
            "text ブロックの配列",
            tool_result(
                "t",
                json!([{"type": "text", "text": "a\nb\nc"}, {"type": "text", "text": "d\ne"}]),
            ),
            lines(&["a", "b", "c", "d", "e"]),
        ),
        (
            "content 無し",
            json!({"type": "tool_result", "tool_use_id": "t"}),
            vec![],
        ),
        ("10 行", tool_result("t", ten.join("\n")), ten.clone()),
    ];
    for (label, block, want) in cases {
        let got = blocks(&[user(json!([block]))]);
        let DisplayBlock::ToolResult { lines, .. } = &got[0] else {
            panic!("{label}: expected ToolResult, got {got:?}");
        };
        assert_eq!(lines, &want, "{label}");
    }
}

#[test]
fn 結果の行から端末をずらす文字を除く() {
    let got = blocks(&[user(json!([tool_result(
        "t",
        "a\tb\n\u{1b}[31mred\u{1b}[0m\n\u{1b}]8;;http://x\u{07}link\u{1b}]8;;\u{07}\ncr\rlf"
    )]))]);
    let DisplayBlock::ToolResult { lines, .. } = &got[0] else {
        panic!("expected ToolResult, got {got:?}");
    };
    assert_eq!(lines, &["a    b", "red", "link", "crlf"]);
}

// tool_use と tool_result の対応

#[test]
fn 結果の種類は前のレコードにある呼び出しから決まる() {
    let entries = parse(&[
        assistant(json!([tool_use(
            "tu1",
            "Read",
            json!({"file_path": "/a.txt"})
        )])),
        user(json!([tool_result("tu1", "file contents")])),
    ]);
    assert_eq!(entries.len(), 2);
    assert!(matches!(
        entries[1].blocks[0],
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
fn 対応の無い結果は隠す() {
    let got = blocks(&[user(json!([tool_result("nonexistent", "x")]))]);
    assert!(matches!(
        got[0],
        DisplayBlock::ToolResult {
            kind: ResultKind::Hidden,
            ..
        }
    ));
}

#[test]
fn 結果のエラー印を拾う() {
    let got = blocks(&[user(json!([errored(tool_result("t", "boom"))]))]);
    assert!(matches!(
        got[0],
        DisplayBlock::ToolResult { is_error: true, .. }
    ));
}

#[test]
fn 呼び出しの失敗印は後から来る対の結果で決まる() {
    let call = || {
        assistant(json!([tool_use(
            "tu1",
            "Bash",
            json!({"command": "false"})
        )]))
    };
    let cases: Vec<(&str, Vec<Rec>, bool)> = vec![
        (
            "対の結果がエラー",
            vec![call(), user(json!([errored(tool_result("tu1", "boom"))]))],
            true,
        ),
        (
            "対の結果が成功",
            vec![call(), user(json!([tool_result("tu1", "ok")]))],
            false,
        ),
        ("対の結果が無い (途中で切れたログ)", vec![call()], false),
    ];
    for (label, records, want) in cases {
        let got = blocks(&records);
        assert!(
            matches!(got[0], DisplayBlock::ToolUse { errored, .. } if errored == want),
            "{label}: {got:?}"
        );
    }
}

// thinking

#[test]
fn thinkingの本文を拾う() {
    let got = blocks(&[assistant(json!([thinking("let me reason")]))]);
    assert!(matches!(&got[0], DisplayBlock::Thinking { text, .. } if text == "let me reason"));
}

#[test]
fn thinkingの秒数は直前に表示したレコードとの時刻差() {
    let reasoning = || assistant(json!([thinking("reasoning")]));
    let cases: Vec<(&str, Vec<Rec>, u64)> = vec![
        (
            "時刻差 5 秒",
            vec![
                user("hi").at("2026-07-31T00:00:00Z"),
                reasoning().at("2026-07-31T00:00:05Z"),
            ],
            5,
        ),
        ("時刻が無ければ 1 秒", vec![user("hi"), reasoning()], 1),
        (
            "飛ばした isMeta のレコードは基準にしない",
            vec![
                user("hi").at("2026-07-31T00:00:00Z"),
                user("skill dump")
                    .with("isMeta", true)
                    .at("2026-07-31T00:05:00Z"),
                reasoning().at("2026-07-31T00:00:05Z"),
            ],
            5,
        ),
    ];
    for (label, records, want) in cases {
        let entries = parse(&records);
        assert_eq!(entries.len(), 2, "{label}");
        assert_eq!(
            entries[1].blocks[0],
            DisplayBlock::Thinking {
                text: "reasoning".to_string(),
                duration_secs: want,
            },
            "{label}"
        );
    }
}

// レコードの取捨

#[test]
fn 描くターンの無いレコードは落とす() {
    let cases: Vec<(&str, Rec)> = vec![
        (
            "user でも assistant でもない type",
            record("system", "system", "sys prompt".into()),
        ),
        ("サイドチェーン", user("hidden").with("isSidechain", true)),
        (
            "isMeta (skill 定義のダンプ)",
            user("Base directory for this skill: ... 20k chars of SKILL.md").with("isMeta", true),
        ),
        (
            "type と role の食い違い",
            record("user", "system", "not a user turn".into()),
        ),
        (
            "表示ブロックを 1 つも生まない content",
            assistant(json!([text_block("")])),
        ),
        ("message が無い", raw(r#"{"type":"user"}"#)),
    ];
    for (label, dropped) in cases {
        assert_eq!(
            parse(&[dropped, user("real user")]),
            vec![entry(Role::User, vec![text("real user")])],
            "{label}"
        );
    }
}

#[test]
fn 壊れた行とノイズを飛ばしても順序が保たれる() {
    let entries = jsonl([
        user("q1").line(),
        assistant("a1").line(),
        record("system", "system", "noise".into()).line(),
        user("chain").with("isSidechain", true).line(),
        user("q2").line(),
        "not-json-at-all".to_string(),
    ]);
    assert_eq!(roles(&entries), [Role::User, Role::Assistant, Role::User]);
}

/// Claude Code が会話とは別に書く記録。どれも画面には出ない。
const JOURNAL_RECORDS: &[&str] = &[
    r#"{"type":"mode","mode":"default"}"#,
    r#"{"type":"permission-mode","permissionMode":"acceptEdits"}"#,
    r#"{"type":"last-prompt","lastPrompt":"first"}"#,
    r#"{"type":"ai-title","aiTitle":"Some title"}"#,
    r#"{"type":"custom-title","customTitle":"Mine"}"#,
    r#"{"type":"agent-name","agentName":"ivy"}"#,
    r#"{"type":"pr-link","prNumber":297,"prUrl":"https://example.invalid/pr/297"}"#,
    r#"{"type":"file-history-snapshot","messageId":"m1","snapshot":{}}"#,
    r#"{"type":"file-history-delta","messageId":"m1","trackingPath":"src/x.rs"}"#,
    r#"{"type":"system","subtype":"something_else","content":"noise"}"#,
];

#[test]
fn セッションのジャーナルは何も描かない() {
    let mut lines = vec![user("first").line()];
    lines.extend(JOURNAL_RECORDS.iter().map(|l| l.to_string()));
    lines.push(assistant("reply").line());
    assert_eq!(roles(&jsonl(lines)), [Role::User, Role::Assistant]);
}

#[test]
fn キュー操作の記録は描かない() {
    // 受理されたプロンプトは promptSource: "queued" の user レコードとして改めて書かれる。
    // enqueue も描くと同じターンが二重に出る。
    let entries = jsonl([
        user("first").line(),
        r#"{"type":"queue-operation","operation":"enqueue","content":"typed while busy"}"#.into(),
        r#"{"type":"queue-operation","operation":"remove","content":"typed while busy"}"#.into(),
        user("typed while busy")
            .with("promptSource", "queued")
            .line(),
        r#"{"type":"queue-operation","operation":"enqueue","content":"never re-emitted"}"#.into(),
        assistant("reply").line(),
    ]);
    assert_eq!(
        entries,
        vec![
            entry(Role::User, vec![text("first")]),
            entry(Role::User, vec![text("typed while busy")]),
            entry(Role::Assistant, vec![text("reply")]),
        ]
    );
}

// ファイル

#[test]
fn ファイルが無ければ空() {
    assert!(load_session(Path::new("/nonexistent/path.jsonl")).is_empty());
}

#[test]
fn ファイルから読む() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "{}", user("from file").line()).unwrap();
    assert_eq!(
        load_session(f.path()),
        vec![entry(Role::User, vec![text("from file")])]
    );
}

// compact と添付 (実測)

/// /compact が書く並び。Claude Code は次のように描き、要約本文はどこにも出ない。
///   ✻ Conversation compacted
///   ❯ /compact
///     ⎿  Compacted (ctrl+o to see full summary)
///     ⎿  Read alpha.rs (42 lines)
///     ⎿  Referenced file beta.yml
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
fn compactの並びは実測どおりのブロックになり要約本文は出ない() {
    assert_eq!(
        blocks_raw(COMPACT_SEQUENCE),
        vec![
            DisplayBlock::CompactBoundary,
            text("/compact"),
            annotation(&["Compacted (ctrl+o to see full summary)"]),
            annotation(&["Read alpha.rs (42 lines)"]),
            annotation(&["Referenced file beta.yml"]),
        ]
    );
}

#[test]
fn 添付で描くのはfileとcompact_file_referenceだけ() {
    let cases: Vec<(&str, &str, Vec<DisplayBlock>)> = vec![
        (
            "file は行数付き",
            r#"{"type":"file","displayPath":"alpha.rs","content":{"type":"text","file":{"numLines":42}}}"#,
            vec![annotation(&["Read alpha.rs (42 lines)"])],
        ),
        (
            "1 行は単数形",
            r#"{"type":"file","displayPath":"one.rs","content":{"type":"text","file":{"numLines":1}}}"#,
            vec![annotation(&["Read one.rs (1 line)"])],
        ),
        (
            "行数が無ければ括弧ごと省く",
            r#"{"type":"file","displayPath":"solo.rs"}"#,
            vec![annotation(&["Read solo.rs"])],
        ),
        (
            "displayPath が無ければ filename",
            r#"{"type":"compact_file_reference","filename":"/abs/path.yml"}"#,
            vec![annotation(&["Referenced file /abs/path.yml"])],
        ),
        (
            "hook_success",
            r#"{"type":"hook_success","hookName":"PreToolUse:Read","stdout":"{}"}"#,
            vec![],
        ),
        (
            "skill_listing",
            r#"{"type":"skill_listing","content":"- daisy: ..."}"#,
            vec![],
        ),
        (
            "diagnostics は displayPath があっても描かない",
            r#"{"type":"diagnostics","displayPath":"x.rs"}"#,
            vec![],
        ),
    ];
    for (label, attachment, want) in cases {
        let line = format!(r#"{{"type":"attachment","attachment":{attachment}}}"#);
        assert_eq!(blocks_raw(&[&line]), want, "{label}");
    }
}

// ツールの分類

#[test]
fn ツールの分類表() {
    use CountedBucket::*;
    use ToolCategory::*;
    let inline = |name: &str, arg: Option<&str>| Inline {
        display_name: name.to_string(),
        arg: arg.map(str::to_string),
    };
    let cases: Vec<(&str, &str, Value, ToolCategory)> = vec![
        (
            "Read",
            "Read",
            json!({"file_path": "/a.txt"}),
            Counted(Read),
        ),
        ("Grep", "Grep", json!({"pattern": "foo"}), Counted(Search)),
        ("Glob", "Glob", json!({"pattern": "foo"}), Counted(Search)),
        (
            "Bash の ls は List",
            "Bash",
            json!({"command": "ls -la /tmp"}),
            Counted(List),
        ),
        (
            "Bash の cat は Read に合流する",
            "Bash",
            json!({"command": "cat foo.txt"}),
            Counted(Read),
        ),
        (
            "Bash の他のコマンドは Inline",
            "Bash",
            json!({"command": "cargo build"}),
            inline("Bash", Some("cargo build")),
        ),
        (
            "先頭の空白は無視して最初の語で振り分ける",
            "Bash",
            json!({"command": "   ls /tmp"}),
            Counted(List),
        ),
        (
            "Write は file_path",
            "Write",
            json!({"file_path": "/tmp/out.txt", "content": "..."}),
            inline("Write", Some("/tmp/out.txt")),
        ),
        (
            "Edit は Update",
            "Edit",
            json!({"file_path": "/tmp/out.txt"}),
            inline("Update", Some("/tmp/out.txt")),
        ),
        (
            "Task は Agent と description",
            "Task",
            json!({"description": "investigate bug", "prompt": "..."}),
            inline("Agent", Some("investigate bug")),
        ),
        (
            "WebFetch は Fetch と url",
            "WebFetch",
            json!({"url": "https://example.com"}),
            inline("Fetch", Some("https://example.com")),
        ),
        (
            "TodoWrite は隠す",
            "TodoWrite",
            json!({"todos": []}),
            Hidden,
        ),
        (
            "未知のツールはキー探索",
            "WebSearch",
            json!({"query": "some search term"}),
            inline("WebSearch", Some("some search term")),
        ),
        (
            "キーが無ければ引数無し",
            "Write",
            json!({"content": "..."}),
            inline("Write", None),
        ),
        (
            "空文字も引数無し",
            "Write",
            json!({"file_path": ""}),
            inline("Write", None),
        ),
    ];
    for (label, name, input, want) in cases {
        assert_eq!(classify(name, &input), want, "{label}");
    }
}

#[test]
fn 未知のツールの引数は固定の優先順でキーを試す() {
    assert_eq!(
        unknown_tool_arg(&json!({"file_path": "/a", "command": "run me"})),
        Some("run me".to_string())
    );
    assert_eq!(unknown_tool_arg(&json!({"unrelated": "x"})), None);
}
