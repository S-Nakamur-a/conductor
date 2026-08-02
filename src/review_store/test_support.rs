//! review_store 配下のサブモジュールのテストスイートが共有するテストヘルパー。
#![cfg(test)]

use std::path::Path;

use super::ReviewStore;

/// テスト用にインメモリの ReviewStore を作る。
pub(super) fn test_store() -> ReviewStore {
    ReviewStore::open(Path::new(":memory:")).expect("open in-memory DB")
}
