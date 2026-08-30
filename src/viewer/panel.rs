//! Viewer パネル/ホバーオーバーレイの描画と、その結果の状態への書き戻し。
//!
//! [render] 配下の関数はどれも `&App`/`&ViewerState` しか取らない純粋関数だが、
//! 描画の直前に済ませておくべきキャッシュの充填（diff 注釈、カーソル行の畳み
//! 展開、宣言貼り付け行の解決）は同フレーム内で `&mut App` を要る。この2つの
//! エントリポイントがその境界で、render 配下を呼ぶ前に済ませ、返ってきた値を
//! 呼び出し元のフィールドへ書き戻す。

use crate::app::{App, Focus};
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::viewer::render;
use crate::viewer::render::{HoverOutcome, RenderOutcome};

/// 与えられた area に Viewer（ファイル内容）パネルを描画する。
pub fn render_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    // 画面行マップをクリアし、diff/media モードで古いデータが使われないようにする。
    app.viewer.content.screen_row_map.clear();
    app.viewer.tab_row_hits.clear();

    // Summary 疑似ファイル: ブランチの変更サマリーがパネル全体を占める。
    // 下記の diff 注釈キャッシュ・カーソル行畳み展開・宣言貼り付け解決は
    // いずれもコード表示のための前処理なので、ここでは行わない。
    if app.viewer.is_summary() {
        let focused = app.focus == Focus::Viewer;
        let outcome = render::render_summary_view(frame, area, app, focused);
        app.viewer.summary_total_lines = outcome.total_lines;
        app.viewer.summary_scroll = outcome.scroll;
        return;
    }

    ensure_diff_annotations_cached(app);

    // file_scroll を書く経路（検索・定義ジャンプ・grep・履歴復元）はどれもここに
    // 合流するので、「飛んだ先が畳まれていたら開く」の判断はこの1か所で足りる。
    //
    // diff 表示は除く。そこでの file_scroll は diff カーソルの写しでしかなく、
    // 画面に出ない畳みを開いてしまうと、素の表示へ戻ったときに理由の分からない
    // 開き方をして見える。
    if !app.viewer.diff_view.diff_mode {
        app.viewer.reveal_cursor_line();
    }

    // 差分表示の file_scroll は diff カーソルの写しなので、行を囲むものを聞いても
    // 意味が無い。畳みがあると file_scroll 自身は画面に出ないことがある。
    let top_visible = if app.viewer.diff_view.diff_mode {
        None
    } else {
        let content = &app.viewer.content;
        content
            .folds
            .visible_from(content.file_scroll + 1, content.file_content.len())
            .next()
    };
    let sticky_declaration = top_visible.and_then(|line_1| app.sticky_declaration_line(line_1 - 1));

    let outcome = render::render(frame, area, app, sticky_declaration);
    apply_render_outcome(app, outcome);
}

fn apply_render_outcome(app: &mut App, outcome: RenderOutcome) {
    if let Some(m) = outcome.screen_row_map {
        app.viewer.content.screen_row_map = m;
    }
    if let Some(m) = outcome.screen_entry_map {
        app.viewer.diff_view.screen_entry_map = m;
    }
    if let Some(tab_row) = outcome.tab_row {
        app.viewer.tab_row_hits = tab_row.hits;
        app.viewer.tab_scroll = tab_row.scroll;
    }
    if let Some(md) = outcome.markdown_scroll {
        app.viewer.md_total_lines = md.total_lines;
        app.viewer.md_scroll = md.scroll;
    }
}

/// ホバー情報ポップアップと、開いている子レベルがあればそれを area の上に
/// 描画する。
pub fn render_hover_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(outcome) = render::render_hover_info_overlay(frame, area, app) else {
        return;
    };
    apply_hover_outcome(app, outcome);
}

/// ViewerState の diff 注釈キャッシュを、現在表示中のファイルについて確実に
/// 埋める。ファイルが変わったかキャッシュが無効化された場合（load_diff() の後
/// など）のみ再構築する。render 本体が共有借用を取る前に呼ぶ。
fn ensure_diff_annotations_cached(app: &mut App) {
    use crate::diff_state::{DiffLineTag, FileDiff, InlineSegment};

    let current_file = app.viewer.content.current_file.clone();

    // キャッシュがまだ有効かどうかを確認する。
    if app.viewer.content.cached_diff_annotations.is_some()
        && app.viewer.content.cached_diff_annotations_file == current_file
    {
        return;
    }

    let mut annotations = std::collections::HashMap::new();

    if let Some(ref current) = current_file {
        let insert_annotations = |file_diff: &FileDiff,
                                  map: &mut std::collections::HashMap<
            usize,
            (DiffLineTag, Vec<InlineSegment>),
        >| {
            for hunk in &file_diff.hunks {
                for line in &hunk.lines {
                    if line.tag == DiffLineTag::Insert
                        && let Some(n) = line.new_line_no
                    {
                        map.entry(n)
                            .or_insert_with(|| (DiffLineTag::Insert, line.inline_segments.clone()));
                    }
                }
            }
        };

        for file_diff in &app.diff_state.files {
            if file_diff.path == *current {
                insert_annotations(file_diff, &mut annotations);
                break;
            }
        }
    }

    app.viewer.content.cached_diff_annotations = Some(annotations);
    app.viewer.content.cached_diff_annotations_file = current_file;
}

fn apply_hover_outcome(app: &mut App, outcome: HoverOutcome) {
    let hover_info = &mut app.code_nav.hover_info;
    hover_info.info_rect = outcome.base.info_rect;
    hover_info.refs_hit = outcome.base.refs_hit;
    hover_info.def_hit = outcome.base.def_hit;

    if let Some(refs_outcome) = outcome.refs
        && let Some(refs) = hover_info.refs.as_mut()
    {
        refs.rect = refs_outcome.rect;
        refs.row_hits = refs_outcome.row_hits;
        refs.scroll = refs_outcome.scroll;
    }
    if let Some(rect) = outcome.preview_rect
        && let Some(preview) = hover_info.refs.as_mut().and_then(|r| r.preview.as_mut())
    {
        preview.rect = rect;
    }
}
