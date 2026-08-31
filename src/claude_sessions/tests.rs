//! トランスクリプト元解決のテスト。
//!
//! 1 つの Claude プロジェクトディレクトリには、あるワークツリーで走った
//! 全セッションのログが同居する。ここでは reflow トランスクリプトビューが
//! 依拠する性質を固定する: *session id* がログを選ぶのであって、同じ
//! ディレクトリを共有する兄弟ログの何か(数、mtime、最初/最後のターン時刻)
//! によって選ばれるログが変わってはならない。

use std::collections::HashSet;
use std::fs::{File, FileTimes};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::rotation::resolve_current_session_id;
use super::session_log_in_dir;
use crate::reflow::log::{DisplayBlock, load_session};

/// dir に <session_id>.jsonl を書く。turns の各要素を1ユーザターンとして
/// 1行ずつ書き、mtime は now - age に強制する。
fn write_log(dir: &Path, session_id: &str, turns: &[&str], age: Duration) -> PathBuf {
    let path = dir.join(format!("{session_id}.jsonl"));
    let mut f = File::create(&path).expect("create log");
    for turn in turns {
        writeln!(
            f,
            r#"{{"type":"user","sessionId":"{session_id}","message":{{"role":"user","content":"{turn}"}}}}"#
        )
        .expect("write turn");
    }
    let mtime = SystemTime::now() - age;
    f.set_times(FileTimes::new().set_modified(mtime))
        .expect("set mtime");
    path
}

/// session_id について解決したトランスクリプト中の、全エントリのテキスト。
fn transcript_texts(dir: &Path, session_id: &str) -> Vec<String> {
    let path = session_log_in_dir(dir, session_id).expect("session log resolves");
    load_session(&path)
        .iter()
        .flat_map(|e| e.blocks.clone())
        .map(|b| match b {
            DisplayBlock::Text(t) => t,
            other => panic!("unexpected block: {other:?}"),
        })
        .collect()
}

#[test]
fn session_id_selects_the_log_among_siblings() {
    // 3 つのセッションが同じプロジェクトディレクトリを共有する(同じ
    // worktree の 3 つの Claude パネル)。mtime がどうであれ、それぞれの
    // id は自分自身のログに解決すること。
    let dir = tempfile::tempdir().expect("tmp dir");
    write_log(dir.path(), "aaa", &["from aaa"], Duration::from_secs(3600));
    write_log(dir.path(), "bbb", &["from bbb"], Duration::from_secs(60));
    write_log(dir.path(), "ccc", &["from ccc"], Duration::ZERO);

    assert_eq!(transcript_texts(dir.path(), "aaa"), vec!["from aaa"]);
    assert_eq!(transcript_texts(dir.path(), "bbb"), vec!["from bbb"]);
    assert_eq!(transcript_texts(dir.path(), "ccc"), vec!["from ccc"]);
}

#[test]
fn idle_session_is_not_displaced_by_a_newer_sibling() {
    // 回帰: メインエージェントが停止しサブエージェントだけ動いているパネルは
    // 自分のセッションログに何も書かない (サブエージェントのターンは
    // <session-id>/subagents/*.jsonl に入る)。その結果、このログはディレクトリ
    // 内で最も古く、しかも同じ worktree で後から始まったセッションより前の
    // 時点で更新が止まっている。それでも解決は *このパネル自身の* ログを
    // 返さなければならない — 以前の実装は最新のログか、このパネルの最終
    // ターンより後に始まったログを追いかけ、別セッションの会話を開いていた。
    let dir = tempfile::tempdir().expect("tmp dir");
    write_log(
        dir.path(),
        "idle-with-subagent",
        &["my prompt", "my later prompt"],
        Duration::from_secs(3600),
    );
    write_log(
        dir.path(),
        "other-live-session",
        &["someone else's prompt"],
        Duration::ZERO,
    );
    // idle なセッションのサブエージェントのトランスクリプトを、自分の
    // サブディレクトリに置く。
    let sub = dir.path().join("idle-with-subagent").join("subagents");
    std::fs::create_dir_all(&sub).expect("subagent dir");
    write_log(&sub, "agent-aivy-1234", &["subagent work"], Duration::ZERO);

    assert_eq!(
        transcript_texts(dir.path(), "idle-with-subagent"),
        vec!["my prompt", "my later prompt"]
    );
}

#[test]
fn unknown_session_id_resolves_to_nothing() {
    // 解決不能な id に対しては「履歴なし」が正しい答え。ディレクトリに
    // たまたまあるログにフォールバックすると別の会話を見せてしまう。
    let dir = tempfile::tempdir().expect("tmp dir");
    write_log(dir.path(), "aaa", &["from aaa"], Duration::ZERO);

    assert!(session_log_in_dir(dir.path(), "no-such-session").is_none());
}

#[test]
fn empty_project_dir_resolves_to_nothing() {
    let dir = tempfile::tempdir().expect("tmp dir");
    assert!(session_log_in_dir(dir.path(), "aaa").is_none());
}

// /clear によるログのローテーション追跡
//
// /clear は Claude Code の書き込み先を新しい session id の .jsonl に移す。
// 新ファイルの先頭に /clear のコマンドレコードが入り、以降の会話はすべて
// そちらへ行く。旧ファイルには追記されないし、両者を結ぶ id もログに残らない。
// pin した id だけを見ていると clear 前で止まる、というのがここで防ぐバグ。

/// age 前を SystemTime で返す。
fn ago(age: Duration) -> SystemTime {
    SystemTime::now() - age
}

/// SystemTime をログ中のタイムスタンプ形式 (RFC3339) にする。
fn rfc3339(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()
}

/// タイムスタンプ付きの通常セッションログを書き、mtime を last_write に合わせる。
fn write_plain_log(dir: &Path, session_id: &str, start: SystemTime, turns: &[&str]) -> PathBuf {
    write_records(dir, session_id, start, &[], turns)
}

/// 実物と同じ並び: mode → caveat (isMeta) → /clear コマンド → 会話。
fn write_cleared_log(dir: &Path, session_id: &str, start: SystemTime, turns: &[&str]) -> PathBuf {
    let ts = rfc3339(start);
    let head = [
        format!(r#"{{"type":"mode","mode":"normal","sessionId":"{session_id}"}}"#),
        format!(
            r#"{{"type":"user","isMeta":true,"timestamp":"{ts}","sessionId":"{session_id}","message":{{"role":"user","content":"<local-command-caveat>Caveat</local-command-caveat>"}}}}"#
        ),
        format!(
            r#"{{"type":"user","timestamp":"{ts}","sessionId":"{session_id}","message":{{"role":"user","content":"<command-name>/clear</command-name>\n<command-args></command-args>"}}}}"#
        ),
    ];
    write_records(dir, session_id, start, &head, turns)
}

/// head をそのまま出力したあと、turns を user レコードとして書く。
/// mtime は最後のターンの時刻 (ターンが無ければ start) に合わせる。
fn write_records(
    dir: &Path,
    session_id: &str,
    start: SystemTime,
    head: &[String],
    turns: &[&str],
) -> PathBuf {
    let path = dir.join(format!("{session_id}.jsonl"));
    let mut f = File::create(&path).expect("create log");
    for line in head {
        writeln!(f, "{line}").expect("write head");
    }
    let mut last = start;
    for (i, turn) in turns.iter().enumerate() {
        last = start + Duration::from_secs(i as u64 + 1);
        let ts = rfc3339(last);
        writeln!(
            f,
            r#"{{"type":"user","timestamp":"{ts}","sessionId":"{session_id}","message":{{"role":"user","content":"{turn}"}}}}"#
        )
        .expect("write turn");
    }
    f.set_times(FileTimes::new().set_modified(last))
        .expect("set mtime");
    path
}

/// 他パネルの pin が無い前提での解決。
fn resolve(dir: &Path, pinned: &str, spawned_at: SystemTime) -> String {
    resolve_current_session_id(dir, pinned, spawned_at, &HashSet::new())
}

#[test]
fn clear_rotation_is_followed() {
    // clear 前の会話で止まらず、clear 後に書かれたログへ移ること。
    let dir = tempfile::tempdir().expect("tmp dir");
    let spawned = ago(Duration::from_secs(600));
    write_plain_log(dir.path(), "before", spawned, &["clear 前のプロンプト"]);
    write_cleared_log(
        dir.path(),
        "after",
        ago(Duration::from_secs(540)),
        &["clear 後のプロンプト"],
    );

    assert_eq!(resolve(dir.path(), "before", spawned), "after");
}

#[test]
fn repeated_clears_chain_to_the_newest_log() {
    // /clear を複数回。連鎖の末端 (いま書かれているログ) まで辿ること。
    let dir = tempfile::tempdir().expect("tmp dir");
    let spawned = ago(Duration::from_secs(900));
    write_plain_log(dir.path(), "first", spawned, &["最初"]);
    write_cleared_log(dir.path(), "second", ago(Duration::from_secs(600)), &["次"]);
    write_cleared_log(
        dir.path(),
        "third",
        ago(Duration::from_secs(300)),
        &["最後"],
    );

    assert_eq!(resolve(dir.path(), "first", spawned), "third");
}

#[test]
fn clear_with_no_output_yet_still_resolves() {
    // clear 直後、まだ 1 ターンも書かれていない状態。ローテーション先は
    // /clear レコードだけを持つが、それでもそちらへ移ること
    // (clear 前の会話を見せてはいけない)。
    let dir = tempfile::tempdir().expect("tmp dir");
    let spawned = ago(Duration::from_secs(60));
    write_plain_log(dir.path(), "before", spawned, &["clear 前のプロンプト"]);
    write_cleared_log(dir.path(), "after", ago(Duration::from_secs(5)), &[]);

    assert_eq!(resolve(dir.path(), "before", spawned), "after");
}

#[test]
fn fresh_sibling_session_does_not_hijack() {
    // 回帰: 同じワークツリーで後から起動しただけの別セッション。/clear で
    // 始まっていないので後続ではない。以前はこれを続きとみなして他人の会話を
    // 表示していた (idle_session_is_not_displaced_by_a_newer_sibling 参照)。
    let dir = tempfile::tempdir().expect("tmp dir");
    let spawned = ago(Duration::from_secs(3600));
    write_plain_log(dir.path(), "mine", spawned, &["自分のプロンプト"]);
    write_plain_log(
        dir.path(),
        "someone-else",
        ago(Duration::from_secs(60)),
        &["別パネルのプロンプト"],
    );

    assert_eq!(resolve(dir.path(), "mine", spawned), "mine");
}

#[test]
fn log_pinned_by_another_panel_is_not_followed() {
    // 別パネルが自分のログとして pin している id は後続候補から外す
    // (そのパネルが --resume で clear 済みセッションを開いている場合)。
    let dir = tempfile::tempdir().expect("tmp dir");
    let spawned = ago(Duration::from_secs(600));
    write_plain_log(dir.path(), "mine", spawned, &["自分のプロンプト"]);
    write_cleared_log(
        dir.path(),
        "other-panel",
        ago(Duration::from_secs(540)),
        &["別パネルのプロンプト"],
    );

    let claimed = HashSet::from(["other-panel".to_string()]);
    assert_eq!(
        resolve_current_session_id(dir.path(), "mine", spawned, &claimed),
        "mine"
    );
}

#[test]
fn clear_predating_the_spawn_is_not_followed() {
    // 古いセッションを --resume した直後は pin したログの mtime が何日も前に
    // なりうる。起動より前に始まっていた /clear 始まりのログは自分の続きでは
    // ないので辿らない。
    let dir = tempfile::tempdir().expect("tmp dir");
    write_plain_log(
        dir.path(),
        "resumed",
        ago(Duration::from_secs(86400 * 3)),
        &["3 日前のプロンプト"],
    );
    write_cleared_log(
        dir.path(),
        "unrelated",
        ago(Duration::from_secs(86400 * 2)),
        &["無関係な会話"],
    );

    let spawned = ago(Duration::from_secs(60));
    assert_eq!(resolve(dir.path(), "resumed", spawned), "resumed");
}

#[test]
fn clear_long_after_the_last_turn_is_not_followed() {
    // 前のターンから何時間も空いて始まった /clear 始まりのログは、自分の
    // 続きなのか、同じワークツリーで別に起動した claude なのかログからは
    // 区別できない。推測せず自分のログに留まる (この場合はこのバグの修正が
    // 効かず、clear 前が表示される — 他人の会話を出すよりはまし、という判断)。
    let dir = tempfile::tempdir().expect("tmp dir");
    let spawned = ago(Duration::from_secs(86400));
    write_plain_log(dir.path(), "mine", spawned, &["自分のプロンプト"]);
    write_cleared_log(
        dir.path(),
        "much-later",
        ago(Duration::from_secs(7200)),
        &["別の会話かもしれない"],
    );

    assert_eq!(resolve(dir.path(), "mine", spawned), "mine");
}

#[test]
fn unrotated_session_resolves_to_itself() {
    let dir = tempfile::tempdir().expect("tmp dir");
    let spawned = ago(Duration::from_secs(600));
    write_plain_log(dir.path(), "solo", spawned, &["プロンプト"]);

    assert_eq!(resolve(dir.path(), "solo", spawned), "solo");
}

#[test]
fn missing_pinned_log_resolves_to_itself() {
    // ログがまだディスクに無い (起動直後) 段階では連鎖の起点が無い。
    // ディレクトリ内の別ログに飛ばず、pin した id をそのまま返すこと。
    let dir = tempfile::tempdir().expect("tmp dir");
    write_cleared_log(
        dir.path(),
        "someone-else",
        ago(Duration::from_secs(60)),
        &["別の会話"],
    );

    let spawned = ago(Duration::from_secs(120));
    assert_eq!(resolve(dir.path(), "not-on-disk", spawned), "not-on-disk");
}

#[test]
fn rotated_transcript_shows_only_post_clear_turns() {
    // 通しの確認: 解決したログを実際に読み、clear 前のターンが出ないこと。
    let dir = tempfile::tempdir().expect("tmp dir");
    let spawned = ago(Duration::from_secs(600));
    write_plain_log(dir.path(), "before", spawned, &["clear 前"]);
    write_cleared_log(
        dir.path(),
        "after",
        ago(Duration::from_secs(540)),
        &["clear 後"],
    );

    let current = resolve(dir.path(), "before", spawned);
    let texts = transcript_texts(dir.path(), &current);
    assert!(!texts.iter().any(|t| t.contains("clear 前")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("clear 後")), "{texts:?}");
}

#[test]
fn symlinked_session_log_resolves() {
    // migrate_session は grab したブランチのセッションを、新しい worktree の
    // プロジェクトディレクトリへシンボリックリンクする。解決はそのリンクを
    // たどれなければならない。
    let source = tempfile::tempdir().expect("tmp dir");
    let target = tempfile::tempdir().expect("tmp dir");
    let real = write_log(source.path(), "aaa", &["from aaa"], Duration::ZERO);
    std::os::unix::fs::symlink(&real, target.path().join("aaa.jsonl")).expect("symlink");

    assert_eq!(transcript_texts(target.path(), "aaa"), vec!["from aaa"]);
}
