//! Free-standing helpers for turning flat directory paths into a nested
//! directory tree, used by [`tree`](super::tree) when flattening visible rows.

use std::collections::BTreeMap;

/// Split a file path into (directory, filename).
/// Returns `(".", filename)` for top-level files.
pub(crate) fn split_dir_file(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(pos) => (path[..pos].to_string(), path[pos + 1..].to_string()),
        None => (".".to_string(), path.to_string()),
    }
}

/// Intermediate structure for building nested directory tree.
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

/// Build a nested directory structure from flat directory paths.
pub(crate) fn build_nested_dirs(dir_paths: &[String]) -> NestedDir {
    let mut root = NestedDir::new(String::new());

    for dir_path in dir_paths {
        if dir_path == "." {
            // Root-level files.
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
