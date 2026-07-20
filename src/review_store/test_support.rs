//! Shared test helper for the `review_store` submodule test suites.
#![cfg(test)]

use std::path::Path;

use super::ReviewStore;

/// Create an in-memory ReviewStore for testing.
pub(super) fn test_store() -> ReviewStore {
    ReviewStore::open(Path::new(":memory:")).expect("open in-memory DB")
}
