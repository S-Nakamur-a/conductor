//! フラットなディレクトリパス群をネストしたディレクトリツリーに変換する
//! 単体のヘルパー群。[tree](super::tree) が表示行をフラット化する際に使う。

use std::collections::BTreeMap;

/// ファイルパスを (ディレクトリ, ファイル名) に分割する。
/// トップレベルのファイルに対しては (".", filename) を返す。
pub(crate) fn split_dir_file(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(pos) => (path[..pos].to_string(), path[pos + 1..].to_string()),
        None => (".".to_string(), path.to_string()),
    }
}

/// ネストしたディレクトリツリーを組み立てるための中間構造。
pub(crate) struct NestedDir {
    pub(crate) path: String,
    pub(crate) is_leaf_dir: bool,
    pub(crate) children: BTreeMap<String, NestedDir>,
}

impl NestedDir {
    fn new(path: String) -> Self {
        Self {
            path,
            is_leaf_dir: false,
            children: BTreeMap::new(),
        }
    }

    pub(crate) fn has_subdirs(&self) -> bool {
        !self.children.is_empty()
    }
}

/// フラットなディレクトリパス群からネストしたディレクトリ構造を構築する。
pub(crate) fn build_nested_dirs(dir_paths: &[String]) -> NestedDir {
    let mut root = NestedDir::new(String::new());

    for dir_path in dir_paths {
        if dir_path == "." {
            // ルート直下のファイル。
            root.is_leaf_dir = true;
            continue;
        }

        let segments: Vec<&str> = dir_path.split('/').collect();
        let mut current = &mut root;

        for (i, seg) in segments.iter().enumerate() {
            let child_path = segments[..=i].join("/");
            let is_last = i == segments.len() - 1;

            current = current
                .children
                .entry(seg.to_string())
                .or_insert_with(|| NestedDir::new(child_path));

            if is_last {
                current.is_leaf_dir = true;
            }
        }
    }

    root
}
