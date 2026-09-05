use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use conductor_core::keymap::Action;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use revidere::Scope;

use super::*;
use crate::command::{CommandId, execute};
use crate::effect::{Effect, apply};
use crate::modal::Modal;
use crate::task::TaskResult;
use crate::testing::{TestRepo, pump, select_only_worktree, workspace_for};
use crate::workspace::Workspace;

/// 1 項目だけの成果物。base は実在するコミットでなければ diff が取れないので、
/// 骨組みの起点を差し替えて渡す。
fn artifact_json(base: &str, head: &str, sections: &str) -> String {
    revidere_fixtures::review(sections)
        .replace("\"aaa\"", &format!("\"{base}\""))
        .replace("\"bbb\"", &format!("\"{head}\""))
}

fn section(title: &str, path: &str, line: u32) -> String {
    format!(
        r#"[{{"title":"{title}","body":"body of {title}","importance":"core",
             "reason":"主目的そのもの",
             "ranges":[{{"path":"{path}","side":"new","start":{line},"end":{line}}}]}}]"#
    )
}

fn write_artifact(worktree: &Path, scope: Scope, json: &str) {
    let path = revidere::review::artifact_path(worktree, scope);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, json).unwrap();
}

/// 起点と前回の起点だけを差し替えた、区間の判定にだけ使う成果物。
fn round(base: &str, previous: Option<&str>) -> String {
    let since = previous
        .map(|p| {
            format!(
                r#","since_previous":{{"previous_head":"{p}","head":"h","files":[],"history_rewritten":false}}"#
            )
        })
        .unwrap_or_default();
    format!(
        r#"{{"schema":2,"base":"{base}","head":"h",
            "overview":{{"problem":"","change":"","mechanism":"","placement":"","scope":""}},
            "sections":[],"impacts":[],
            "coverage":{{"total":0,"classified":0,"unclassified":[],"conflicts":[],"unknown":[]}}{since}}}"#
    )
}

/// 成果物が無いのは正常。ここを Broken に畳むと、走らせていないだけの worktree で
/// エラーが出続ける。
#[test]
fn 成果物が無いのはエラーではない() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        artifact::load(dir.path(), Scope::Base),
        Outcome::Missing
    ));
}

/// 読めたテキストが成果物として通らないのは異常で、黙って Missing にしてはいけない。
/// 「走らせたのに何も出ない」の原因が見えなくなる。
#[test]
fn 壊れた成果物は理由を返す() {
    let dir = tempfile::tempdir().unwrap();
    write_artifact(dir.path(), Scope::Base, "{ not json");
    let Outcome::Broken(why) = artifact::load(dir.path(), Scope::Base) else {
        panic!("壊れた JSON は Broken になるはず");
    };
    assert!(why.contains("JSON"), "got: {why}");
}

/// 前の回の成果物が残っていても、それを今の回として出さない。起点が違えば区間が
/// 違うので、直しを読んでいるつもりで 1 つ前のラウンドを読むことになる。逆に、
/// 起点が一致していれば捨てない — 解析済みでも毎回作り直させることになる。
#[test]
fn 前回からの差分は起点が今の前回と一致するものだけ残す() {
    for (previous_base, keep) in [("p1", false), ("p2", true)] {
        let dir = tempfile::tempdir().unwrap();
        write_artifact(dir.path(), Scope::Base, &round("b0", Some("p2")));
        write_artifact(
            dir.path(),
            Scope::SincePrevious,
            &round(previous_base, None),
        );
        let outcome = artifact::load(dir.path(), Scope::SincePrevious);
        // git の無い一時ディレクトリなので diff は取れず Broken まで進む。ここで
        // 見たいのは捨てられたかどうかだけ。
        match (keep, &outcome) {
            (false, Outcome::Missing) | (true, Outcome::Broken(_)) => {}
            _ => panic!("base={previous_base} keep={keep}: {outcome:?}"),
        }
    }
}

#[test]
fn 確認の文言は成果物と今のコミットの関係で決まる() {
    let repo = TestRepo::new();
    let base = repo
        .git(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    repo.commit_in(&repo.root(), "a.txt", "alpha\nbeta\n", "second");
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();

    let mut panel = RevidereState::default();
    assert_eq!(panel.artifact(Some(&head)), Artifact::None);

    write_artifact(
        &repo.root(),
        Scope::Base,
        &artifact_json(&base, "analysed", &section("beta を足す", "a.txt", 2)),
    );
    panel.install(artifact::load(&repo.root(), Scope::Base));
    let loaded = panel.review().expect("読めているはず");
    assert_eq!(loaded.order.sections.len(), 1);

    assert_eq!(panel.artifact(Some(&head)), Artifact::Stale);
    // 成果物の側は短縮 oid。前方一致で見ないと Current に当たらない。
    let short = &head[..8];
    write_artifact(
        &repo.root(),
        Scope::Base,
        &artifact_json(&base, short, &section("beta を足す", "a.txt", 2)),
    );
    panel.install(artifact::load(&repo.root(), Scope::Base));
    assert_eq!(panel.artifact(Some(&head)), Artifact::Current);
}

#[test]
fn 説明もれがあっても成果物は読める() {
    let repo = TestRepo::new();
    let base = repo
        .git(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    repo.commit_in(&repo.root(), "a.txt", "alpha\nbeta\n", "second");
    // 骨組みは total 2 / classified 2 を書く。説明の付かなかった位置を 1 つ足す。
    let json = artifact_json(&base, "analysed", &section("beta を足す", "a.txt", 2)).replace(
        r#""classified":2,"unclassified":[]"#,
        r#""classified":1,"unclassified":[{"path":"a.txt","side":"new","line":1}]"#,
    );
    write_artifact(&repo.root(), Scope::Base, &json);

    let mut panel = RevidereState::default();
    let effects = panel.install(artifact::load(&repo.root(), Scope::Base));
    let review = panel.review().expect("読めているはず");
    assert!(effects.is_empty(), "警告は出すが読み込みは成功する");
    assert!(!review.is_complete());
    assert_eq!(review.unexplained(), 1);
}

fn analyze_task(branch: &str) -> Task {
    Task::Analyze {
        worktree: "/tmp/wt".into(),
        branch: branch.into(),
        scope: Scope::Base,
        force: false,
        api: Default::default(),
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

/// 成果物の置き場は worktree ごとに分かれているので、競合する対象はブランチだけ。
/// すべてを直列化すると、ある worktree のレビュアーが別の worktree の解析を始められない。
#[test]
fn 解析はブランチ単位で数え全停止で全部に止まれと伝える() {
    let mut panel = RevidereState::default();
    let (a, b) = (analyze_task("feature/a"), analyze_task("feature/b"));
    let (Task::Analyze { cancel: ca, .. }, Task::Analyze { cancel: cb, .. }) = (&a, &b) else {
        unreachable!()
    };
    let (ca, cb) = (Arc::clone(ca), Arc::clone(cb));
    panel.note_spawned(&a);
    panel.note_spawned(&b);
    assert!(panel.is_running("feature/a") && panel.is_running("feature/b"));

    // 終わった 1 本はブランチの枠を解放する。次の要求が「すでに実行中」にならない。
    panel.finished(
        "feature/a",
        AnalyzeOutcome::Done {
            coverage_complete: true,
        },
        "/tmp/wt".into(),
        "other",
    );
    assert!(!panel.is_running("feature/a") && panel.is_running("feature/b"));

    panel.abort();
    assert!(!panel.is_running("feature/b"));
    assert!(cb.load(Ordering::Relaxed), "走っていた側に止まれと伝わる");
    assert!(!ca.load(Ordering::Relaxed), "終わった側は触らない");
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// w → 成果物なし → 確認 → y → 解析 → 成果物が届き、w で 2 列ビュー、までの一本道。
#[test]
fn 解析の確認から2列ビューまで通る() {
    let repo = TestRepo::new();
    let base = repo
        .git(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    repo.commit_in(&repo.root(), "a.txt", "alpha\nbeta\n", "second");
    let (mut ws, mut svc) = workspace_for(&repo);
    select_only_worktree(&mut ws, &mut svc, &repo.root());

    // 成果物がまだ無いので、開こうとすると作る確認になる。
    let effects = execute(&mut ws, CommandId::ShowRevidere);
    apply(&mut ws, &mut svc, effects);
    let Some(Modal::RevidereConfirm(confirm)) = ws.modals.last() else {
        panic!("{:?}", ws.modals.last());
    };
    assert_eq!(confirm.artifact, Artifact::None);
    assert_ne!(ws.focus, Focus::Revidere, "確認の前にビューへは行かない");

    // y は解析を起こす。AI は呼ばず、届いた結果として成果物を差し込む。
    let mut modal = ws.modals.pop().unwrap();
    let effects = modal.update(key(KeyCode::Char('y')), &ws.ctx());
    let spawned = effects
        .iter()
        .any(|e| matches!(e, Effect::Spawn(Task::Analyze { .. })));
    assert!(spawned, "{effects:?}");

    write_artifact(
        &repo.root(),
        Scope::Base,
        &artifact_json(&base, "analysed", &section("beta を足す", "a.txt", 2)),
    );
    let effects = ws.accept(TaskResult::Analyzed {
        branch: ws.branch().to_string(),
        outcome: AnalyzeOutcome::Done {
            coverage_complete: true,
        },
    });
    apply(&mut ws, &mut svc, effects);
    pump(&mut ws, &mut svc);

    let review = ws.panels.revidere.review().expect("成果物が読めている");
    assert_eq!(review.order.sections.len(), 1);

    // 出来上がってから w で開く。2 列になり、左列に項目の見出しが出る。
    let effects = execute(&mut ws, CommandId::ShowRevidere);
    apply(&mut ws, &mut svc, effects);
    pump(&mut ws, &mut svc);
    assert_eq!(ws.focus, Focus::Revidere);
    ws.panels.revidere.show_overview(false);
    let layout = crate::layout::layout(&ws, Rect::new(0, 0, 120, 40));
    ws.sync_layout(&layout);
    ws.prepare();
    let cache = ws.panels.revidere.cache().expect("組み立て済み");
    let listed: String = cache
        .order_lines
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect();
    assert!(listed.contains("beta を足す"), "{listed}");
    assert!(!cache.diff_lines.is_empty());
}

/// 項目を選ぶと右の列がその項目の先頭へ動き、enter は列を渡り歩いてから Viewer へ出す。
#[test]
fn 左列で選び右列へ渡って_viewer_へ出す() {
    let repo = TestRepo::new();
    let base = repo
        .git(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    repo.commit_in(&repo.root(), "a.txt", "alpha\nbeta\n", "second");
    write_artifact(
        &repo.root(),
        Scope::Base,
        &artifact_json(&base, "analysed", &section("beta を足す", "a.txt", 2)),
    );
    let (mut ws, mut svc) = workspace_for(&repo);
    select_only_worktree(&mut ws, &mut svc, &repo.root());
    pump(&mut ws, &mut svc);
    assert!(
        ws.panels.revidere.review().is_some(),
        "worktree を選んだら読む"
    );

    ws.focus = Focus::Revidere;
    ws.panels.revidere.show_overview(false);
    let layout = crate::layout::layout(&ws, Rect::new(0, 0, 120, 40));
    ws.sync_layout(&layout);
    ws.prepare();

    assert_eq!(ws.panels.revidere.column(), Column::Order);
    ws.dispatch(Action::ExpandOrRight).unwrap();
    assert_eq!(ws.panels.revidere.column(), Column::Diff);
    ws.dispatch(Action::CollapseOrLeft).unwrap();
    assert_eq!(ws.panels.revidere.column(), Column::Order);

    // 左列の enter は右へ入るだけ。右列の enter が変更ファイルとして開く。
    let effects = ws.dispatch(Action::Select).unwrap();
    assert!(effects.is_empty(), "{effects:?}");
    let effects = ws.dispatch(Action::Select).unwrap();
    let [Effect::OpenChangedFile { path, line }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!((path.as_str(), *line), ("a.txt", Some(2)));

    // esc はビューを抜ける。
    let effects = ws.dispatch(Action::ExitSubPanel).unwrap();
    assert_eq!(effects, vec![Effect::Focus(Focus::Explorer)]);
}

/// 区間の切り替えは読みかけの位置を持ち越さず、その区間の成果物を読み直す。
#[test]
fn 区間の切り替えは読み直しを頼む() {
    let mut ws = Workspace::for_test();
    ws.focus = Focus::Revidere;
    assert_eq!(ws.panels.revidere.scope(), Scope::Base);
    let effects = ws.dispatch(Action::RevidereToggleScope).unwrap();
    assert_eq!(ws.panels.revidere.scope(), Scope::SincePrevious);
    let [Effect::Spawn(Task::LoadRevidere { scope, .. })] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(*scope, Scope::SincePrevious);
}

/// 判定は 1 か所で、確認の文言とキャッシュを捨てるかどうかが同じ値から出る。
#[test]
fn 同じコミットの作り直しは貯めた応答を捨てる() {
    let repo = TestRepo::new();
    let base = repo
        .git(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    repo.commit_in(&repo.root(), "a.txt", "alpha\nbeta\n", "second");
    write_artifact(
        &repo.root(),
        Scope::Base,
        &artifact_json(&base, "analysed", &section("beta を足す", "a.txt", 2)),
    );
    // head_oid は本物の worktree 一覧にしか載らない。差し替えると Stale に固定される。
    let (mut ws, mut svc) = workspace_for(&repo);
    pump(&mut ws, &mut svc);

    for (artifact, force) in [(Artifact::Current, true), (Artifact::Stale, false)] {
        // 解析したときの HEAD を書き換えて、今のコミットとの関係だけを動かす。
        let head = if artifact == Artifact::Current {
            repo.git(&["rev-parse", "--short", "HEAD"])
                .trim()
                .to_string()
        } else {
            "0000000".to_string()
        };
        write_artifact(
            &repo.root(),
            Scope::Base,
            &artifact_json(&base, &head, &section("beta を足す", "a.txt", 2)),
        );
        let effects = vec![ws.panels.revidere.reload(repo.root())];
        apply(&mut ws, &mut svc, effects);
        pump(&mut ws, &mut svc);

        let effects = execute(&mut ws, CommandId::AnalyzeRevidere);
        let [Effect::PushModal(Modal::RevidereConfirm(confirm))] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(confirm.artifact, artifact);
        let [Effect::Spawn(Task::Analyze { force: got, .. }), _] = confirm.on_yes.as_slice() else {
            panic!("{:?}", confirm.on_yes);
        };
        assert_eq!(*got, force, "{artifact:?}");
    }
}

/// 説明もれの件数は左列の枠題に出す。読む順のどこにも属さない変更があることを、
/// 項目を全部見る前に知りたい。
#[test]
fn 左列の枠題が説明もれの件数を言う() {
    let repo = TestRepo::new();
    let base = repo
        .git(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    repo.commit_in(&repo.root(), "a.txt", "alpha\nbeta\n", "second");
    let complete = artifact_json(&base, "analysed", &section("beta を足す", "a.txt", 2));
    let leaky = complete.replace(
        r#""classified":2,"unclassified":[]"#,
        r#""classified":1,"unclassified":[{"path":"a.txt","side":"new","line":1}]"#,
    );
    let mut ws = Workspace::for_test();

    for (json, want) in [(&complete, false), (&leaky, true)] {
        write_artifact(&repo.root(), Scope::Base, json);
        ws.panels
            .revidere
            .install(artifact::load(&repo.root(), Scope::Base));
        let title = render::order_title(&ws);
        assert!(title.contains("読む順 1 項目"), "{title}");
        assert_eq!(title.contains("説明の無い変更 1 件"), want, "{title}");
    }
}

/// 解析は数分かかる。終わった頃には端末で打鍵しているので、勝手に画面を持っていかない。
#[test]
fn 解析完了はフォーカスを奪わない() {
    let mut panel = RevidereState::default();
    for outcome in [
        AnalyzeOutcome::Done {
            coverage_complete: true,
        },
        AnalyzeOutcome::Done {
            coverage_complete: false,
        },
        AnalyzeOutcome::Failed("boom".into()),
    ] {
        let effects = panel.finished("main", outcome, "/tmp/wt".into(), "main");
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Focus(_))),
            "{effects:?}"
        );
        assert!(matches!(effects.first(), Some(Effect::Status(..))));
    }
}
