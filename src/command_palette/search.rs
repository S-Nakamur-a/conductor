//! パレットコマンドをクエリに対してあいまいフィルタリング・スコアリングし、
//! フォーカス中パネルから見たスコープでグループ化する。

use super::commands::COMMANDS;
use super::types::{CommandScope, PaletteCommand, ScoredCommand, scope_rank};
use crate::keymap::{KeyContext, KeyMap};

/// コマンドをフォーカス中パネルとの関係で分類する。グローバルにバインドされた
/// アクションは、パネルのレイヤー側でも重ねてバインドされていても (パレットを
/// 開く : など) 常に global になる。そうでなければ、現在のパネル自身のレイヤーに
/// バインドされているアクションは current、それ以外 (別パネルだけにバインド
/// されていて、パレット経由でのみここから実行できるもの) は other になる。
/// パレット専用コマンドはグローバル扱いとする。
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

/// 小文字化したクエリに対するコマンドのあいまいスコア。マッチしなければ None。
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

/// コマンドをクエリに対してフィルタリング・スコアリングし、フォーカス中パネル
/// (current) から見たスコープでグループ化する。クエリが空ならスコープ順に
/// 並べた全コマンドを返し、そうでなければマッチしたコマンドをスコープ順・
/// 関連度順に並べて返す。
///
/// この並び順はレンダラ (グループ表示用) とキー操作ハンドラ (選択・実行用) の
/// 両方で共有される。選択位置はこの並びそのものへの添字になる。
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
