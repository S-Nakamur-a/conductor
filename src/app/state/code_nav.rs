//! コードナビゲーション: シンボル索引、ジャンプ履歴、それに付随するポップアップ。

use std::path::PathBuf;

use crate::jump_history::JumpHistory;
use crate::overlay::{HoverInfoOverlay, ReferencesOverlay, SymbolActionOverlay, SymbolHintOverlay};
use crate::semantic_index::SemanticIndex;
use crate::symbol_index::SymbolIndex;

/// 定義ジャンプ / 参照検索 / ホバー情報を成り立たせている状態。
///
/// 4 つのポップアップが汎用の OverlayManager ではなくここにあるのは、
/// どれも index の検索結果を表示するためのもので、単独では意味を持たないから。
/// OverlayManager 側は「排他的に 1 つだけ開くモーダル」を管理しており、
/// こちらのポップアップはビューアの上に重なって共存しうる。
pub struct CodeNav {
    /// tree-sitter で作ったシンボル索引 (バックグラウンドで構築)。
    ///
    /// [`semantic`](Self::semantic) が答えられない位置を埋める構文層でもある
    /// (`semantic_index::Bridge`)。索引がまだ無い / Rust ではない / 生成時から
    /// 内容が変わったファイル、のいずれでもここに落ちる。
    pub index: SymbolIndex,
    /// SCIP 索引による意味層。確信度つきで答え、答えられなければ構文層に回す。
    pub semantic: SemanticIndex,
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
    /// スクロールで画面の外へ出た、いま中にいるシンボルの宣言行。
    pub sticky: StickyHeader,
}

/// 描画のたびに引かれるので、同じ位置では索引に聞き直さない。
#[derive(Default)]
pub struct StickyHeader {
    pub asked: Option<(String, usize)>,
    pub declaration: Option<usize>,
}

impl CodeNav {
    /// 索引の探索起点をリポジトリルートに定めて初期化する。
    pub fn new(root: PathBuf) -> Self {
        Self {
            index: SymbolIndex::new(root),
            semantic: SemanticIndex::default(),
            history: JumpHistory::new(),
            references: ReferencesOverlay::default(),
            symbol_hint: SymbolHintOverlay::default(),
            symbol_action: SymbolActionOverlay::default(),
            hover_info: HoverInfoOverlay::default(),
            sticky: StickyHeader::default(),
        }
    }
}
