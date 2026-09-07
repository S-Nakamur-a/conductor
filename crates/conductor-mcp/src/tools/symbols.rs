//! search_symbols / read_symbol の中身。索引 (`Store`) を知らない純粋関数にして、
//! 索引を持たずにテストできるようにしてある。呼び出し側 (`tools.rs`) が
//! `Store::symbols()` の結果と入出力を橋渡しする。

use std::path::Path;

use conductor_core::semantic_index::{Body, Placement, SymbolEntry, SymbolKind, kind_label};

pub(crate) struct Query<'a> {
    pub text: Option<&'a str>,
    pub path: Option<&'a str>,
    pub kind: Option<KindFilter>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct KindFilter(&'static [SymbolKind]);

impl KindFilter {
    pub(crate) fn matches(self, kind: SymbolKind) -> bool {
        self.0.contains(&kind)
    }
}

/// 1 語が複数の [`SymbolKind`] にまたがるのは、言語ごとの
/// kind の割れ方 (Rust の `const`/`static` など) が呼び出し側の関心事ではないため。
pub(crate) fn parse_kind(word: &str) -> Result<KindFilter, String> {
    use SymbolKind::*;
    let set: &[SymbolKind] = match word {
        "function" => &[Function, Method],
        "method" => &[Method],
        "struct" => &[Struct],
        "class" => &[Class],
        "enum" => &[Enum],
        "variant" => &[EnumMember],
        "trait" => &[Trait],
        "interface" => &[Interface],
        "field" => &[Field],
        "const" => &[Constant, Static],
        "type" => &[TypeAlias, AssociatedType],
        "module" => &[Module, Package],
        other => {
            return Err(format!(
                "Unknown kind '{other}'. Use one of: function, method, struct, class, enum, variant, trait, interface, field, const, type, module."
            ));
        }
    };
    Ok(KindFilter(set))
}

pub(crate) fn entry_path(entry: &SymbolEntry) -> &Path {
    match &entry.at {
        Placement::Exact { definition, .. } => &definition.path,
        Placement::Stale { path } => path,
    }
}

fn sort_key(entry: &SymbolEntry) -> (&Path, u32) {
    match &entry.at {
        Placement::Exact { definition, .. } => (definition.path.as_path(), definition.line),
        Placement::Stale { path } => (path.as_path(), 0),
    }
}

/// 絞り込みと順位付け。`limit` はここでは使わない — 全件返し、切り詰めと
/// 「あと N 件」の表示は [`render_list`] が受け持つ。
pub(crate) fn select<'a>(entries: &'a [SymbolEntry], q: &Query) -> Vec<&'a SymbolEntry> {
    let mut hits: Vec<(u8, &SymbolEntry)> = entries
        .iter()
        .filter(|e| q.kind.is_none_or(|k| k.matches(e.detail.kind)))
        .filter(|e| {
            q.path
                .is_none_or(|p| entry_path(e).to_string_lossy().contains(p))
        })
        .filter_map(|e| match q.text {
            Some(text) => rank(e, text).map(|r| (r, e)),
            None => Some((0, e)),
        })
        .collect();
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| sort_key(a.1).cmp(&sort_key(b.1)))
    });
    hits.into_iter().map(|(_, e)| e).collect()
}

/// `Type::name` / `Type.name` は前半を container、後半を名前の照合に使う。
fn rank(entry: &SymbolEntry, text: &str) -> Option<u8> {
    let (qualifier, name) = split_qualifier(text);
    if let Some(qualifier) = qualifier
        && !container_matches(entry.detail.container.as_deref(), qualifier)
    {
        return None;
    }
    let entry_name = entry.name.to_lowercase();
    let name = name.to_lowercase();
    if entry_name == name {
        Some(0)
    } else if entry_name.starts_with(&name) {
        Some(1)
    } else if entry_name.contains(&name) {
        Some(2)
    } else {
        None
    }
}

/// `select`/`rank` と違って曖昧な順位付けはせず、名前は完全一致だけを見る。
pub(crate) fn matches_name(entry: &SymbolEntry, query: &str, case_insensitive: bool) -> bool {
    let (qualifier, name) = split_qualifier(query);
    if let Some(qualifier) = qualifier
        && !container_matches(entry.detail.container.as_deref(), qualifier)
    {
        return false;
    }
    if case_insensitive {
        entry.name.eq_ignore_ascii_case(name)
    } else {
        entry.name == name
    }
}

/// container の最後の 1 セグメントと完全一致するか (大文字小文字を区別する)。
/// 区別しないと型 `Store` とそれを置くモジュール `store` を見分けられず、
/// この照合が直そうとしている誤答をそのまま再現する。
fn container_matches(container: Option<&str>, qualifier: &str) -> bool {
    container
        .and_then(last_segment)
        .is_some_and(|last| last == qualifier)
}

/// `::` と `.` のうち、より後ろにある方で 1 回だけ割る。
fn split_qualifier(text: &str) -> (Option<&str>, &str) {
    let colon = text.rfind("::").map(|i| (i, i + 2));
    let dot = text.rfind('.').map(|i| (i, i + 1));
    let cut = match (colon, dot) {
        (Some(c), Some(d)) => Some(if c.0 > d.0 { c } else { d }),
        (Some(c), None) => Some(c),
        (None, Some(d)) => Some(d),
        (None, None) => None,
    };
    match cut {
        Some((start, after)) => (Some(&text[..start]), &text[after..]),
        None => (None, text),
    }
}

pub(crate) fn render_list(hits: &[&SymbolEntry], total: usize, limit: usize) -> String {
    if total == 0 {
        return "No symbols match.".to_string();
    }
    let shown = total.min(limit);
    let mut lines = Vec::with_capacity(shown * 2 + 1);
    if total > shown {
        lines.push(format!(
            "{total} symbols, showing {shown} (narrow with kind or path)"
        ));
    } else {
        lines.push(format!("{total} symbols"));
    }
    for entry in hits.iter().take(shown) {
        lines.push(entry_line(entry));
        if let Some(sig) = signature_line(entry) {
            lines.push(format!("  {sig}"));
        }
    }
    lines.join("\n")
}

fn entry_line(entry: &SymbolEntry) -> String {
    let head = entry_head(entry);
    match &entry.at {
        Placement::Exact { definition, body } => {
            let range = match body {
                Some(b) => format!("  [{}-{}]", b.first_line + 1, b.last_line + 1),
                None => String::new(),
            };
            format!(
                "{}:{}  {head}{range}",
                definition.path.display(),
                definition.line + 1
            )
        }
        Placement::Stale { path } => format!(
            "{}  {head}  [stale: file changed since the index was built]",
            path.display()
        ),
    }
}

fn entry_head(entry: &SymbolEntry) -> String {
    let label = kind_label(entry.detail.kind);
    let name = qualified_name(entry);
    if label.is_empty() {
        name
    } else {
        format!("{label} {name}")
    }
}

/// 囲んでいるものが型のときだけ、その型を前に付ける。メンバー種別は必ず型に囲まれる。
/// `self` を取らない関連関数は kind が Function のまま container が型になるので、
/// 大文字始まりの綴りで型と判定する (モジュール・パッケージは小文字で綴られる)。
fn qualified_name(entry: &SymbolEntry) -> String {
    let member = matches!(
        entry.detail.kind,
        SymbolKind::Method
            | SymbolKind::Field
            | SymbolKind::EnumMember
            | SymbolKind::AssociatedType
    );
    if let Some(container) = entry.detail.container.as_deref()
        && let Some(last) = last_segment(container)
        && (member || last.starts_with(|c: char| c.is_ascii_uppercase()))
    {
        return format!("{last}::{}", entry.name);
    }
    entry.name.clone()
}

fn last_segment(container: &str) -> Option<&str> {
    let colon = container.rfind("::").map(|i| i + 2);
    let dot = container.rfind('.').map(|i| i + 1);
    let start = match (colon, dot) {
        (Some(c), Some(d)) => c.max(d),
        (Some(c), None) => c,
        (None, Some(d)) => d,
        (None, None) => 0,
    };
    let segment = &container[start..];
    (!segment.is_empty()).then_some(segment)
}

fn signature_line(entry: &SymbolEntry) -> Option<String> {
    let sig = entry.detail.signature.as_deref()?;
    let collapsed = sig.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then(|| truncate_chars(&collapsed, 120))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

const MAX_BODY_LINES: usize = 500;

pub(crate) fn render_body(
    entry: &SymbolEntry,
    read: impl FnOnce(&Path) -> std::io::Result<String>,
) -> String {
    let (path, line, body) = match &entry.at {
        Placement::Stale { path } => {
            return format!(
                "{} changed since the index was built; line numbers are unknown. Read the file, or rebuild the index (conductor: Repo ▸ Rebuild Code Index).",
                path.display()
            );
        }
        Placement::Exact { definition, body } => (definition.path.as_path(), definition.line, body),
    };

    if body.is_none() && matches!(entry.detail.kind, SymbolKind::Module | SymbolKind::Package) {
        return format!(
            "{} is a module; its definition is the whole file {}.",
            entry.name,
            path.display()
        );
    }

    let contents = match read(path) {
        Ok(text) => text,
        Err(e) => return format!("{}: failed to read file: {e}", path.display()),
    };
    let lines: Vec<&str> = contents.lines().collect();

    match body {
        Some(body) => render_full_body(entry, path, *body, &lines),
        None => render_declaration_only(entry, path, line, &lines),
    }
}

fn render_declaration_only(entry: &SymbolEntry, path: &Path, line: u32, lines: &[&str]) -> String {
    let declaration = lines.get(line as usize).copied().unwrap_or("");
    format!(
        "{}:{}  {}\n{declaration}\nThe index only records the enclosing block for this symbol; Read {} from line {}.",
        path.display(),
        line + 1,
        entry_head(entry),
        path.display(),
        line + 1
    )
}

fn render_full_body(entry: &SymbolEntry, path: &Path, body: Body, lines: &[&str]) -> String {
    let first = body.first_line as usize;
    let last_exclusive = (body.last_line as usize + 1).min(lines.len());
    let selected: &[&str] = if first < lines.len() {
        &lines[first..last_exclusive]
    } else {
        &[]
    };

    let head = format!(
        "{}:{}-{}  {}",
        path.display(),
        body.first_line + 1,
        body.last_line + 1,
        entry_head(entry)
    );

    if selected.len() <= MAX_BODY_LINES {
        format!("{head}\n{}", selected.join("\n"))
    } else {
        let shown = &selected[..MAX_BODY_LINES];
        let more = selected.len() - MAX_BODY_LINES;
        let next_offset = first + MAX_BODY_LINES + 1;
        format!(
            "{head}\n{}\n… truncated, {more} more lines. Read {} with offset {next_offset}.",
            shown.join("\n"),
            path.display()
        )
    }
}

#[cfg(test)]
mod tests;
