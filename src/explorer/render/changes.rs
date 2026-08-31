//! 変更ファイル一覧（Explorer 下半分、Changes ビュー）と、ファイルごとの
//! レビューコメント数バッジの描画。

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem};

use crate::diff_state::DiffListEntry;
use crate::explorer::ctx::{Ctx, Paint};
use crate::explorer::state::{Explorer, Pane};
use crate::icons::file_icon;
use crate::revidere::ArtifactState;
use crate::widget::list::Viewport;
use crate::widget::row::{Row, Segment};

/// 右上に出す revidere の状態チップの幅。
///
/// 状態が変わっても幅は変わらない。当たり判定はここから導いていて、描画された
/// 文字列を測っているわけではないので、状態ごとに幅が動くと押せる場所がずれる。
const REVIDERE_BADGE_W: u16 = 10;

/// 状態チップが占める画面の列。左のタイトルと重なるほど狭ければ None
/// (描画側もクリック側もこれで揃って諦めるので、見えないチップは押せない)。
///
/// 右寄せタイトルは右枠の 1 つ内側で終わる。
pub(crate) fn revidere_badge_cols(
    total: usize,
    has_error: bool,
    icon_set: crate::icons::IconSet,
    x: u16,
    width: u16,
) -> Option<Range<u16>> {
    let title = changes_title(total, has_error, icon_set);
    badge_cols(x, width, title.chars().count() as u16)
}

fn badge_cols(x: u16, width: u16, title_w: u16) -> Option<Range<u16>> {
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

/// None は GitStatusMap にエントリが無い、つまり HEAD に対してクリーン (= Committed)。
/// 編集 → add → さらに編集で WT_* と INDEX_* が両方立つので、unstaged を優先して先に見る。
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

/// 変更ファイル一覧（下部ペインの Changes ビュー）を描画する。
pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    view: Viewport,
    ex: &Explorer,
    ctx: &Ctx,
    paint: &Paint,
) {
    let theme = ctx.theme;
    let icon_set = ctx.config.ui.icon_set();
    let list_focused = ctx.focused && ex.focus() == Pane::Bottom;

    let total = ctx.diff.files.len();
    let has_error = ctx.diff.error.is_some();
    let title = changes_title(total, has_error, icon_set);

    let title_style = if list_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let mut block = crate::ui::common::PanelChrome::new(theme, title, ctx.focused, paint.border)
        .with_title_style(title_style)
        .into_block();

    // revidere の状態は消えない場所に出す。ステータス行のフラッシュだけだと、
    // 数分かかる解析が終わったのか、そもそも走っているのかが後から分からない。
    if revidere_badge_cols(total, has_error, icon_set, area.x, area.width).is_some() {
        let mut style = Style::default()
            .fg(crate::ui::common::revidere_color(theme, ctx.revidere))
            .add_modifier(Modifier::BOLD);
        // hover は前景の下線で示す。背景を敷くとテーマによっては枠線ごと
        // 潰れて、どこが押せるのか逆に読めなくなる。
        if paint.revidere_badge_hover {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        block = block.title_top(
            Line::from(Span::styled(
                revidere_badge_label(ctx.revidere, paint.tick),
                style,
            ))
            .alignment(Alignment::Right),
        );
    }

    // 以前は base 解決の失敗が完全に無音だった: 一覧が単に空で返ってきて
    // 「変更なし」に見えてしまっていた。メッセージを先頭行に固定して両者を
    // 混同しないようにする。このバナーは display_list の一部ではないため
    // 選択もできず、ナビゲーションキーが扱うインデックスもずらさない —
    // コストはリストの高さ1行分だけ。改行はスペースに潰す。複数行の
    // ListItem はここで確保した1行より多くの行を静かに消費してしまい、
    // List ウィジェットはパネル端で溢れた分を切り捨てるだけだから。
    let error_banner: Option<ListItem> = ctx.diff.error.as_deref().map(|msg| {
        ListItem::new(Span::styled(
            format!("  \u{26a0} {}", msg.replace('\n', " ")),
            Style::default().fg(theme.error),
        ))
    });
    let selected = ex.changes_cursor.selected();
    let range = ex.changes_cursor.visible(ctx.diff.display_list.len(), view);

    let entry_items = range.clone().filter_map(|idx| {
        let entry = ctx.diff.display_list.get(idx)?;
        let selected = idx == selected;
        let hover = paint.hover_changes.phase(idx);
        match entry {
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
                // ファイル行と違って segment を分ける必要がない。
                let prefix = format!("  {indent}{arrow} {} ", icon.glyph(icon_set));

                let line = Row::new(name.clone(), theme.info)
                    .lead([Segment::plain(prefix)])
                    .into_line(theme, selected, list_focused, hover);
                Some(ListItem::new(line))
            }
            DiffListEntry::File { file_index, depth } => {
                // インデックスアクセスではなく .get を使う: display_list と
                // ファイル vector は異なるティックで再構築されるため、片方が
                // 古いままレンダリングされるフレームがありうる。行をスキップ
                // すればチラつきで済むが、インデックスアクセスだと描画処理の
                // 内側からアプリ全体を落としかねない。上のファイルツリーも
                // 同様の対応をしている。
                let file_diff = ctx.diff.files.get(*file_index)?;
                let filename = file_diff.path.rsplit('/').next().unwrap_or(&file_diff.path);
                let indent = "  ".repeat(*depth);
                let icon = file_icon(filename);
                let prefix = format!("  {indent}");
                let glyph = format!("{} ", icon.glyph(icon_set));

                // ファイル名の色はファイルの git ステージ状態
                // （untracked / unstaged / staged / committed）を表す。
                // 行数はベースからの合計なので、その内訳がコミット済みか
                // 手元の編集かはこの色でしか分からない。
                let stage_state = file_stage_state(ex.tree.git_status.status(&file_diff.path));
                let base_fg = status_color(theme, stage_state);

                let icon_fg = Some(icon.role.color(theme));

                let mut trail = vec![
                    Segment::colored(format!(" +{}", file_diff.added_lines), theme.diff_add),
                    Segment::colored(format!(" -{}", file_diff.deleted_lines), theme.diff_del),
                ];
                if let Some((text, color)) = comment_badge(ctx, &file_diff.path) {
                    trail.push(Segment::colored(text, color));
                }
                if ex.viewed.contains(&file_diff.path) {
                    trail.push(Segment::colored("  \u{2713}", theme.success));
                }

                let line = Row::new(filename.to_string(), base_fg)
                    .lead([
                        Segment::plain(prefix),
                        Segment {
                            text: glyph.into(),
                            fg: icon_fg,
                            bold: false,
                        },
                    ])
                    .trail(trail)
                    .into_line(theme, selected, list_focused, hover);
                Some(ListItem::new(line))
            }
            DiffListEntry::Summary {} => {
                // 非選択時も常に太字にする。選択済みなら選択の強調が既に
                // 太字なので bold_name は効果を持たない。
                let line = Row::new("SUMMARY", theme.accent)
                    .bold_name()
                    .lead([Segment::plain("  \u{25A3} ")])
                    .into_line(theme, selected, list_focused, hover);
                Some(ListItem::new(line))
            }
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

/// diff error の接尾辞は、失敗して変更が欠けている場合と本当に (0) の場合を見分けるため。
/// "base error" としないのは、HEAD の解決失敗や merge-base 不在もここに入るから。
fn changes_title(total: usize, has_error: bool, icon_set: crate::icons::IconSet) -> String {
    let icon = crate::icons::PANEL_CHANGED.labeled(icon_set);
    if has_error {
        format!(" {icon}Changed files ({total}) — diff error ")
    } else {
        format!(" {icon}Changed files ({total}) ")
    }
}

/// changed-files 一覧の先頭でエラーバナーが占める行数。
///
/// [crate::explorer::keys::Panes::split] だけがこれを読む。描画と入力が別々に
/// 数えていた頃は 1 行のずれでクリックが別のファイルを開いていた。
pub(crate) fn changes_banner_rows(has_error: bool) -> usize {
    usize::from(has_error)
}

/// コメントが無ければ None。未解決があれば accent、全て解決済みなら muted。
fn comment_badge(ctx: &Ctx, file_path: &str) -> Option<(String, ratatui::style::Color)> {
    use crate::review_store::CommentStatus;
    let mut total = 0usize;
    let mut unresolved = 0usize;
    for c in ctx
        .review
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
        ctx.theme.accent
    } else {
        ctx.theme.muted
    };
    let icon_set = ctx.config.ui.icon_set();
    Some((
        format!("  {}{total}", crate::icons::COMMENT.get(icon_set)),
        color,
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
        assert_ne!(
            changes_title(0, true, crate::icons::IconSet::Unicode),
            changes_title(0, false, crate::icons::IconSet::Unicode)
        );
        assert_eq!(
            changes_title(0, false, crate::icons::IconSet::Unicode),
            " Changed files (0) "
        );
        assert!(changes_title(0, true, crate::icons::IconSet::Unicode).contains("error"));
    }

    /// base 解決が失敗しても HEAD 基準の一覧は生き残るので、件数は 0 以外に
    /// なり、かつエラーマーカーも表示される — 両方が出ていること。
    #[test]
    fn error_title_keeps_the_count() {
        let title = changes_title(17, true, crate::icons::IconSet::Unicode);
        assert!(title.contains("(17)"), "{title}");
        assert!(title.contains("error"), "{title}");
    }

    /// レンダラ、スクロールのページサイズ、マウスの行→インデックス変換は
    /// いずれもバナーの行コストをここから導出する。1行でもずれると
    /// クリックで別のファイルが開いてしまうので、契約として固定する。
    #[test]
    fn banner_costs_exactly_one_row_and_only_when_erroring() {
        assert_eq!(changes_banner_rows(false), 0);
        assert_eq!(changes_banner_rows(true), 1);
    }

    #[test]
    fn each_git_status_resolves_to_its_own_colour() {
        let theme = Theme::default();
        for (status, want) in [
            (Some(git2::Status::WT_NEW), theme.hint),
            (Some(git2::Status::WT_MODIFIED), theme.error),
            (Some(git2::Status::INDEX_MODIFIED), theme.warning),
            // 両方のビットが立つのは add したあとさらに編集した場合。staged と
            // 表示すると、その上に載っている未コミットの変更が隠れる。
            (
                Some(git2::Status::INDEX_MODIFIED | git2::Status::WT_MODIFIED),
                theme.error,
            ),
            // None は GitStatusMap にエントリが無い、つまり HEAD に対してクリーン。
            (None, theme.success),
        ] {
            assert_eq!(
                status_color(&theme, file_stage_state(status)),
                want,
                "{status:?}"
            );
        }
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
        let title = changes_title(3, false, crate::icons::IconSet::Unicode);
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
        let title_w = changes_title(0, false, crate::icons::IconSet::Unicode)
            .chars()
            .count() as u16;
        assert!(badge_cols(0, 20, title_w).is_none());
        assert!(badge_cols(0, 40, title_w).is_some());
    }
}
