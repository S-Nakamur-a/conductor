//! Full-text search (grep) engine — walks files respecting `.gitignore`
//! and searches for regex or literal patterns.

use std::fs;
use std::path::Path;
use std::sync::mpsc;

use ignore::WalkBuilder;
use regex::RegexBuilder;

/// A single match found by the grep search.
#[derive(Debug, Clone)]
pub struct GrepMatch {
    /// File path relative to the worktree root.
    pub file_path: String,
    /// 1-indexed line number.
    pub line_number: usize,
    /// Full content of the matching line.
    pub line_content: String,
    /// Byte offset of the match start within the line.
    pub match_start: usize,
    /// Byte offset of the match end within the line.
    pub match_end: usize,
}

/// Progress updates sent from the background search thread.
pub enum GrepProgress {
    /// A batch of results (sent periodically).
    Results(Vec<GrepMatch>),
    /// Search completed; carries the total match count.
    Done(usize),
    /// An error occurred.
    Error(String),
}

/// Maximum number of results before the search is truncated.
const MAX_RESULTS: usize = 5000;
/// Batch size — results are sent in chunks of this many.
const BATCH_SIZE: usize = 50;

/// Run a full-text search in a background thread.
///
/// `root` is the worktree directory to search in.
/// `pattern` is the search query (literal or regex).
/// `regex_mode` controls whether the pattern is interpreted as regex.
/// `case_sensitive` controls case sensitivity.
pub fn run_search(
    root: &Path,
    pattern: &str,
    regex_mode: bool,
    case_sensitive: bool,
    tx: mpsc::Sender<GrepProgress>,
) {
    let root = root.to_path_buf();
    let pattern = pattern.to_string();

    std::thread::spawn(move || {
        // Compile the regex pattern.
        let escaped = if regex_mode {
            pattern.clone()
        } else {
            regex::escape(&pattern)
        };
        let re = match RegexBuilder::new(&escaped)
            .case_insensitive(!case_sensitive)
            .build()
        {
            Ok(re) => re,
            Err(e) => {
                let _ = tx.send(GrepProgress::Error(format!("Invalid pattern: {e}")));
                return;
            }
        };

        let mut total = 0usize;
        let mut batch = Vec::with_capacity(BATCH_SIZE);

        let walker = WalkBuilder::new(&root)
            .hidden(true) // skip hidden files
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Skip directories.
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }

            let path = entry.path();

            // Read file contents, skip binary files.
            let content = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue, // binary or unreadable
            };

            for (line_idx, line) in content.lines().enumerate() {
                if let Some(m) = re.find(line) {
                    let rel_path = match path.strip_prefix(&root) {
                        Ok(p) => p.to_string_lossy().to_string(),
                        Err(_) => path.to_string_lossy().to_string(),
                    };

                    batch.push(GrepMatch {
                        file_path: rel_path,
                        line_number: line_idx + 1,
                        line_content: line.to_string(),
                        match_start: m.start(),
                        match_end: m.end(),
                    });

                    total += 1;

                    if batch.len() >= BATCH_SIZE {
                        if tx.send(GrepProgress::Results(std::mem::take(&mut batch))).is_err() {
                            return; // receiver dropped (search cancelled)
                        }
                        batch = Vec::with_capacity(BATCH_SIZE);
                    }

                    if total >= MAX_RESULTS {
                        // Flush remaining batch.
                        if !batch.is_empty() {
                            let _ = tx.send(GrepProgress::Results(batch));
                        }
                        let _ = tx.send(GrepProgress::Done(total));
                        return;
                    }
                }
            }
        }

        // Flush remaining batch.
        if !batch.is_empty() {
            let _ = tx.send(GrepProgress::Results(batch));
        }
        let _ = tx.send(GrepProgress::Done(total));
    });
}
