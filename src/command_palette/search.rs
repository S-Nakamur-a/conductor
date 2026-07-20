//! Fuzzy filtering and scoring of palette commands against a query, grouped
//! by scope relative to the focused panel.

use super::commands::COMMANDS;
use super::types::{CommandScope, PaletteCommand, ScoredCommand, scope_rank};
use crate::keymap::{KeyContext, KeyMap};

/// Classify a command relative to the focused panel. Global-bound actions are
/// "global" even if a panel layer also binds them (e.g. `:` for the palette);
/// otherwise an action bound in the current panel's own layer is "current", and
/// anything else (bound only in another panel, runnable here via the palette) is
/// "other". Palette-only commands count as global.
fn command_scope(cmd: &PaletteCommand, keymap: &KeyMap, current: KeyContext) -> CommandScope {
    match cmd.action {
        None => CommandScope::Global,
        Some(action) => {
            if !keymap.keys_in_layer(KeyContext::Global, action).is_empty() {
                CommandScope::Global
            } else if current != KeyContext::Global
                && !keymap.keys_in_layer(current, action).is_empty()
            {
                CommandScope::Current
            } else {
                CommandScope::Other
            }
        }
    }
}

/// Fuzzy score for a command against a lowercased query; `None` if no match.
fn score_command(cmd: &PaletteCommand, query_lower: &str) -> Option<i32> {
    let label_lower = cmd.label.to_lowercase();
    let keywords_lower = cmd.keywords.to_lowercase();
    let category_lower = cmd.category.label().to_lowercase();
    let haystack = format!("{label_lower} {keywords_lower} {category_lower}");

    if !haystack.contains(query_lower) {
        return None;
    }

    let mut score: i32 = 0;
    if label_lower.starts_with(query_lower) {
        score += 100;
    }
    for word in label_lower.split(|c: char| !c.is_alphanumeric()) {
        if word.starts_with(query_lower) {
            score += 50;
            break;
        }
    }
    if label_lower.contains(query_lower) {
        score += 20;
    }
    if keywords_lower.contains(query_lower) {
        score += 10;
    }
    if category_lower.contains(query_lower) {
        score += 5;
    }
    Some(score)
}

/// Filter and score commands against a query, grouped by scope relative to the
/// focused panel (`current`). Returns all commands (sorted by scope) when the
/// query is empty, or matching commands sorted by scope then relevance.
///
/// The ordering is shared by the renderer (for grouped display) and the key
/// handler (for selection + execution), so `selected` indexes into this exact
/// sequence.
pub fn filter_commands(query: &str, keymap: &KeyMap, current: KeyContext) -> Vec<ScoredCommand> {
    let query_lower = query.to_lowercase();

    let mut results: Vec<ScoredCommand> = COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(i, cmd)| {
            let score = if query.is_empty() {
                0
            } else {
                score_command(cmd, &query_lower)?
            };
            Some(ScoredCommand {
                index: i,
                score,
                scope: command_scope(cmd, keymap, current),
            })
        })
        .collect();

    results.sort_by(|a, b| {
        scope_rank(a.scope)
            .cmp(&scope_rank(b.scope))
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| a.index.cmp(&b.index))
    });
    results
}
