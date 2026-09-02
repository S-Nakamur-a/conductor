//! コマンド表の整合と、あいまい検索・スコープの並び。
//!
//! 到達性を担保するのはこのファイルと [crate::menu] のテスト。「すべての操作に
//! 行き先がある」を、コマンドを足すたびに古びる主張ではなく検証済みの性質にする。

use super::exec::NOT_YET;
use super::*;
use crate::effect::Effect;
use crate::modal::Modal;
use crate::workspace::{Focus, Workspace};
use conductor_core::keymap::{Action, KeyMap};

fn keymap() -> KeyMap {
    KeyMap::with_warnings(&toml::Table::new()).0
}

#[test]
fn 全コマンドが表に1度だけ出る() {
    for (i, command) in COMMANDS.iter().enumerate() {
        assert!(
            !COMMANDS[i + 1..].iter().any(|c| c.id == command.id),
            "{:?} is listed twice",
            command.id
        );
    }
    // find は表の網羅を前提に unwrap するので、そこも一緒に固定する。
    for command in COMMANDS {
        assert_eq!(find(command.id).label, command.label);
    }
}

#[test]
fn worktreeとレビューの入口が揃っている() {
    // キーを覚えていない人でも辿り着けるよう、それぞれにコマンドが要る。
    // pull_worktree がまるごと欠けていたことがある。
    let must_have = [
        Action::CreateWorktree,
        Action::DeleteWorktree,
        Action::SwitchBranch,
        Action::GrabBranch,
        Action::UngrabBranch,
        Action::PruneWorktrees,
        Action::MergeToMain,
        Action::PullWorktree,
        Action::CherryPick,
        Action::OpenPullRequest,
        Action::ReviewPullRequest,
        Action::PublishReview,
    ];
    for action in must_have {
        assert!(
            COMMANDS.iter().any(|c| c.action == Some(action)),
            "no command for {action:?}"
        );
    }
}

/// コマンドを持つ Action は、パネルが消費しなくても必ずそのコマンドへ落ちる。
#[test]
fn コマンドを持つactionはグローバルの解釈で拾われる() {
    let ws = Workspace::for_test();
    for command in COMMANDS {
        let Some(action) = command.action else {
            continue;
        };
        // パネルを跨ぐ語彙は別扱い。それ以外はコマンドの 1 本に落ちる。
        if matches!(
            action,
            Action::CycleFocusForward
                | Action::CycleFocusBackward
                | Action::FocusMenuBar
                | Action::CommandPalette
                | Action::OpenCommentList
        ) {
            continue;
        }
        let effects = crate::route::global_effects(&ws, action);
        let [Effect::Command(id)] = effects.as_slice() else {
            panic!("{:?}: {effects:?}", command.id);
        };
        assert_eq!(*id, command.id);
    }
}

#[test]
fn 絞り込みはグローバルと今の層を分ける() {
    let hits = filter("", &keymap(), KeyContext::Worktree);
    let scope_of = |action: Action| {
        hits.iter()
            .find(|h| COMMANDS[h.index].action == Some(action))
            .map(|h| h.scope)
    };
    assert_eq!(scope_of(Action::CreateWorktree), Some(Scope::Current));
    assert_eq!(scope_of(Action::Quit), Some(Scope::Global));
    assert_eq!(scope_of(Action::SearchInFile), Some(Scope::Other));
}

#[test]
fn 結果は今の層グローバルその他の順にまとまる() {
    let hits = filter("", &keymap(), KeyContext::Worktree);
    assert_eq!(hits.len(), COMMANDS.len(), "空クエリは全件");
    assert!(hits.windows(2).all(|w| w[0].scope <= w[1].scope));
}

#[test]
fn あいまい検索はラベルの頭を上に出す() {
    let hits = filter("quit", &keymap(), KeyContext::Global);
    assert_eq!(COMMANDS[hits[0].index].id, CommandId::Quit);

    // キーワードだけの一致も拾う。"grep" は Full-text Search のラベルに無い。
    let hits = filter("ripgrep", &keymap(), KeyContext::Global);
    assert_eq!(
        hits.iter()
            .map(|h| COMMANDS[h.index].id)
            .collect::<Vec<_>>(),
        [CommandId::SearchFullText]
    );

    assert!(filter("zzzz", &keymap(), KeyContext::Global).is_empty());
}

#[test]
fn 使えないコマンドは理由を出して何もしない() {
    let mut ws = Workspace::for_test();
    assert!(matches!(
        enabled(&ws, CommandId::SwitchRepo),
        Enabled::No(_)
    ));
    let effects = execute(&mut ws, CommandId::SwitchRepo);
    assert!(
        matches!(effects.as_slice(), [Effect::Status(..)]),
        "{effects:?}"
    );
    assert!(ws.modals.is_empty());

    ws.repo.known.push("/tmp/other".into());
    assert_eq!(enabled(&ws, CommandId::SwitchRepo), Enabled::Yes);
    let effects = execute(&mut ws, CommandId::SwitchRepo);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::PushModal(Modal::RepoPicker(_))]
        ),
        "{effects:?}"
    );
}

#[test]
fn 未実装のコマンドは理由付きで灰色になる() {
    let mut ws = Workspace::for_test();
    for (id, _) in NOT_YET {
        assert!(matches!(enabled(&ws, *id), Enabled::No(_)), "{id:?}");
        let effects = execute(&mut ws, *id);
        let [Effect::Status(_, text)] = effects.as_slice() else {
            panic!("{id:?}: {effects:?}");
        };
        assert!(text.contains("not implemented yet"), "{id:?}: {text}");
    }
}

#[test]
fn フォーカスのコマンドはフォーカスを動かす() {
    let mut ws = Workspace::for_test();
    for (id, focus) in [
        (CommandId::FocusViewer, Focus::Viewer),
        (CommandId::FocusWorktree, Focus::Worktree),
        (CommandId::FocusTerminalShell, Focus::TerminalShell),
    ] {
        let effects = execute(&mut ws, id);
        let [Effect::Focus(moved)] = effects.as_slice() else {
            panic!("{id:?}: {effects:?}");
        };
        assert_eq!(*moved, focus);
    }
}

#[test]
fn 最大化は交互に切り替わる() {
    let mut ws = Workspace::for_test();
    execute(&mut ws, CommandId::TogglePanelExpand);
    assert!(ws.chrome.maximized);
    execute(&mut ws, CommandId::TogglePanelExpand);
    assert!(!ws.chrome.maximized);
}

/// 境界は下限に当たると動かない。列が消えるとその列へ戻る手段が無くなる。
#[test]
fn 列の幅は下限で止まる() {
    let widths = |ws: &Workspace| {
        let l = &ws.config.layout;
        (
            l.explorer_width_pct,
            l.viewer_width_pct,
            100 - l.explorer_width_pct - l.viewer_width_pct,
        )
    };
    let mut ws = Workspace::for_test();
    for (focus, command) in [
        (Focus::Explorer, CommandId::ResizePaneLeft),
        (Focus::Explorer, CommandId::ResizePaneRight),
        (Focus::Viewer, CommandId::ResizePaneRight),
        (Focus::TerminalShell, CommandId::ResizePaneLeft),
    ] {
        ws.focus = focus;
        for _ in 0..40 {
            execute(&mut ws, command);
        }
        let (explorer, viewer, terminal) = widths(&ws);
        assert!(
            explorer >= 10 && viewer >= 10 && terminal >= 10,
            "{focus:?} {command:?}: {explorer}/{viewer}/{terminal}"
        );
    }

    ws.focus = Focus::TerminalClaude;
    for _ in 0..20 {
        execute(&mut ws, CommandId::ResizePaneUp);
    }
    assert_eq!(ws.config.layout.terminal_split_pct, 20);
    for _ in 0..40 {
        execute(&mut ws, CommandId::ResizePaneDown);
    }
    assert_eq!(ws.config.layout.terminal_split_pct, 80);
}

/// 境界を動かした値はディスクに書き戻す。書き込み自体は svc の仕事。
#[test]
fn 幅が動いたときだけ設定に書き戻す() {
    let mut ws = Workspace::for_test();
    ws.focus = Focus::Explorer;
    let effects = execute(&mut ws, CommandId::ResizePaneLeft);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Spawn(crate::task::Task::PersistConfig(_))]
        ),
        "{effects:?}"
    );
    ws.focus = Focus::Worktree;
    assert!(execute(&mut ws, CommandId::ResizePaneLeft).is_empty());
}

#[test]
fn git系コマンドは状態で灰色になる() {
    use conductor_core::git_engine::{GrabState, WorktreeInfo};

    fn worktree(path: &str, branch: &str, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            path: path.into(),
            branch: branch.into(),
            is_main,
            added: 0,
            modified: 0,
            deleted: 0,
            staged: 0,
            is_clean: true,
            ahead: None,
            behind: None,
            head_oid: None,
            head_time: None,
        }
    }

    let list = |ws: &mut Workspace, worktrees: Vec<WorktreeInfo>| {
        ws.accept(crate::task::TaskResult::Worktrees(Ok(worktrees)));
    };

    // main しか無いとき。
    let mut ws = Workspace::for_test();
    list(&mut ws, vec![worktree("/tmp/repo", "main", true)]);
    for id in [
        CommandId::MergeToMain,
        CommandId::CherryPick,
        CommandId::UngrabBranch,
        CommandId::PublishReview,
    ] {
        assert!(matches!(enabled(&ws, id), Enabled::No(_)), "{id:?}");
    }
    assert_eq!(enabled(&ws, CommandId::GrabBranch), Enabled::Yes);
    assert_eq!(enabled(&ws, CommandId::OpenPullRequest), Enabled::Yes);

    list(
        &mut ws,
        vec![
            worktree("/tmp/repo", "main", true),
            worktree("/tmp/wt/a", "feature/a", false),
        ],
    );
    crate::effect::apply(
        &mut ws,
        &mut conductor_svc::Services::new(),
        vec![Effect::SelectWorktree(1)],
    );
    assert_eq!(enabled(&ws, CommandId::MergeToMain), Enabled::Yes);
    assert_eq!(enabled(&ws, CommandId::CherryPick), Enabled::Yes);

    ws.accept(crate::task::TaskResult::GrabState(Ok(Some(GrabState {
        branch: "feature/a".into(),
        source_worktree: "/tmp/wt/a".into(),
        stash_branch: "feature/a__grab".into(),
        claude_session_id: None,
    }))));
    assert!(matches!(
        enabled(&ws, CommandId::GrabBranch),
        Enabled::No(_)
    ));
    assert_eq!(enabled(&ws, CommandId::UngrabBranch), Enabled::Yes);

    ws.review.install(Ok(crate::review::Snapshot {
        unpublished: 2,
        ..Default::default()
    }));
    assert_eq!(enabled(&ws, CommandId::PublishReview), Enabled::Yes);
}
