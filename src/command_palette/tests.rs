//! Tests for the command palette's data integrity and fuzzy search/scope
//! grouping behavior.

use super::types::scope_rank;
use super::*;
use crate::keymap::{Action, KeyContext, KeyMap};

fn keymap() -> KeyMap {
    KeyMap::new(&toml::Table::new())
}

#[test]
fn every_command_action_is_valid() {
    use keymap_suite::ActionName;
    // A Some(action) must round-trip through the action vocabulary, so a
    // palette entry can never point at a stale/renamed action.
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
    // Guards against silent omissions of high-value worktree commands —
    // `pull_worktree` was previously missing entirely.
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
    // Guards the PR-intake and AI-walkthrough entry points — each needs a
    // palette command so a user without a keybinding memorized can still
    // reach them.
    let must_have = [
        Action::ReviewPullRequest,
        Action::GenerateWalkthrough,
        Action::ForceGenerateWalkthrough,
        Action::PublishReview,
    ];
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
    // Focused on the worktree panel: create-worktree is a worktree-layer
    // action → Current; quit is global → Global.
    let scoped = filter_commands("", &km, KeyContext::Worktree);
    let scope_of = |action: Action| {
        scoped
            .iter()
            .find(|s| COMMANDS[s.index].action == Some(action))
            .map(|s| s.scope)
    };
    assert_eq!(scope_of(Action::CreateWorktree), Some(CommandScope::Current));
    assert_eq!(scope_of(Action::Quit), Some(CommandScope::Global));
    // A viewer-only action is neither global nor in the worktree layer.
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
