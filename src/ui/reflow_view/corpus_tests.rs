//! 実際の Claude Code セッションログに対する不変条件のスイープ。
//!
//! ここでは期待する出力を一切書き下さない — あらゆる幅で、あらゆるトランスクリプトが
//! 満たすべき性質だけを検証する。これが狙いである: 実際のログには手書きでは思いつかない
//! ような入力が含まれる（壊れた UTF-8、ZWJ 絵文字、ネストしたコードフェンス、数メガバイトの
//! tool result）。以下の性質は、まさにそれらが破られたときにパネルが目に見えて壊れる
//! ものばかりである。
//!
//! オプトイン: CONDUCTOR_TRANSCRIPT_CORPUS に .jsonl セッションログのディレクトリを
//! 設定する。未設定ならスキップする。ログはリポジトリに含まれておらず、決して含めては
//! いけない — ファイルパス、プロンプト、tool の出力を含むため。

use std::path::PathBuf;

use syntect::highlighting::ThemeSet;
use unicode_width::UnicodeWidthStr;

use crate::claude_log::{LogEntry, load_session};
use crate::ui::markdown::MarkdownCache;

use super::build::{BuildCtx, build_lines};

const WIDTHS: [usize; 6] = [20, 40, 60, 80, 120, 200];

/// cargo test の実行が数秒で終わるよう、スイープに上限を設ける。コーパスは
/// 数百のログを持ちうるが、不変条件はコンテンツの「形」についてのものであり、
/// 大きなファイルほど新しい形を追加するのではなく同じ形を繰り返す。
const MAX_FILES: usize = 30;
const MAX_BYTES: u64 = 5 * 1024 * 1024;

fn corpus_files() -> Option<Vec<PathBuf>> {
    let dir = std::env::var_os("CONDUCTOR_TRANSCRIPT_CORPUS")?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES))
        .collect();
    // readdir の順序に依存せず、失敗が再現可能になるようソートする。
    files.sort();
    files.truncate(MAX_FILES);
    Some(files)
}

fn line_width(line: &ratatui::text::Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

fn texts(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

struct Harness {
    theme: crate::theme::Theme,
    syntax_set: syntect::parsing::SyntaxSet,
    syntect_theme: syntect::highlighting::Theme,
}

impl Harness {
    fn new() -> Self {
        Self {
            theme: crate::theme::Theme::default(),
            syntax_set: two_face::syntax::extra_newlines(),
            syntect_theme: ThemeSet::load_defaults()
                .themes
                .remove("base16-ocean.dark")
                .unwrap(),
        }
    }

    fn build(
        &self,
        cache: &MarkdownCache,
        entries: &[LogEntry],
        width: usize,
        expanded: bool,
    ) -> super::build::BuiltLines {
        let ctx = BuildCtx {
            entries,
            cache,
            theme: &self.theme,
            syntax_set: &self.syntax_set,
            syntect_theme: &self.syntect_theme,
            expanded,
        };
        build_lines(&ctx, width)
    }
}

#[test]
fn real_transcripts_hold_the_layout_invariants() {
    let Some(files) = corpus_files() else {
        eprintln!("CONDUCTOR_TRANSCRIPT_CORPUS unset — skipping corpus sweep");
        return;
    };
    assert!(!files.is_empty(), "corpus directory holds no .jsonl files");

    let h = Harness::new();
    let mut checked = 0usize;

    for path in &files {
        let entries = load_session(path);
        if entries.is_empty() {
            continue;
        }
        for expanded in [false, true] {
            for width in WIDTHS {
                let cache = MarkdownCache::new();
                let built = h.build(&cache, &entries, width, expanded);

                // (1) パネルをはみ出すものがあってはならない。これがはみ出し系バグの
                // 直接的な検出器であり、幅-1の安全マージンを外せた理由でもある。
                for (i, line) in built.lines.iter().enumerate() {
                    let w = line_width(line);
                    assert!(
                        w <= width,
                        "{}: line {i} is {w} cols at width {width} (expanded={expanded}): {:?}",
                        path.display(),
                        texts(&built.lines[i..=i])[0],
                    );
                }

                // (2) すべての行がメタデータを持たなければならない: レンダラは両者を
                // zip するので、meta が短いとビューが黙って切り詰められてしまう。
                assert_eq!(
                    built.lines.len(),
                    built.meta.len(),
                    "{}: line/meta length mismatch at width {width}",
                    path.display()
                );

                // (3) 2回構築した結果は一致しなければならない — Markdown キャッシュが
                // この経路の途中に挟まっており、古いキャッシュヒットがあればここに現れる。
                let again = h.build(&cache, &entries, width, expanded);
                assert_eq!(
                    texts(&built.lines),
                    texts(&again.lines),
                    "{}: rebuild at width {width} differed",
                    path.display()
                );

                checked += 1;
            }
        }
    }
    assert!(checked > 0, "corpus produced no displayable entries");
    eprintln!("corpus sweep: {} files x {} builds", files.len(), checked);
}

#[test]
fn rebuilding_at_a_previous_width_reproduces_it() {
    let Some(files) = corpus_files() else {
        eprintln!("CONDUCTOR_TRANSCRIPT_CORPUS unset — skipping corpus sweep");
        return;
    };
    let h = Harness::new();

    for path in &files {
        let entries = load_session(path);
        if entries.is_empty() {
            continue;
        }
        // 実際の描画経路と同じく、3回の構築すべてで同じキャッシュを1つ使う: 幅を
        // 行って戻したとき、途中の幅がキャッシュに残した何かではなく、元のレイアウトに
        // 戻らなければならない。
        let cache = MarkdownCache::new();
        let first = texts(&h.build(&cache, &entries, 80, false).lines);
        let _ = h.build(&cache, &entries, 40, false);
        let back = texts(&h.build(&cache, &entries, 80, false).lines);
        assert_eq!(
            first,
            back,
            "{}: 80 -> 40 -> 80 was not idempotent",
            path.display()
        );
    }
}
