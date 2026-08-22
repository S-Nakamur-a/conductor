//! ロード済みの索引を 1 つだけ持つ枠。

use super::Store;
use std::path::Path;

/// ロード済みの [`Store`] を 1 つだけ持つ枠。
///
/// 取り出す口をツリーのルート込みにしてある。[`Store`] は照合先のルートを内部に
/// 固定して持つので、別の worktree を映している画面から引くと、そこで内容が一致した
/// ファイルについて別ブランチの行番号を `Exact` として返す。ルートを渡さずに中身へ
/// 届く経路を残すと、利用側がこの検査を書き忘れられる。
#[derive(Debug, Default)]
pub struct Slot {
    store: Option<Store>,
}

impl Slot {
    /// `tree_root` に向いているストア。向いていなければ `None`。
    pub fn get(&self, tree_root: &Path) -> Option<&Store> {
        self.store.as_ref().filter(|s| s.root() == tree_root)
    }

    /// 別のツリーを見に行くことになったなら捨てる。読み直しを待たずに捨てるのは、
    /// 使えないストアを抱えたままにしないため。
    pub fn retarget(&mut self, tree_root: &Path) {
        if self.get(tree_root).is_none() {
            self.store = None;
        }
    }

    /// 背景ロードの結果を取り込む。要求した時点の向き先と今の向き先が違えば取り込まず、
    /// 捨てたことを `false` で返す。
    ///
    /// ロード中の切替はロードの起動そのものを拒まれることがあるので、`false` を見て
    /// 読み直しを起こさないと、新しいツリーの索引が二度と読まれない。
    pub fn accept(&mut self, requested: &Path, current: &Path, store: Option<Store>) -> bool {
        if requested != current {
            self.store = None;
            return false;
        }
        self.store = store;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::fixture::load_single;
    use protobuf::Message;
    use std::collections::HashMap;

    /// `root` に向いた、Document を 1 つ持つストア。
    fn store_for(root: &Path) -> Store {
        use protobuf::{EnumOrUnknown, MessageField};
        use scip::types::{Document, Index, Metadata, TextEncoding};

        let mut index = Index::new();
        let mut metadata = Metadata::new();
        metadata.text_document_encoding = EnumOrUnknown::new(TextEncoding::UTF8);
        index.metadata = MessageField::some(metadata);
        let mut doc = Document::new();
        doc.relative_path = "src/lib.rs".to_string();
        index.documents.push(doc);

        let path = root.join("index.scip");
        std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();
        load_single(&path, root, HashMap::new())
    }

    #[test]
    fn 別のツリーを指定して取り出すことはできない() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut slot = Slot::default();

        assert!(slot.accept(a.path(), a.path(), Some(store_for(a.path()))));
        assert!(slot.get(a.path()).is_some());
        assert!(slot.get(b.path()).is_none(), "別のツリーの問いに答えた");
    }

    #[test]
    fn 向き先が変わった結果は取り込まず読み直しを促す() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut slot = Slot::default();

        let accepted = slot.accept(a.path(), b.path(), Some(store_for(a.path())));
        assert!(!accepted, "要求時と今で向き先が違うのに取り込んだ");
        assert!(slot.get(a.path()).is_none());
        assert!(slot.get(b.path()).is_none());
    }

    #[test]
    fn 別のツリーへ向け直すと捨てる() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut slot = Slot::default();
        slot.accept(a.path(), a.path(), Some(store_for(a.path())));

        slot.retarget(a.path());
        assert!(slot.get(a.path()).is_some(), "同じツリーなのに捨てた");
        slot.retarget(b.path());
        assert!(slot.get(a.path()).is_none(), "別のツリーへ向けたのに残った");
    }
}
