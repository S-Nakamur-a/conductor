//! エクスプローラ下半分の変更ファイル一覧と、ファイルごとのレビューコメント数
//! バッジの描画。

use crate::app::{App, Focus};
use crate::revidere::ArtifactState;
use crate::icons::file_icon;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem};

/// 右上に出す revidere の状態チップの幅。
///
/// 状態が変わっても幅は変わらない。当たり判定はここから導いていて、描画された
/// 文字列を測っているわけではないので、状態ごとに幅が動くと押せる場所がずれる。
const REVIDERE_BADGE_W: u16 = 10;

/// 状態チップが占める画面の列。左のタイトルと重なるほど狭ければ None
/// (描画側もクリック側もこれで揃って諦めるので、見えないチップは押せない)。
///
/// 右寄せタイトルは右枠の 1 つ内側で終わる。
pub(crate) fn revidere_badge_cols(app: &App, x: u16, width: u16) -> Option<std::ops::Range<u16>> {
    let title = diff_list_title(app.diff_state.files.len(), app.diff_state.error.is_some());
    badge_cols(x, width, title.chars().count() as u16)
}

fn badge_cols(x: u16, width: u16, title_w: u16) -> Option<std::ops::Range<u16>> {
    let end = x + width.checked_sub(1)?;
    let start = end.checked_sub(REVIDERE_BADGE_W)?;
    (start > x + title_w).then_some(start..end)
}

/// 状態チップの文字列。幅は常に [REVIDERE_BADGE_W]。
fn revidere_badge_label(state: ArtifactState, ui_tick: u64) -> String {
    format!(
        " {} review ",
        crate::ui::common::revidere_marker(state, ui_tick)
    )
}

/// Changed-files の各行のファイル名色が表す、4種類の git ステージ状態のいずれか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileStageState {
    Untracked,
    Unstaged,
    Staged,
    Committed,
}

/// git のステータスビットからファイルのステージ状態を分類する。None は
/// GitStatusMap にそのパスのエントリが全く無かったことを意味し、つまり
/// HEAD に対してクリーンな状態 — それがここでの Committed にあたる。
///
/// 判定順序が重要: 編集して git add し、さらに編集する、といった操作を
/// すると WT_* と INDEX_* の両方のビットが立つことがある。この場合は
/// unstaged を優先させたいので WT_* のチェックを先に行う。
fn file_stage_state(status: Option<git2::Status>) -> FileStageState {
    let Some(status) = status else {
        return FileStageState::Committed;
    };
    if status.is_wt_new() {
        FileStageState::Untracked
    } else if status.is_wt_modified()
        || status.is_wt_deleted()
        || status.is_wt_renamed()
        || status.is_wt_typechange()
    {
        FileStageState::Unstaged
    } else if status.is_index_new()
        || status.is_index_modified()
        || status.is_index_deleted()
        || status.is_index_renamed()
        || status.is_index_typechange()
    {
        FileStageState::Staged
    } else {
        FileStageState::Committed
    }
}

/// ステージ状態を対応するテーマ色にマッピングする。
fn status_color(theme: &crate::theme::Theme, state: FileStageState) -> ratatui::style::Color {
    match state {
        FileStageState::Untracked => theme.hint,
        FileStageState::Unstaged => theme.error,
        FileStageState::Staged => theme.warning,
        FileStageState::Committed => theme.success,
    }
}

/// diff 対象ファイル一覧（下半分）をディレクトリツリーとして描画する。
pub(super) fn render_diff_list(frame: &mut Frame, area: Rect, app: &App, panel_focused: bool) {
    use crate::diff_state::DiffListEntry;

    let theme = &app.theme;
    let icon_set = app.config.ui.icon_set();
    let vs_explorer = &app.viewer_state.explorer;
    let on_diff = vs_explorer.explorer_focus_on_diff_list;
    let diff_focused = panel_focused && on_diff;
    let border_color = if diff_focused {
        app.animated_border_color(Focus::Explorer)
    } else if panel_focused {
        theme.border_secondary
    } else if on_diff {
        app.animated_border_color(Focus::Explorer)
    } else {
        theme.border_unfocused
    };

    let total = app.diff_state.files.len();
    let title = diff_list_title(total, app.diff_state.error.is_some());

    // ボーダーの太さは Explorer カラム全体、タイトルの強調はその下半分に
    // フォーカスがあるかで決まる。
    let title_style = if diff_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let mut block = crate::ui::common::PanelChrome::new(theme, title, panel_focused, border_color)
        .with_title_style(title_style)
        .into_block();

    // revidere の状態は消えない場所に出す。ステータス行のフラッシュだけだと、
    // 数分かかる解析が終わったのか、そもそも走っているのかが後から分からない。
    if revidere_badge_cols(app, area.x, area.width).is_some() {
        let state = app.revidere_artifact_state();
        let mut style = Style::default()
            .fg(crate::ui::common::revidere_color(theme, state))
            .add_modifier(Modifier::BOLD);
        // hover は前景の下線で示す。背景を敷くとテーマによっては枠線ごと
        // 潰れて、どこが押せるのか逆に読めなくなる。
        if app.revidere.badge_hover {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        block = block.title_top(
            Line::from(Span::styled(
                revidere_badge_label(state, app.ui_tick),
                style,
            ))
            .alignment(Alignment::Right),
        );
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let scroll = vs_explorer.diff_list_scroll;

    // 以前は base 解決の失敗が完全に無音だった: 一覧が単に空で返ってきて
    // 「変更なし」に見えてしまっていた。メッセージを先頭行に固定して両者を
    // 混同しないようにする。このバナーは display_list の
    // 一部ではないため選択もできず、ナビゲーションキーが扱うインデックスも
    // ずらさない — コストはリストの高さ1行分だけ。改行はスペースに潰す。
    // 複数行の ListItem はここで確保した1行より多くの行を静かに消費して
    // しまい、List ウィジェットはパネル端で溢れた分を切り捨てるだけだから。
    let error_banner: Option<ListItem> = app.diff_state.error.as_deref().map(|msg| {
        ListItem::new(Span::styled(
            format!("  \u{26a0} {}", msg.replace('\n', " ")),
            Style::default().fg(theme.error),
        ))
    });
    let list_height = inner_height.saturating_sub(diff_list_banner_rows(error_banner.is_some()));

    let entry_items = app
        .diff_state
        .display_list
        .iter()
        .enumerate()
        .skip(scroll)
        .take(list_height)
        .filter_map(|(idx, entry)| match entry {
            DiffListEntry::Directory {
                name,
                depth,
                collapsed,
                ..
            } => {
                let indent = "  ".repeat(*depth);
                let arrow = crate::icons::expand_arrow(!*collapsed, icon_set);
                let icon = crate::icons::dir_icon(!*collapsed);
                // ディレクトリのアイコン色は行の色 (theme.info) と同じなので、
                // ファイル行と違って span を分ける必要がない。
                let prefix = format!("  {indent}{arrow} {} ", icon.glyph(icon_set));

                let style = crate::ui::common::list_row::row_style(
                    theme,
                    theme.info,
                    idx == vs_explorer.diff_list_selected,
                    diff_focused,
                    app.list_hover.diff_list.phase(idx),
                );

                // prefix を切り離すことで、hover 時の下線が名前の位置で
                // 止まるようにする（list_row::decoration_style を参照）。
                Some(ListItem::new(Line::from(vec![
                    Span::styled(prefix, crate::ui::common::list_row::decoration_style(style)),
                    Span::styled(name.clone(), style),
                ])))
            }
            DiffListEntry::File { file_index, depth } => {
                // インデックスアクセスではなく .get を使う: display_list と
                // ファイル vector は異なるティックで再構築されるため、片方が
                // 古いままレンダリングされるフレームがありうる。行をスキップ
                // すればチラつきで済むが、インデックスアクセスだと描画処理の
                // 内側からアプリ全体を落としかねない。上のファイルツリーも
                // 同様の対応をしている。
                let file_diff = app.diff_state.files.get(*file_index)?;

                let filename = file_diff.path.rsplit('/').next().unwrap_or(&file_diff.path);

                let indent = "  ".repeat(*depth);
                let icon = file_icon(filename);
                let prefix = format!("  {indent}");
                let glyph = format!("{} ", icon.glyph(icon_set));

                // ファイル名の色はファイルの git ステージ状態
                // （untracked / unstaged / staged / committed）を表す。
                // 行数はベースからの合計なので、その内訳がコミット済みか
                // 手元の編集かはこの色でしか分からない。
                let stage_state =
                    file_stage_state(app.viewer_state.tree.git_status.status(&file_diff.path));
                let base_fg = status_color(theme, stage_state);
                let style = crate::ui::common::list_row::row_style(
                    theme,
                    base_fg,
                    idx == vs_explorer.diff_list_selected,
                    diff_focused,
                    app.list_hover.diff_list.phase(idx),
                );
                // ファイル名以外の部分 — インデント、アイコン、行数 — は
                // hover の下線を外し、下線がファイル名だけに付くようにする。
                let decoration = crate::ui::common::list_row::decoration_style(style);
                // 行の背景/選択スタイルは style（row_style 経由）から来るが、
                // +added/-deleted はステージ状態に関わらず自前の前景色を保つ
                // ため、ラベルに焼き込まず別の span に分けている。
                let counts_style = |fg| Style {
                    fg: Some(fg),
                    ..decoration
                };

                // GitHub 風のコメントバッジ: レビューコメントがあるファイルには
                // 💬N を表示し、未解決のものが残っているかで色を変える。
                // アイコンの色はファイル種別を表すが、選択行では行の色に譲る。
                // 選択の背景色の上で種別色が読める保証が11テーマぶんには無いため。
                let icon_style = if idx == vs_explorer.diff_list_selected {
                    decoration
                } else {
                    counts_style(icon.role.color(theme))
                };

                let mut spans = vec![
                    Span::styled(prefix, decoration),
                    Span::styled(glyph, icon_style),
                    Span::styled(filename.to_string(), style),
                    Span::styled(
                        format!(" +{}", file_diff.added_lines),
                        counts_style(theme.diff_add),
                    ),
                    Span::styled(
                        format!(" -{}", file_diff.deleted_lines),
                        counts_style(theme.diff_del),
                    ),
                ];
                if let Some(badge) = comment_badge(app, &file_diff.path, theme) {
                    spans.push(badge);
                }
                if vs_explorer.viewed.contains(&file_diff.path) {
                    spans.push(Span::styled(
                        "  \u{2713}",
                        Style::default().fg(theme.success),
                    ));
                }
                Some(ListItem::new(Line::from(spans)))
            }
            DiffListEntry::Summary {} => {
                let selected = idx == vs_explorer.diff_list_selected;
                let mut style = crate::ui::common::list_row::row_style(
                    theme,
                    theme.accent,
                    selected,
                    diff_focused,
                    app.list_hover.diff_list.phase(idx),
                );
                // 非選択の SUMMARY 行は hover の有無に関わらず太字にする。
                // row_style は選択時以外は BOLD を適用しないため。
                if !selected {
                    style = style.add_modifier(Modifier::BOLD);
                }
                Some(ListItem::new(Line::from(vec![
                    Span::styled(
                        "  \u{25A3} ",
                        crate::ui::common::list_row::decoration_style(style),
                    ),
                    Span::styled("SUMMARY", style),
                ])))
            }
        });
    let items: Vec<ListItem> = error_banner.into_iter().chain(entry_items).collect();

    // 最後の項目より下の行（またはスクロールや高さ変更後の古い行）に
    // 前フレームの文字が残らないよう、先にクリアする。viewer と同じ
    // スクロール残像対策。
    frame.render_widget(ratatui::widgets::Clear, area);
    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// changed-files ブロックのタイトルを組み立てる。— diff error サフィックスは
/// 「何かが失敗してベースからの変更が欠けている」場合と、本当に
/// (0) である場合を区別するためのもの。これが無いと両者は同じ見た目になる。
/// あえて "base error" とはしていない: base ref の解決失敗はよくある原因の
/// 一つに過ぎず、HEAD が解決できない場合や merge-base が見つからない場合も
/// ここに含まれるため。
fn diff_list_title(total: usize, has_error: bool) -> String {
    if has_error {
        format!(" Changed files ({total}) — diff error ")
    } else {
        format!(" Changed files ({total}) ")
    }
}

/// changed-files 一覧の先頭でエラーバナーが占める行数。
///
/// この寸法に関する単一の情報源。3箇所がここに合わせる必要がある:
/// レンダラ（何行分のエントリが収まるか）、スクロールのページサイズ、
/// マウスハンドラ（画面上のどの行が display_list のどのインデックスに
/// 対応するか）。以前はこれらがずれてしまうことがあり、1行のずれが
/// クリック時に別のファイルを静かに開いてしまっていた。
pub(super) fn diff_list_banner_rows(has_error: bool) -> usize {
    usize::from(has_error)
}

/// ファイルパスに対して GitHub 風のコメント数バッジ（例:  💬3）を組み立てる。
/// レビューコメントが無ければ None。未解決のコメントがあればバッジは
/// accent 色になり、全て解決済みなら muted 色になる。
fn comment_badge(app: &App, file_path: &str, theme: &crate::theme::Theme) -> Option<Span<'static>> {
    use crate::review_store::CommentStatus;
    let mut total = 0usize;
    let mut unresolved = 0usize;
    for c in app
        .review_state
        .comments
        .iter()
        .filter(|c| c.file_path == file_path)
    {
        total += 1;
        if c.status == CommentStatus::Pending {
            unresolved += 1;
        }
    }
    if total == 0 {
        return None;
    }
    let color = if unresolved > 0 {
        theme.accent
    } else {
        theme.muted
    };
    Some(Span::styled(
        format!("  \u{1f4ac}{total}"),
        Style::default().fg(color),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    /// エラーサフィックスの存在意義そのもの: base 解決の失敗と、本当に
    /// クリーンなツリーが同じタイトルにレンダリングされてはならない。
    #[test]
    fn error_title_differs_from_a_genuine_zero() {
        assert_ne!(diff_list_title(0, true), diff_list_title(0, false));
        assert_eq!(diff_list_title(0, false), " Changed files (0) ");
        assert!(diff_list_title(0, true).contains("error"));
    }

    /// base 解決が失敗しても HEAD 基準の一覧は生き残るので、件数は 0 以外に
    /// なり、かつエラーマーカーも表示される — 両方が出ていること。
    #[test]
    fn error_title_keeps_the_count() {
        let title = diff_list_title(17, true);
        assert!(title.contains("(17)"), "{title}");
        assert!(title.contains("error"), "{title}");
    }

    /// レンダラ、スクロールのページサイズ、マウスの行→インデックス変換は
    /// いずれもバナーの行コストをここから導出する。1行でもずれると
    /// クリックで別のファイルが開いてしまうので、契約として固定する。
    #[test]
    fn banner_costs_exactly_one_row_and_only_when_erroring() {
        assert_eq!(diff_list_banner_rows(false), 0);
        assert_eq!(diff_list_banner_rows(true), 1);
    }

    /// 色の対応表を status_color の実装から独立して再現する — 期待する色を
    /// ここでテーマから再導出しておけば、status_color 内で2色を取り違えた
    /// バグも検出できる。
    fn diff_file_status_color(
        theme: &Theme,
        status: Option<git2::Status>,
    ) -> ratatui::style::Color {
        status_color(theme, file_stage_state(status))
    }

    #[test]
    fn diff_file_status_color_untracked_is_hint() {
        let theme = Theme::default();
        let status = Some(git2::Status::WT_NEW);
        assert_eq!(diff_file_status_color(&theme, status), theme.hint);
    }

    #[test]
    fn diff_file_status_color_unstaged_is_error() {
        let theme = Theme::default();
        let status = Some(git2::Status::WT_MODIFIED);
        assert_eq!(diff_file_status_color(&theme, status), theme.error);
    }

    #[test]
    fn diff_file_status_color_staged_is_warning() {
        let theme = Theme::default();
        let status = Some(git2::Status::INDEX_MODIFIED);
        assert_eq!(diff_file_status_color(&theme, status), theme.warning);
    }

    #[test]
    fn diff_file_status_color_committed_is_success() {
        let theme = Theme::default();
        // None は「GitStatusMap にこのパスのエントリが無い」ことを表し、
        // つまり HEAD に対してクリーンな状態。
        assert_eq!(diff_file_status_color(&theme, None), theme.success);
    }

    /// 状態が変わっても幅が変わらないこと。当たり判定は幅を定数から導いて
    /// いるので、ここがずれると「見えている場所と押せる場所」が食い違う。
    #[test]
    fn every_state_renders_the_same_width() {
        for state in [
            ArtifactState::None,
            ArtifactState::Running,
            ArtifactState::Fresh,
            ArtifactState::Stale,
        ] {
            let label = revidere_badge_label(state, 0);
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(label.as_str()),
                REVIDERE_BADGE_W as usize,
                "{state:?}: {label:?}"
            );
        }
    }

    /// 右寄せタイトルが実際に落ちる位置と、当たり判定の矩形が一致すること。
    /// この 2 つは別々に計算されているので、ratatui の右寄せの寸法が変われば
    /// クリックだけ 1 セルずれる、という壊れ方をする。
    #[test]
    fn the_hit_box_is_where_ratatui_puts_the_badge() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::widgets::{Block, Borders};

        let width = 40u16;
        let title = diff_list_title(3, false);
        let cols = badge_cols(0, width, title.chars().count() as u16).expect("40 幅なら出る");
        let label = revidere_badge_label(ArtifactState::Fresh, 0);

        let mut terminal = Terminal::new(TestBackend::new(width, 3)).unwrap();
        terminal
            .draw(|f| {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(title.clone())
                    .title_top(Line::from(label.clone()).alignment(Alignment::Right));
                f.render_widget(block, f.area());
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let top: String = (0..width).map(|x| buf[(x, 0)].symbol()).collect();
        let marker = crate::ui::common::revidere_marker(ArtifactState::Fresh, 0);
        let at = top
            .chars()
            .position(|c| c.to_string() == marker)
            .expect("チップが上枠に出ている");
        assert_eq!(at as u16, cols.start + 1, "枠内の位置: {top:?}");
        assert!(cols.contains(&(at as u16)));
        // 縦の境界のセル (右枠とその 1 つ外) は掴む余地を残すこと。
        assert!(cols.end < width);
    }

    /// 狭いパネルではチップを出さない。出さないものは押せないことが同じ
    /// 判定から導かれるので、見えないボタンは生まれない。
    #[test]
    fn a_narrow_panel_hides_the_badge() {
        let title_w = diff_list_title(0, false).chars().count() as u16;
        assert!(badge_cols(0, 20, title_w).is_none());
        assert!(badge_cols(0, 40, title_w).is_some());
    }

    /// 編集して git add し、さらに編集したファイルは WT_* と INDEX_* の
    /// 両方のビットを同時に持つ。この場合 staged ではなく unstaged（error）
    /// に解決されなければならない — 作業ツリーの編集の方がより新しく重要な
    /// 状態であり、"staged" と表示すると staged の上にさらに uncommitted な
    /// 変更があることが隠れてしまう。
    #[test]
    fn diff_file_status_color_staged_and_unstaged_resolves_to_unstaged() {
        let theme = Theme::default();
        let status = Some(git2::Status::INDEX_MODIFIED | git2::Status::WT_MODIFIED);
        assert_eq!(
            file_stage_state(status),
            FileStageState::Unstaged,
            "both staged and unstaged bits set must resolve to Unstaged"
        );
        assert_eq!(diff_file_status_color(&theme, status), theme.error);
    }
}
