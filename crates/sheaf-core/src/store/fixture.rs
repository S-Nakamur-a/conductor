//! 検査で使う組み立て。複数のモジュールの検査が同じものを要るので 1 箇所に置く。

use crate::{IndexSource, Span, Store};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(super) fn span(start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> Span {
    Span {
        start_line,
        start_col,
        end_line,
        end_col,
    }
}

/// 索引 1 本を、索引ルート = root として投入する。
pub(super) fn load_single(
    index_path: &Path,
    root: &Path,
    expected: HashMap<PathBuf, String>,
) -> Store {
    Store::load(
        &[IndexSource {
            index: index_path.to_path_buf(),
            subroot: PathBuf::new(),
            expected,
        }],
        root,
    )
    .unwrap()
}
