//! コマンドパレットのデータ整合性と、あいまい検索・スコープごとのグループ化の
//! 挙動に対するテスト。

use super::types::scope_rank;
use super::*;
use crate::keymap::{Action, KeyContext, KeyMap};

fn keymap() -> KeyMap {
    KeyMap::new(&toml::Table::new())
}

#[test]
fn every_command_action_is_valid() {
    use keymap_suite::ActionName;
    // Some(action) はアクション語彙を往復できなければならない。こうしておくことで
    // パレットの項目が古い名前や改名済みのアクションを指したままになることがない。
    for cmd in COMMANDS {
        if let Some(action) = cmd.action {
            assert_eq!(
                Action::from_name(action.name()),
                Some(action),
                "command {:?} has an unrecognized action",
                cmd.id
            );
        }
    }
}

#[test]
fn comprehensive_worktree_commands_present() {
    // 重要な worktree コマンドがサイレントに抜け落ちるのを防ぐ回帰テスト。
    // 以前 pull_worktree がまるごと欠けていたことがある。
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
    ];
    for action in must_have {
        assert!(
            COMMANDS.iter().any(|c| c.action == Some(action)),
            "missing palette command for {action:?}"
        );
    }
}

#[test]
fn comprehensive_review_commands_present() {
    // PR 取り込みとレビューの入口を守る回帰テスト。キーバインドを覚えて
    // いないユーザでもたどり着けるよう、それぞれにパレットコマンドが要る。
    let must_have = [Action::ReviewPullRequest, Action::PublishReview];
    for action in must_have {
        assert!(
            COMMANDS.iter().any(|c| c.action == Some(action)),
            "missing palette command for {action:?}"
        );
    }
}

#[test]
fn scope_splits_global_from_current_layer() {
    let km = keymap();
    // worktree パネルにフォーカスした状態: create-worktree は worktree レイヤーの
    // アクションなので Current、quit はグローバルなので Global になる。
    let scoped = filter_commands("", &km, KeyContext::Worktree);
    let scope_of = |action: Action| {
        scoped
            .iter()
            .find(|s| COMMANDS[s.index].action == Some(action))
            .map(|s| s.scope)
    };
    assert_eq!(scope_of(Action::CreateWorktree), Some(CommandScope::Current));
    assert_eq!(scope_of(Action::Quit), Some(CommandScope::Global));
    // viewer 専用のアクションはグローバルでも worktree レイヤーでもない。
    assert_eq!(scope_of(Action::SearchInFile), Some(CommandScope::Other));
}

#[test]
fn results_are_grouped_current_then_global_then_other() {
    let km = keymap();
    let scoped = filter_commands("", &km, KeyContext::Worktree);
    let ranks: Vec<u8> = scoped.iter().map(|s| scope_rank(s.scope)).collect();
    assert!(
        ranks.windows(2).all(|w| w[0] <= w[1]),
        "scopes must be contiguous/ordered: {ranks:?}"
    );
}
