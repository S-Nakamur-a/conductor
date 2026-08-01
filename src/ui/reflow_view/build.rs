//! Line builder — turns a [`BuildCtx`]'s session-log entries into the cached
//! `Vec<Line<'static>>` that [`render`](super::render::render) blits each
//! frame. Rebuilt only when the panel width (or the expand toggle) changes.
//! Independent of `App` so it can be constructed and tested without one.


use ratatui::text::Line;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::claude_log::{DisplayBlock, LogEntry, Role};

use super::block_render::{BlockPos, TranscriptStyles, render_block};

use super::palette::claude_markdown_theme;
use super::tool_lines::count_buckets;

/// Everything [`build_lines`] needs to turn a session log into rendered
/// lines, borrowed independently of `App` so the builder can be called (and
/// tested) without constructing one. All fields are shared references —
/// [`crate::ui::markdown::MarkdownCache::render_flavored`] takes `&self`
/// (its cache is a `RefCell` internally), so no field needs `&mut`.
pub(crate) struct BuildCtx<'a> {
    pub entries: &'a [LogEntry],
    pub cache: &'a crate::ui::markdown::MarkdownCache,
    pub theme: &'a crate::theme::Theme,
    pub syntax_set: &'a syntect::parsing::SyntaxSet,
    pub syntect_theme: &'a syntect::highlighting::Theme,
    /// Whether to expand tool_use/tool_result blocks (conductor's own
    /// ctrl+o-equivalent toggle; wired up in S1).
    pub expanded: bool,
}

/// Block index standing for the blank separator line between entries.
pub(crate) const SEPARATOR_BLOCK: usize = usize::MAX;

/// Rightmost column a gutter marker can start at, and so the last column
/// [`width_risk_hole`] scans. Markers are emitted at column 0 by
/// [`helpers::with_marker`](super::helpers::with_marker) and
/// [`fit_glyph_line`](super::helpers::fit_glyph_line), and at column 2 by
/// [`tool_lines`](super::tool_lines)' `"  ⎿  "` / `" ⎿  "` prefixes. Anything
/// past this is body text, where the same characters are content.
pub(crate) const MAX_GUTTER_GLYPH_COL: usize = 2;

/// Where one rendered line came from, plus the one thing the renderer needs
/// to know about its shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineMeta {
    /// Index into `ctx.entries`.
    pub entry: usize,
    /// Index of the block within that entry, or [`SEPARATOR_BLOCK`].
    pub block: usize,
    /// Index of this line within its block — the third component of the
    /// scroll anchor, so a rebuild at a different width lands back inside the
    /// same block rather than just at its top.
    pub offset: usize,
    /// Column that must be left *unwritten* so ratatui's diff sees a
    /// discontinuity and the crossterm backend emits an absolute cursor move
    /// before the text that follows. See [`width_risk_hole`].
    pub skip_col: Option<u16>,
}

/// [`build_lines`]'s output: the lines to blit and one [`LineMeta`] each.
pub(crate) struct BuiltLines {
    pub lines: Vec<Line<'static>>,
    pub meta: Vec<LineMeta>,
}

/// Rebuild the full line list from `ctx.entries`.
///
/// Called only when the panel width changes (or the expand toggle flips).
pub(crate) fn build_lines(ctx: &BuildCtx<'_>, width: usize) -> BuiltLines {
    let entries = ctx.entries;
    let styles = TranscriptStyles::default();
    // 本文の Markdown が装飾と揃うよう、Claude 風のテーマを作る。
    let md_theme = claude_markdown_theme(ctx.theme);

    let mut all_lines: Vec<Line<'static>> = Vec::new();
    let mut meta: Vec<LineMeta> = Vec::new();

    for (ei, entry) in entries.iter().enumerate() {
        let lines_before_entry = all_lines.len();
        // 折りたたんだ `Counted` 系ツール結果 (§2.1) のためのエントリ単位の集計。
        // `ctx.expanded` が true でも作るが参照されるのは false のときだけ —
        // どのみち走査するブロック列を 1 度なめるだけのコスト。
        let bucket_counts = count_buckets(entry);
        // エントリ内のすべての `Counted` 結果を 1 本の要約行がまとめて表す
        // (実測: 複数バケットがカンマ区切りの節の列として 1 行に描かれる) ので、
        // バケットごとの集合ではなく単一のラッチで足りる。
        let mut summary_emitted = false;

        for (bi, block) in entry.blocks.iter().enumerate() {
            let pos = BlockPos {
                entry: ei,
                block: bi,
                is_user: entry.role == Role::User,
                entry_has_content: all_lines.len() > lines_before_entry,
                bucket_counts: &bucket_counts,
            };
            let lines = render_block(
                ctx,
                &styles,
                &md_theme,
                width,
                &pos,
                block,
                &mut summary_emitted,
            );
            meta.extend((0..lines.len()).map(|offset| LineMeta {
                entry: ei,
                block: bi,
                offset,
                skip_col: None,
            }));
            all_lines.extend(lines);
        }

        // ── エントリ間の空行 ────────────────────────────────────────────────
        // 注釈だけのエントリは、その上のエントリの *継続* であって独立したターン
        // ではない — CLI はスラッシュコマンド・その標準出力・引き継いだ各ファイルを
        // 別々のレコードとして記録するが、描画は途切れない 1 つのまとまりとして行う:
        //
        //     ❯ /compact
        //       ⎿  Compacted (ctrl+o to see full summary)
        //       ⎿  Read alpha.rs (42 lines)
        //
        // なので、その手前に入るはずの区切りは抑制する。
        let next_is_continuation = entries.get(ei + 1).is_some_and(is_annotation_only);
        if !next_is_continuation && all_lines.len() > lines_before_entry {
            all_lines.push(Line::from(""));
            meta.push(LineMeta {
                entry: ei,
                block: SEPARATOR_BLOCK,
                offset: 0,
                skip_col: None,
            });
        }
    }

    // 各行の「幅リスクの穴」は最後にまとめて解決する。こうしておくと、
    // 上のどの生成側も溝の幾何を気にしなくて済む。
    for (line, m) in all_lines.iter().zip(meta.iter_mut()) {
        m.skip_col = width_risk_hole(line);
    }

    debug_assert_eq!(all_lines.len(), meta.len());
    BuiltLines {
        lines: all_lines,
        meta,
    }
}

/// Whether `entry` consists solely of [`DisplayBlock::Annotation`] blocks, and
/// so glues to the entry above it instead of starting a new turn.
fn is_annotation_only(entry: &LogEntry) -> bool {
    !entry.blocks.is_empty()
        && entry
            .blocks
            .iter()
            .all(|b| matches!(b, DisplayBlock::Annotation { .. }))
}

/// The column immediately after the first width-ambiguous gutter glyph on
/// `line`, if it has one.
///
/// `⏺`/`⎿`/`✻` measure one column in `unicode-width` but many terminals draw
/// them two columns wide, which used to shift the whole row (the scrollback
/// "bleed"). Claude Code itself sidesteps this by emitting an absolute column
/// (CHA) right after the glyph; leaving this one cell unwritten makes
/// ratatui's diff discontinuous there, which makes the crossterm backend emit
/// an absolute `MoveTo` — the same trick. Verified against the real backend
/// in `super::render`'s tests.
///
/// Two things this must get right, both of which a naive scan gets wrong:
///
/// * **Only the gutter counts.** `⏺`/`⎿`/`✻` are also ordinary characters that
///   appear in body text — this app's own transcripts are full of pasted Claude
///   Code output. A hole is an *unwritten cell*, so punching one into body text
///   both drops a character and leaves whatever the previous frame had there.
///   A marker only ever sits at column 0 (`helpers::with_marker`,
///   `helpers::fit_glyph_line`) or column 2 (`tool_lines`' `"  ⎿  "` and
///   `" ⎿  "` prefixes), so the scan stops after [`MAX_GUTTER_GLYPH_COL`].
/// * **Columns advance by grapheme cluster, not by `char`.** Summing per `char`
///   over-counts a ZWJ sequence (a family emoji is 2 columns but 7 `char`s) and
///   under-counts an emoji-presentation sequence, which would put the hole on
///   the wrong cell. Same reasoning as `helpers::truncate_to_width` and
///   `user_text::wrap_plain_text`.
fn width_risk_hole(line: &Line<'_>) -> Option<u16> {
    let mut col: usize = 0;
    for span in &line.spans {
        for cluster in span.content.graphemes(true) {
            if col > MAX_GUTTER_GLYPH_COL {
                return None; // past the gutter — everything here is content
            }
            let w = UnicodeWidthStr::width(cluster);
            // `w == 1` *is* the ambiguity: the defence exists for glyphs
            // `unicode-width` calls one column while the terminal may draw two.
            // A marker carrying a variation selector already measures two, so
            // measurement and terminal agree and no hole is wanted — punching
            // one there would blank the body's first cell instead.
            if w == 1
                && cluster
                    .chars()
                    .next()
                    .is_some_and(super::glyphs::is_width_ambiguous)
            {
                return u16::try_from(col + w).ok();
            }
            col += w;
        }
    }
    None
}
