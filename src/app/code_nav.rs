//! シンボルインデックス・意味索引のバックグラウンド構築の起点。
//!
//! コードナビゲーション本体は [crate::viewer::code_nav] にある。ここに残る 2 メソッドは
//! App のライフサイクル (起動時・worktree 切り替え時・ファイルシステム変更時) から直接
//! 呼ばれる背景タスクの起点なので、Viewer パネル固有の責務ではなく App 側に置く。

use super::App;

impl App {
    /// 現在選択中のワークツリーを対象に、シンボルインデックスの構築を
    /// バックグラウンドで開始する。
    ///
    /// 各呼び出し元ではなくここでインデックスの対象を合わせ直すことで、両者が
    /// ずれないようにしている。インデックスはビューアが表示しているツリーを
    /// 説明していなければならず、ビルドを望むあらゆる経路――起動時、ワーク
    /// ツリー切り替え、ファイルシステム変更――は同じツリーを対象にしたい。
    /// [crate::symbol_index::SymbolIndex::set_root] はルートが変わっていなければ
    /// 何もしないので、ファイルシステム変更の経路もその場での再構築になる。
    ///
    /// すでに実行中のビルドは、置き換えずに完了まで走らせておく。ワークツリーの選択変更は
    /// リストをスクロールできる速さで届くので、これが無いと 10 個をドラッグ通過するだけで
    /// 10 個のフルツリー解析が並行して走る (BackgroundOp は中断できず、join handle を捨てても
    /// ワーカーは完走する)。その間索引は使えず、ちょうど操作しているときにナビゲーションが
    /// 死ぬ。置き換えられたビルドは世代チェックで自分の結果を捨てる。
    pub fn start_symbol_index_build(&mut self) {
        self.code_nav.index.set_root(self.selected_worktree_path());
        if self.bg.symbol_index.is_running() {
            return;
        }
        let index = self.code_nav.index.clone();
        self.bg.symbol_index.start(move |tx| {
            let result = match index.build() {
                Ok(count) => Ok(count),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });
    }

    /// 索引ルートを調べ直し、置いてある索引を読む。どちらもツリーを歩くので背景で。
    ///
    /// 読んでいるファイルを渡すのは、そのファイルの索引ルートだけは索引が無くても
    /// 鍵を出しておくため。渡さないと、まだ索引の無いルートは鍵が出ず、生成が
    /// 始まらない。
    pub fn start_semantic_index_load(&mut self) {
        if self.bg.semantic_index.is_running() {
            return;
        }
        let repo_root = self.repo.path.clone();
        let tree_root = self.selected_worktree_path();
        let reading = self.viewer.content.current_file.clone();
        // 鍵を失ったルートを名指しで渡す。渡さないと、調査に選ばれないまま
        // 「鍵が無い」と言い続け、調査が毎フレーム走る。
        let wanted = self
            .code_nav
            .semantic
            .needs_survey(&tree_root)
            .unwrap_or_default();
        self.code_nav.semantic.invalidate_if_retargeted(&tree_root);
        self.bg.semantic_index.start(move |tx| {
            let (survey, store) = crate::semantic_index::survey_and_load(
                &repo_root,
                &tree_root,
                reading.as_deref().map(std::path::Path::new),
                &wanted,
            );
            let _ = tx.send((tree_root, survey, store));
        });
    }
}
