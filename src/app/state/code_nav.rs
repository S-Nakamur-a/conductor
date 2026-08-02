//! コードナビゲーション: シンボル索引、ジャンプ履歴、それに付随するポップアップ。

use std::path::PathBuf;

use crate::jump_history::JumpHistory;
use crate::overlay::{HoverInfoOverlay, ReferencesOverlay, SymbolActionOverlay, SymbolHintOverlay};
use crate::symbol_index::SymbolIndex;

/// 定義ジャンプ / 参照検索 / ホバー情報を成り立たせている状態。
///
/// 4 つのポップアップが汎用の OverlayManager ではなくここにあるのは、
/// どれも index の検索結果を表示するためのもので、単独では意味を持たないから。
/// OverlayManager 側は「排他的に 1 つだけ開くモーダル」を管理しており、
/// こちらのポップアップはビューアの上に重なって共存しうる。
pub struct CodeNav {
    /// tree-sitter で作ったシンボル索引 (バックグラウンドで構築)。
    pub index: SymbolIndex,
    /// ジャンプ元へ戻るための履歴。
    pub history: JumpHistory,
    /// 参照一覧のポップアップ。
    pub references: ReferencesOverlay,
    /// ジャンプ可能なシンボルであることを示すヒント表示。
    pub symbol_hint: SymbolHintOverlay,
    /// シンボルに対して実行できる操作のポップアップ。
    pub symbol_action: SymbolActionOverlay,
    /// マウスを乗せたときに出る定義プレビュー。
    pub hover_info: HoverInfoOverlay,
}

impl CodeNav {
    /// 索引の探索起点をリポジトリルートに定めて初期化する。
    pub fn new(root: PathBuf) -> Self {
        Self {
            index: SymbolIndex::new(root),
            history: JumpHistory::new(),
            references: ReferencesOverlay::default(),
            symbol_hint: SymbolHintOverlay::default(),
            symbol_action: SymbolActionOverlay::default(),
            hover_info: HoverInfoOverlay::default(),
        }
    }
}
