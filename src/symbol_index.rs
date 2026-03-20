//! Ctags-based symbol indexing for code navigation.
//!
//! Provides `SymbolIndex` which runs Universal Ctags in JSON mode,
//! parses the output, and supports definition/implementation/reference
//! lookups.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use anyhow::Result;

/// The kind of a symbol (function, struct, trait, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Struct,
    Trait,
    Impl,
    Type,
    Const,
    Module,
    Enum,
    EnumVariant,
    Field,
    Method,
    Macro,
    Static,
    Interface,
    Unknown,
}

impl SymbolKind {
    /// Parse a ctags `kind` field (full name or single letter) into a `SymbolKind`.
    pub fn from_ctags(kind: &str) -> Self {
        match kind {
            "function" | "f" => SymbolKind::Function,
            "struct" | "s" => SymbolKind::Struct,
            "trait" | "i" => SymbolKind::Trait,
            "implementation" | "c" => SymbolKind::Impl,
            "typedef" | "type" | "t" => SymbolKind::Type,
            "constant" | "C" => SymbolKind::Const,
            "module" | "n" => SymbolKind::Module,
            "enum" | "g" => SymbolKind::Enum,
            "enumerator" | "e" => SymbolKind::EnumVariant,
            "field" | "w" | "member" | "m" => SymbolKind::Field,
            "method" | "P" => SymbolKind::Method,
            "macro" | "d" => SymbolKind::Macro,
            "variable" | "v" => SymbolKind::Static,
            "interface" => SymbolKind::Interface,
            _ => SymbolKind::Unknown,
        }
    }
}

/// A symbol definition found by ctags.
#[derive(Debug, Clone)]
pub struct Symbol {
    /// The symbol name (e.g. "MyStruct", "my_function").
    pub name: String,
    /// The kind of symbol.
    pub kind: SymbolKind,
    /// Relative file path from the repository root.
    pub file_path: String,
    /// 1-indexed line number.
    pub line: usize,
    /// 0-indexed column (if available).
    pub column: usize,
    /// Scope (e.g. parent struct/module name), if available.
    pub scope: Option<String>,
}

/// A reference (usage) of a symbol found by text search.
#[derive(Debug, Clone)]
pub struct Reference {
    /// Relative file path from the repository root.
    pub file_path: String,
    /// 1-indexed line number.
    pub line: usize,
    /// The full text content of the line.
    pub content: String,
}

/// Inner data protected by a mutex.
struct IndexData {
    symbols: Vec<Symbol>,
    available: bool,
}

/// Thread-safe ctags-based symbol index.
pub struct SymbolIndex {
    root: Arc<Mutex<PathBuf>>,
    data: Arc<Mutex<IndexData>>,
}

impl SymbolIndex {
    /// Create a new empty symbol index rooted at `root`.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(Mutex::new(root)),
            data: Arc::new(Mutex::new(IndexData {
                symbols: Vec::new(),
                available: false,
            })),
        }
    }

    /// Check if `ctags` is available on the system.
    pub fn check_ctags_available(root: &Path) -> bool {
        Command::new("ctags")
            .arg("--version")
            .current_dir(root)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Build the index by running ctags and parsing JSON output.
    /// Returns the number of symbols indexed.
    pub fn build(&self) -> Result<usize> {
        let root = self.root.lock().unwrap().clone();

        if !Self::check_ctags_available(&root) {
            anyhow::bail!("ctags not available");
        }

        let output = Command::new("ctags")
            .args([
                "--output-format=json",
                "--fields=+nKS",
                "--kinds-all=*",
                "-R",
                ".",
            ])
            .current_dir(&root)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ctags failed: {stderr}");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut symbols = Vec::new();

        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                if entry.get("_type").and_then(|v| v.as_str()) != Some("tag") {
                    continue;
                }
                let name = entry
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let kind_str = entry
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let kind = SymbolKind::from_ctags(kind_str);
                let file_path = entry
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let line_no = entry
                    .get("line")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let column = entry
                    .get("end")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let scope = entry
                    .get("scope")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                // Normalize file path: strip leading "./"
                let file_path = file_path.strip_prefix("./").unwrap_or(&file_path).to_string();

                if !name.is_empty() && line_no > 0 {
                    symbols.push(Symbol {
                        name,
                        kind,
                        file_path,
                        line: line_no,
                        column,
                        scope,
                    });
                }
            }
        }

        let count = symbols.len();
        let mut data = self.data.lock().unwrap();
        data.symbols = symbols;
        data.available = true;
        Ok(count)
    }

    /// Find definition symbols matching the given name.
    pub fn find_definitions(&self, name: &str) -> Vec<Symbol> {
        let data = self.data.lock().unwrap();
        data.symbols
            .iter()
            .filter(|s| {
                s.name == name
                    && !matches!(s.kind, SymbolKind::Field | SymbolKind::EnumVariant)
            })
            .cloned()
            .collect()
    }

    /// Find implementation symbols matching the given name.
    pub fn find_implementations(&self, name: &str) -> Vec<Symbol> {
        let data = self.data.lock().unwrap();
        data.symbols
            .iter()
            .filter(|s| {
                s.kind == SymbolKind::Impl
                    && s.scope.as_deref() == Some(name)
            })
            .cloned()
            .collect()
    }

    /// Find references to a symbol name by searching source files.
    /// Uses the `ignore` crate walker for respecting .gitignore.
    pub fn find_references(&self, name: &str, root: &Path) -> Vec<Reference> {
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

            // Only search text files with common source extensions.
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if !matches!(
                ext,
                "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "go" | "c" | "cpp"
                    | "h" | "hpp" | "java" | "rb" | "swift" | "kt" | "scala"
                    | "zig" | "toml" | "yaml" | "yml" | "json" | "md"
            ) {
                continue;
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            for (i, line) in content.lines().enumerate() {
                if pattern.is_match(line) {
                    refs.push(Reference {
                        file_path: rel_path.clone(),
                        line: i + 1,
                        content: line.to_string(),
                    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_kind_from_ctags() {
        assert_eq!(SymbolKind::from_ctags("function"), SymbolKind::Function);
        assert_eq!(SymbolKind::from_ctags("f"), SymbolKind::Function);
        assert_eq!(SymbolKind::from_ctags("struct"), SymbolKind::Struct);
        assert_eq!(SymbolKind::from_ctags("s"), SymbolKind::Struct);
        assert_eq!(SymbolKind::from_ctags("trait"), SymbolKind::Trait);
        assert_eq!(SymbolKind::from_ctags("implementation"), SymbolKind::Impl);
        assert_eq!(SymbolKind::from_ctags("typedef"), SymbolKind::Type);
        assert_eq!(SymbolKind::from_ctags("enum"), SymbolKind::Enum);
        assert_eq!(SymbolKind::from_ctags("module"), SymbolKind::Module);
        assert_eq!(SymbolKind::from_ctags("blah"), SymbolKind::Unknown);
    }

    #[test]
    fn test_symbol_index_new() {
        let idx = SymbolIndex::new(PathBuf::from("/tmp"));
        assert!(!idx.is_available());
        assert_eq!(idx.root(), PathBuf::from("/tmp"));
    }

    #[test]
    fn test_find_definitions_empty() {
        let idx = SymbolIndex::new(PathBuf::from("/tmp"));
        let results = idx.find_definitions("foo");
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_ctags_json_line() {
        // Simulate what build() does for a single line.
        let line = r#"{"_type":"tag","name":"MyStruct","path":"./src/lib.rs","line":42,"kind":"struct"}"#;
        let entry: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(entry["name"].as_str(), Some("MyStruct"));
        assert_eq!(entry["kind"].as_str(), Some("struct"));
        assert_eq!(entry["line"].as_u64(), Some(42));
    }
}
