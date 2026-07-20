//! UTF-8 locale-override detection for spawned editors, and UTF-8-safe byte
//! chunking used when writing large payloads to a PTY.

/// Decide the locale-environment overrides needed so a spawned editor treats its
/// I/O as UTF-8, given the inherited `LC_ALL` / `LC_CTYPE` / `LANG` values.
///
/// Terminal editors derive their character encoding from the locale: vim, for
/// instance, falls back to `encoding=latin1` when no UTF-8 locale is active,
/// which garbles full-width / multi-byte (e.g. Japanese) text on input *and*
/// when reading the file back.
///
/// Returns `(sets, removes)`: environment variables to set, and variables to
/// remove, on the child command. When a UTF-8 locale is already active the
/// user's setting is respected and both lists are empty.
pub(super) fn utf8_locale_overrides(
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
) -> (Vec<(&'static str, &'static str)>, Vec<&'static str>) {
    fn denotes_utf8(value: &str) -> bool {
        let value = value.to_ascii_lowercase();
        value.contains("utf-8") || value.contains("utf8")
    }
    // An empty value (`LANG=`) is equivalent to the variable being unset.
    fn active(value: Option<&str>) -> Option<&str> {
        value.filter(|s| !s.is_empty())
    }

    // POSIX precedence: LC_ALL overrides LC_CTYPE, which overrides LANG.
    let effective = active(lc_all).or(active(lc_ctype)).or(active(lang));
    if effective.is_some_and(denotes_utf8) {
        return (Vec::new(), Vec::new());
    }

    // `C.UTF-8` is a locale-neutral UTF-8 locale: it exists on modern Linux and
    // is parsed by vim into `encoding=utf-8` on macOS even though it is not a
    // separately installed locale there. We set only `LC_CTYPE` (the category
    // that governs character encoding) to avoid changing the editor's message
    // language. A non-UTF-8 `LC_ALL` would shadow that, so drop it when present.
    let mut removes = Vec::new();
    if active(lc_all).is_some() {
        removes.push("LC_ALL");
    }
    (vec![("LC_CTYPE", "C.UTF-8")], removes)
}

/// Split `text` into consecutive sub-slices each at most `max` **bytes** long,
/// never cutting through a multi-byte UTF-8 character.
///
/// The PTY writers chunk large payloads (with a flush, and a small delay at the
/// chunk limit, between chunks) to stay under the kernel's PTY input buffer.
/// Splitting the raw byte slice at a fixed offset can land in the middle of a
/// multi-byte character; the receiving application then sees a truncated
/// sequence and may render a replacement / garbage glyph. Backing each split off
/// to the nearest character boundary keeps full-width / multi-byte input intact.
pub(super) fn utf8_chunks(text: &str, max: usize) -> Vec<&str> {
    assert!(max > 0, "chunk size must be positive");
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = max.min(rest.len());
        // Back off until `end` lands on a character boundary (it always does at
        // `rest.len()`, so this terminates).
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        // Defensive: only reachable if `max` is smaller than the first
        // character (never the case for the 1 KiB chunk size used here). Emit
        // that whole character so we always make forward progress.
        if end == 0 {
            end = rest.chars().next().map_or(rest.len(), char::len_utf8);
        }
        let (chunk, tail) = rest.split_at(end);
        chunks.push(chunk);
        rest = tail;
    }
    chunks
}
