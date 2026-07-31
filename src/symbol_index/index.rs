//! `SymbolIndex`: the thread-safe, tree-sitter-backed index itself — building
//! it by walking the repository, and the definition/implementation/reference
//! query methods used by code navigation.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use super::CodeMask;
use super::extract_go::extract_go_symbols;
use super::extract_rust::extract_rust_symbols;
use super::extract_ts::extract_ts_symbols;
use super::model::{Reference, Symbol, SymbolKind};

/// Inner data protected by a mutex.
pub(super) struct IndexData {
    // `pub(super)` so the `tests` sibling module can seed/inspect symbols
    // directly, mirroring the pre-split file where both lived in one module.
    pub(super) symbols: Vec<Symbol>,
    pub(super) available: bool,
    /// Bumped by [`SymbolIndex::set_root`]. A build stamps this at its start
    /// and refuses to publish if it has moved on by the time it finishes.
    pub(super) generation: u64,
}

/// Thread-safe tree-sitter-based symbol index.
pub struct SymbolIndex {
    root: Arc<Mutex<PathBuf>>,
    // `pub(super)` for the same reason as `IndexData`'s fields — the `tests`
    // module reaches directly into the mutex to seed fixture data.
    pub(super) data: Arc<Mutex<IndexData>>,
}

impl SymbolIndex {
    /// Create a new empty symbol index rooted at `root`.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(Mutex::new(root)),
            data: Arc::new(Mutex::new(IndexData {
                symbols: Vec::new(),
                available: false,
                generation: 0,
            })),
        }
    }

    /// Point the index at a different tree, discarding what it holds.
    ///
    /// Marking it unavailable is the point, not a side effect: the symbols
    /// describe the old tree, and answering from them after the viewer has
    /// moved to another worktree is what produces a jump to the right file at
    /// a line number computed from a different branch — silently, since
    /// nothing about the result looks wrong. Going quiet until the rebuild
    /// lands is the honest answer.
    ///
    /// A rebuild already in flight cannot be stopped (`BackgroundOp` drops the
    /// join handle, and the worker writes its result whether or not anyone is
    /// still listening), so instead the generation moves and that worker's
    /// result is refused on arrival.
    pub fn set_root(&self, root: PathBuf) {
        let mut current = self.root.lock().unwrap();
        if *current == root {
            return;
        }
        *current = root;
        let mut data = self.data.lock().unwrap();
        data.symbols.clear();
        data.available = false;
        data.generation = data.generation.wrapping_add(1);
    }

    /// Build the index by parsing source files with tree-sitter.
    /// Returns the number of symbols indexed, or 0 if the build was superseded.
    pub fn build(&self) -> Result<usize> {
        // Read as a pair, under the root lock, because `set_root` moves both
        // together. Sampling them independently leaves a window where a build
        // picks up the old root and the new generation, and then publishes a
        // stale tree that the generation check waves through — the exact thing
        // the counter exists to stop. `set_root` takes these two locks in this
        // same order, so holding one to take the other cannot deadlock.
        let (root, generation) = {
            let root = self.root.lock().unwrap();
            (root.clone(), self.data.lock().unwrap().generation)
        };

        let mut parser = tree_sitter::Parser::new();
        let mut symbols = Vec::new();

        let walker = ignore::WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            let lang = match ext {
                "rs" => Lang::Rust,
                "go" => Lang::Go,
                "ts" | "tsx" => Lang::TypeScript,
                "js" | "jsx" => Lang::JavaScript,
                _ => continue,
            };

            let ts_lang: tree_sitter::Language = match lang {
                Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
                Lang::Go => tree_sitter_go::LANGUAGE.into(),
                Lang::TypeScript | Lang::JavaScript => {
                    if ext == "tsx" || ext == "jsx" {
                        tree_sitter_typescript::LANGUAGE_TSX.into()
                    } else {
                        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
                    }
                }
            };

            if parser.set_language(&ts_lang).is_err() {
                continue;
            }

            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let rel_path = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let tree = match parser.parse(&source, None) {
                Some(t) => t,
                None => continue,
            };

            match lang {
                Lang::Rust => {
                    extract_rust_symbols(tree.root_node(), &source, &rel_path, &mut symbols)
                }
                Lang::Go => extract_go_symbols(tree.root_node(), &source, &rel_path, &mut symbols),
                Lang::TypeScript | Lang::JavaScript => {
                    extract_ts_symbols(tree.root_node(), &source, &rel_path, &mut symbols)
                }
            }
        }

        Ok(self.publish(symbols, generation))
    }

    /// The generation a build stamps itself with when it starts.
    ///
    /// A test seam: `build` reads this under the root lock so the pair is
    /// sampled atomically, which this accessor cannot do on its own.
    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.data.lock().unwrap().generation
    }

    /// Install `symbols` as the index contents, unless the root has moved since
    /// `generation` was stamped. Returns how many were published — zero means
    /// the result was thrown away.
    ///
    /// Separate from [`Self::build`] so the discard rule can be exercised
    /// without interleaving threads: the ordering that matters (a build starts,
    /// the root moves, the build finishes) is expressible here as three plain
    /// calls, whereas driving it through `build` would mean racing a slow walk
    /// against a re-root and hoping the scheduler cooperates.
    pub(super) fn publish(&self, symbols: Vec<Symbol>, generation: u64) -> usize {
        let mut data = self.data.lock().unwrap();
        // The root moved while this build was walking it, so these symbols
        // describe a tree nobody is looking at any more. Publishing them would
        // overwrite whatever the newer build produces, if this one happens to
        // finish second.
        if data.generation != generation {
            return 0;
        }
        let count = symbols.len();
        data.symbols = symbols;
        data.available = true;
        count
    }

    /// Find definition symbols matching the given name.
    pub fn find_definitions(&self, name: &str) -> Vec<Symbol> {
        let data = self.data.lock().unwrap();
        data.symbols
            .iter()
            .filter(|s| {
                s.name == name && !matches!(s.kind, SymbolKind::Field | SymbolKind::EnumVariant)
            })
            .cloned()
            .collect()
    }

    /// Find implementation symbols matching the given name.
    pub fn find_implementations(&self, name: &str) -> Vec<Symbol> {
        let data = self.data.lock().unwrap();
        data.symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Impl && s.scope.as_deref() == Some(name))
            .cloned()
            .collect()
    }

    /// Find references to a symbol name by searching source files.
    /// Uses the `ignore` crate walker for respecting .gitignore.
    ///
    /// Two passes per file, and the second one is skipped almost everywhere:
    /// a plain regex line scan finds which files mention `name` at all, and
    /// only *those* get parsed with [`CodeMask`] to tell a real usage from a
    /// mention inside a comment or string. Most files in a repository don't
    /// contain any given name, so this keeps the tree-sitter parse — the
    /// expensive part, see `code_mask::CodeMask::compute` — off the common
    /// path instead of paying for it on every file up front.
    ///
    /// Skipping the parse "almost everywhere" holds for a distinctive name and
    /// fails for a common one: `new` occurs in nearly 200 files here, and
    /// parsing all of them costs ~157ms. Callers on the frame path must use
    /// [`Self::count_references_upto`] rather than this.
    pub fn find_references(&self, name: &str, root: &Path) -> Vec<Reference> {
        self.collect_references(name, root, usize::MAX)
    }

    /// Count references, giving up once `cap` of them are found.
    ///
    /// The hover popup shows this number beside a symbol, and it is redrawn
    /// whenever the pointer settles — on the UI thread, inside a 16ms frame
    /// budget. An exact count for a name like `new` means parsing every file
    /// that mentions it, which measured ~157ms and dropped ten frames. The
    /// popup does not need the exact figure to be useful, so the scan stops
    /// early and the caller renders the cap as "and more".
    ///
    /// Returns `(count, hit_cap)`.
    pub fn count_references_upto(&self, name: &str, root: &Path, cap: usize) -> (usize, bool) {
        let found = self.collect_references(name, root, cap);
        (found.len(), found.len() >= cap)
    }

    fn collect_references(&self, name: &str, root: &Path, cap: usize) -> Vec<Reference> {
        let pattern = match regex::Regex::new(&format!(r"\b{}\b", regex::escape(name))) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };

        let mut refs = Vec::new();
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .build();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            // Non-code extensions (docs, config) never hold a real reference,
            // only text that happens to match the name.
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(
                ext,
                "rs" | "py"
                    | "js"
                    | "ts"
                    | "jsx"
                    | "tsx"
                    | "go"
                    | "c"
                    | "cpp"
                    | "h"
                    | "hpp"
                    | "java"
                    | "rb"
                    | "swift"
                    | "kt"
                    | "scala"
                    | "zig"
            ) {
                continue;
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Pass 1: cheap regex scan, no parsing — same cost as before this
            // change. `hits` borrows `content`, so it stays alive only for
            // this file's iteration.
            let hits: Vec<(usize, &str)> = content
                .lines()
                .enumerate()
                .filter(|(_, line)| pattern.is_match(line))
                .collect();
            if hits.is_empty() {
                continue;
            }

            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            // Pass 2: only for files that actually mention `name`. Dispatches
            // on `rel_path`'s extension the same way the viewer's mask does.
            //
            // When there is no grammar for the language, the hits are kept
            // unfiltered rather than dropped. Elsewhere an unanalysable file
            // yields no navigation, which is the cautious answer because
            // offering a jump asserts something about one word. A reference
            // search asserts something about the whole repository, and there
            // the cautious-looking answer is the dangerous one: "no results"
            // reads as "there are none", so silently discarding every hit in
            // a language we cannot parse would state something false with the
            // same confidence as a real answer. Comment matches sitting in the
            // list are visible and dismissable; a missing list is not.
            let mask = CodeMask::compute(&content, &rel_path);
            for (i, line) in hits {
                let line_1 = i + 1;
                let is_code = !mask.is_supported()
                    || pattern
                        .find_iter(line)
                        .any(|m| mask.is_code_at_column(line, line_1, m.start()));
                if is_code {
                    refs.push(Reference {
                        file_path: rel_path.clone(),
                        line: line_1,
                        content: line.to_string(),
                    });
                    if refs.len() >= cap {
                        return refs;
                    }
                }
            }
        }

        refs
    }

    /// Whether the index has been built successfully.
    pub fn is_available(&self) -> bool {
        self.data.lock().unwrap().available
    }

    /// Return the root path of this index.
    pub fn root(&self) -> PathBuf {
        self.root.lock().unwrap().clone()
    }
}

// Allow cloning for background thread usage.
impl Clone for SymbolIndex {
    fn clone(&self) -> Self {
        Self {
            root: Arc::clone(&self.root),
            data: Arc::clone(&self.data),
        }
    }
}

// ── Language detection ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum Lang {
    Rust,
    Go,
    TypeScript,
    JavaScript,
}
