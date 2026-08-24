//! 行ビルダー。BuildCtx のセッションログエントリを、render が毎フレーム転送する
//! キャッシュ済みの Vec<Line<'static>> に変換する。パネル幅（あるいは展開トグル）が
//! 変わったときだけ再構築する。App から独立しているので、App を構築せずに単体で
//! 生成・テストできる。

use ratatui::text::Line;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::claude_log::{DisplayBlock, LogEntry, Role};

use super::block_render::{BlockPos, TranscriptStyles, render_block};

use super::palette::claude_markdown_theme;
use super::tool_lines::count_buckets;

/// build_lines がセッションログを描画済みの行に変換するために必要なものすべて。App から
/// 独立して借用するので、App を構築せずにビルダーを呼び出し（テストし）できる。
/// すべてのフィールドは共有参照である。
/// crate::ui::markdown::MarkdownCache::render_flavored は &self を取る
/// （キャッシュは内部的に RefCell）ので、&mut を必要とするフィールドはない。
pub(crate) struct BuildCtx<'a> {
    pub entries: &'a [LogEntry],
    pub cache: &'a crate::ui::markdown::MarkdownCache,
    pub theme: &'a crate::theme::Theme,
    pub syntax_set: &'a syntect::parsing::SyntaxSet,
    pub syntect_theme: &'a syntect::highlighting::Theme,
    /// tool_use/tool_result ブロックを展開するかどうか（conductor 独自の
    /// ctrl+o 相当のトグル）。
    pub expanded: bool,
}

/// エントリ間の空白の区切り行を表すブロックインデックス。
pub(crate) const SEPARATOR_BLOCK: usize = usize::MAX;

/// ガターマーカーが開始しうる最も右のカラム。すなわち width_risk_hole が走査する
/// 最後のカラムでもある。マーカーは helpers::with_marker と
/// helpers::fit_glyph_line によってカラム0に、tool_lines の "  ⎿  " / " ⎿  " という
/// プレフィックスによってカラム2に出力される。それより先は本文テキストであり、
/// 同じ文字であってもそこでは内容そのものである。
pub(crate) const MAX_GUTTER_GLYPH_COL: usize = 2;

/// 描画済みの1行がどこから来たかと、レンダラがその形状について知る必要のある唯一の情報。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineMeta {
    /// ctx.entries へのインデックス。
    pub entry: usize,
    /// そのエントリ内でのブロックのインデックス、または SEPARATOR_BLOCK。
    pub block: usize,
    /// そのブロック内でのこの行のインデックス。スクロールアンカーの第3要素であり、
    /// これがあることで異なる幅での再構築が、ブロックの先頭ではなく同じブロック内の
    /// 位置に戻ってこられる。
    pub offset: usize,
    /// ratatui の diff が不連続を検出し、crossterm バックエンドが後続テキストの前に
    /// 絶対カーソル移動を発行するよう、書き込まずに残す必要のあるカラム。
    /// width_risk_hole を参照。
    pub skip_col: Option<u16>,
}

/// build_lines の出力。転送する行と、行ごとの LineMeta。
pub(crate) struct BuiltLines {
    pub lines: Vec<Line<'static>>,
    pub meta: Vec<LineMeta>,
}

/// ctx.entries から行リスト全体を再構築する。
///
/// パネル幅が変わった（あるいは展開トグルが切り替わった）ときにのみ呼ばれる。
pub(crate) fn build_lines(ctx: &BuildCtx<'_>, width: usize) -> BuiltLines {
    let entries = ctx.entries;
    let styles = TranscriptStyles::default();
    // 本文の Markdown が装飾と揃うよう、Claude 風のテーマを作る。
    let md_theme = claude_markdown_theme(ctx.theme);

    let mut all_lines: Vec<Line<'static>> = Vec::new();
    let mut meta: Vec<LineMeta> = Vec::new();

    for (ei, entry) in entries.iter().enumerate() {
        let lines_before_entry = all_lines.len();
        // 折りたたんだ Counted 系ツール結果のためのエントリ単位の集計。
        // ctx.expanded が true のときも作るが参照されるのは false のときだけである。
        // どのみちブロック列を1度なめるだけのコストなので気にしない。
        let bucket_counts = count_buckets(entry);
        // エントリ内のすべての Counted 結果は1本の要約行にまとめて表される
        // （実測: 複数バケットがカンマ区切りの節の列として1行に描かれる）ので、
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

        // エントリ間の空行
        // 注釈だけのエントリは、その上のエントリの継続であって独立したターンではない。
        // CLI はスラッシュコマンド・その標準出力・引き継いだ各ファイルを別々のレコードとして
        // 記録するが、描画は途切れない1つのまとまりとして行う。
        //
        //     ❯ /compact
        //       ⎿  Compacted (ctrl+o to see full summary)
        //       ⎿  Read alpha.rs (42 lines)
        //
        // そのため、その手前に入るはずの区切りは抑制する。
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

/// entry が DisplayBlock::Annotation ブロックだけで構成されているかどうか。そうであれば
/// 新しいターンを始めるのではなく、上のエントリに接着する。
fn is_annotation_only(entry: &LogEntry) -> bool {
    !entry.blocks.is_empty()
        && entry
            .blocks
            .iter()
            .all(|b| matches!(b, DisplayBlock::Annotation { .. }))
}

/// line 上で最初に現れる幅の曖昧なガターグリフの直後のカラム。存在しなければ None。
///
/// ⏺/⎿/✻ は unicode-width では1カラムと計測されるが、多くの端末は2カラム幅で描画するため、
/// かつては行全体がずれる「scrollback のにじみ」が起きていた。Claude Code 自身はグリフの
/// 直後に絶対カラム（CHA）を発行することでこれを回避している。このセルを1つ未書き込みの
/// ままにしておくと、その位置で ratatui の diff が不連続になり、crossterm バックエンドが
/// 絶対位置指定の MoveTo を発行する。同じ手口である。super::render のテストで実際の
/// バックエンドに対して検証済み。
///
/// これには正しく処理すべき点が2つあり、素朴な走査ではどちらも間違える。
///
/// ガターだけを対象にすること。⏺/⎿/✻ は本文テキストにも普通の文字として現れる。
/// このアプリ自身のトランスクリプトは、貼り付けられた Claude Code の出力であふれている。
/// 穴は未書き込みのセルなので、本文にそれを開けると文字が1つ欠けるうえ、前フレームで
/// そこにあったものが残ってしまう。マーカーは必ずカラム0（helpers::with_marker、
/// helpers::fit_glyph_line）かカラム2（tool_lines の "  ⎿  " と " ⎿  " プレフィックス）
/// にしか置かれないので、走査は MAX_GUTTER_GLYPH_COL の後で止める。
///
/// カラムは char ではなく書記素クラスタ単位で進めること。char ごとに合計すると、
/// ZWJ シーケンス（家族の絵文字は2カラムだが7 char ある）を過大に数え、emoji
/// presentation シーケンスを過小に数えてしまい、穴が誤ったセルに置かれてしまう。
/// helpers::truncate_to_width や user_text::wrap_plain_text と同じ理屈である。
fn width_risk_hole(line: &Line<'_>) -> Option<u16> {
    let mut col: usize = 0;
    for span in &line.spans {
        for cluster in span.content.graphemes(true) {
            if col > MAX_GUTTER_GLYPH_COL {
                return None; // ガターを過ぎたので、ここから先はすべて内容
            }
            let w = UnicodeWidthStr::width(cluster);
            // w == 1 であること自体が曖昧さの正体である。この防御策は、
            // unicode-width が1カラムと呼ぶが端末は2カラムで描画しうるグリフのために
            // 存在する。異体字セレクタを伴うマーカーはすでに2カラムと計測されており、
            // 計測値と端末の描画が一致しているので穴は不要である。そこに穴を開けると
            // むしろ本文の先頭セルを空白にしてしまう。
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
