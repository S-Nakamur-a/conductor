//! Viewer列のクリック処理: シンボルジャンプ、コメントスレッド、左マージンの
//! ガター（コメントマーカー / 行番号 / テスト実行バッジ）、diff表示での
//! ExpandableContext行。

use crossterm::event::{KeyModifiers, MouseEvent};

use crate::app::{App, Focus, StatusLevel};

use super::super::explorer::open_viewer_comment;
use super::{ClickGeometry, resolve_screen_line};

/// address-conductor-comment スキル経由で、アクティブなClaude CodeのPTYにコメントを送る。
fn ask_claude_about_comment(app: &mut App, comment_id: &str) {
    let prompt = format!("/conductor:address-conductor-comment {comment_id}\n");

    // アクティブなClaude Codeセッションに書き込む。
    if let Some(idx) = app.terminal.active_claude_session {
        if app.terminal.pty_manager.is_waiting_for_input(idx) {
            let _ = app
                .terminal
                .pty_manager
                .write_chunked_to_session(idx, &prompt);
        } else {
            // 保留中のプロンプトとしてキューに積む。
            app.terminal.deferred_prompts.insert(idx, prompt);
        }
        app.set_focus(Focus::TerminalClaude);
        app.set_status(
            "Sent comment to Claude".to_string(),
            crate::app::StatusLevel::Info,
        );
    } else {
        app.set_status(
            "No active Claude Code session".to_string(),
            crate::app::StatusLevel::Warning,
        );
    }
}

/// クリックされた実行ボタンのテストコマンドをアクティブなShell PTYに送り、
/// そこにフォーカスする。コマンドは自動実行される（改行で終端）。言語に
/// 依存しない — コマンド（go test … や cargo test … など）はスキャナが
/// 組み立てる。
fn run_test(app: &mut App, run: &crate::test_run::TestRun) {
    let Some(idx) = app.terminal.active_shell_session else {
        app.set_status(
            "No shell session to run tests".to_string(),
            StatusLevel::Warning,
        );
        return;
    };
    let line = format!("{}\n", run.command);
    if let Err(e) = app
        .terminal
        .pty_manager
        .write_chunked_to_session(idx, &line)
    {
        log::warn!("failed to send test command to shell: {e}");
        app.set_status(
            "Failed to send test command to shell".to_string(),
            StatusLevel::Warning,
        );
        return;
    }
    // Shellターミナルを最新の末尾までスクロールし、コマンドが見えるようにする。
    app.terminal.scroll_shell = 0;
    app.set_focus(Focus::TerminalShell);
    app.set_status(format!("Running {}", run.label), StatusLevel::Info);
}

/// 画面上の行をThreadActions行として解決し、comment_idを返す。
fn resolve_screen_action(app: &App, screen_offset: usize) -> Option<String> {
    let map = &app.viewer_state.content.screen_row_map;
    match map.get(screen_offset) {
        Some(crate::viewer::ScreenRow::ThreadActions { comment_id }) => Some(comment_id.clone()),
        _ => None,
    }
}

/// Viewer列内の左クリックを処理する（シンボルジャンプ、コメントスレッド、ガター）。
pub(super) fn handle_viewer_column_click(
    app: &mut App,
    mouse: MouseEvent,
    col: u16,
    row: u16,
    geom: &ClickGeometry,
) {
    let main_area = geom.main_area;
    let explorer_end = geom.explorer_end;
    let viewer_end = geom.viewer_end;

    app.set_focus(Focus::Viewer);

    // レンダリング済みmarkdownには行番号がないので、以降の処理（シンボルジャンプ、
    // コメントスレッド、ガターのコメント/テスト実行ゾーン）はどれも解決する対象の
    // 行を持たない。クリックは単なるフォーカス変更で終わり、それ以上は何もしない。
    if app.viewer_state.is_showing_rendered_markdown() {
        return;
    }

    let inner_x = explorer_end + 1; // 左枠の内側
    let inner_y = main_area.y + 1; // 上枠の内側

    // タブ行は内側の先頭行。判定は描画が記録したクリック領域だけで行う —
    // タブ行を描かないモード（メディア/SUMMARY など）ではこれが空になるので、
    // その行のクリックを飲み込まずに通常の処理へ落ちる。
    if row == inner_y
        && let Some(action) = crate::ui::tab_bar::hit_at(&app.viewer_state.tab_row_hits, col)
    {
        match action {
            crate::ui::tab_bar::TabAction::Select(idx) => app.focus_viewer_tab(idx),
            crate::ui::tab_bar::TabAction::Close(idx) => app.close_viewer_tab(Some(idx)),
            _ => {}
        }
        return;
    }
    let marker_w = crate::viewer::COMMENT_MARKER_W;
    let gutter_w = app.viewer_state.gutter_total_width();
    let on_gutter = col >= inner_x && col < inner_x + marker_w + gutter_w;

    // Cmd+Click (macOS) / Ctrl+Click — クリックしたシンボルの定義へジャンプする。
    let has_jump_modifier = mouse.modifiers.contains(KeyModifiers::SUPER)
        || mouse.modifiers.contains(KeyModifiers::CONTROL);
    if has_jump_modifier && !on_gutter && !app.viewer_state.diff_view.diff_mode && row >= inner_y {
        let badge_w: u16 = 2;
        let content_start_x = inner_x + marker_w + gutter_w + badge_w;
        if col >= content_start_x {
            let screen_offset = (row - inner_y) as usize;
            if let Some(line_1) = resolve_screen_line(app, screen_offset) {
                let content_col =
                    (col - content_start_x) as usize + app.viewer_state.content.h_scroll;
                // インデックスではなく.getを使う: screen_row_mapは描画時にしか
                // 再構築されないので、ファイルウォッチャーの再読み込みと同じループの
                // イテレーションで処理されたクリックは「前のフレーム」のマップを
                // 参照して解決することになる。ファイルが縮んだ場合（Claude Codeによる
                // 書き換えやgit checkoutなど）、その行番号は既に末尾を超えており、
                // インデックスアクセスだとクリックの最中にアプリ全体を落としかねない。
                // ホバー側のパスも既に同じ方法でこれをガードしている。
                if let Some(line_text) = app.viewer_state.content.file_content.get(line_1 - 1)
                    && let Some((symbol, _, _)) = crate::app::masked_symbol_at_column(
                        line_text,
                        content_col,
                        line_1,
                        &app.viewer_state.content.code_mask,
                    )
                {
                    handle_symbol_click_jump(app, &symbol, screen_offset);
                }
            }
        }
        return;
    }

    // スレッドアクション行（返信 / 解決 / 削除 / 質問）へのクリックを処理する。
    // diff表示とファイル内容表示のどちらでも動く（両方ともscreen_row_mapを埋める）。
    if row >= inner_y {
        let screen_offset = (row - inner_y) as usize;
        if let Some(comment_id) = resolve_screen_action(app, screen_offset) {
            use crate::ui::viewer_panel::thread_actions;
            // 列オフセットからどのアクションがクリックされたかを判定する。レンダラが
            // その行を描画するのに使うのと同じレイアウト定数を使う。
            // レンダラとのオフセットの対応: left_pad は marker +
            // gutter_total_width() + 2（バッジ）で、そこに "  │ " の4列が続く。
            let content_x = inner_x + marker_w + gutter_w + 2 + 4;
            let click_col = col.saturating_sub(content_x) as usize;
            if click_col < thread_actions::reply_end() {
                // 返信: このコメントに対するインライン返信を開始する。
                // このコメントがどの行にあるかを探す（末尾の行）。
                if let Some(comment) = app
                    .review_state
                    .comments
                    .iter()
                    .find(|c| c.id == comment_id)
                {
                    let end_line = comment.line_end.unwrap_or(comment.line_start) as usize;
                    if !app
                        .viewer_state
                        .explorer
                        .expanded_inline_threads
                        .contains(&end_line)
                    {
                        app.viewer_state
                            .explorer
                            .expanded_inline_threads
                            .insert(end_line);
                    }
                    app.viewer_state.explorer.inline_reply_line = Some(end_line);
                    app.viewer_state.explorer.inline_reply_comment_id = Some(comment_id);
                    app.viewer_state.explorer.inline_reply_buffer.clear();
                }
            } else if click_col < thread_actions::resolve_end() {
                // 解決/未解決に戻す。
                if let Some(store) = app.review_store.as_ref() {
                    let new_status = if let Some(c) = app
                        .review_state
                        .comments
                        .iter()
                        .find(|c| c.id == comment_id)
                    {
                        match c.status {
                            crate::review_store::CommentStatus::Pending => {
                                crate::review_store::CommentStatus::Resolved
                            }
                            crate::review_store::CommentStatus::Resolved => {
                                crate::review_store::CommentStatus::Pending
                            }
                        }
                    } else {
                        return;
                    };
                    let _ = store.update_review_status(&comment_id, new_status);
                    let wt = app.selected_worktree_branch();
                    app.review_state.load_comments(store, &wt);
                    if let Some(file) = app.viewer_state.content.current_file.clone() {
                        app.review_state.build_file_comment_cache(&file);
                    }
                }
            } else {
                // クリックが右側の「ask claude」ボタン上かどうかを確認する。
                // 絶対列で判定する: 右端からその幅の範囲内かどうか。
                let ask_claude_w = thread_actions::ask_claude_width() as u16 + 2;
                if col + ask_claude_w >= viewer_end {
                    // Ask Claude: コメントをアクティブなClaudeのPTYに送る。
                    ask_claude_about_comment(app, &comment_id);
                } else {
                    // 削除（確認あり）。
                    app.request_delete_comment_by_id(comment_id);
                }
            }
            return;
        }
    }

    // ExpandableContext行をクリックすると展開する。インラインスレッドは画面上の
    // 行をずらすので、entry mapを介してその行を対応するdiffエントリへ逆引きする。
    // （これらの行は行番号を持たないので、下のマージンのディスパッチと衝突する
    // ことはない。）
    if app.viewer_state.diff_view.diff_mode && row >= inner_y {
        let screen_offset = (row - inner_y) as usize;
        if let Some(idx) = app
            .viewer_state
            .diff_view
            .screen_entry_map
            .get(screen_offset)
            .copied()
            .flatten()
            && matches!(
                app.viewer_state.diff_view.diff_view_lines.get(idx),
                Some(crate::viewer::UnifiedDiffEntry::ExpandableContext { .. })
            )
        {
            app.viewer_state.expand_context_at(idx, false);
        }
    }

    // 折りたたみマーカー。ガターの中にあるので、コメント作成へ落ちる前に
    // ここで捌く。当たり判定の列は in_fold_zone が持つ（ホバーの罫線と共有）。
    if !app.viewer_state.diff_view.diff_mode
        && row >= inner_y
        && super::in_fold_zone(col, inner_x + marker_w + gutter_w)
    {
        let screen_offset = (row - inner_y) as usize;
        if let Some(line_1) = resolve_screen_line(app, screen_offset)
            && app.viewer_state.content.folds.is_foldable(line_1)
        {
            app.viewer_state.fold_toggle_at(line_1);
            return;
        }
    }

    // 左マージンのディスパッチ。マージンは役割の異なる3つのゾーンからなる:
    //   - コメントマーカー列（一番左、💬/│） → 既存のインラインスレッドを
    //     トグルする。スレッドのフォーカスが存在するのはここだけ。
    //   - 行番号ガター → 既にコメント範囲に含まれる行（重なる/入れ子の範囲）で
    //     あっても、常に新しいコメントを開始する。
    //   - 2セル分のバッジ列 → ▶ はテストを実行し、それ以外は「+」が新しい
    //     コメントを開始する — コメント済みか否かに関わらず、どの行でも同じ。
    // コードの内容部分へのクリックは、単なるフォーカス変更として扱われる。
    let badge_w: u16 = 2;
    let on_marker = col >= inner_x && col < inner_x + marker_w;
    let gutter_start = inner_x + marker_w;
    let on_number_gutter = col >= gutter_start && col < gutter_start + gutter_w;
    let on_badge = col >= gutter_start + gutter_w && col < gutter_start + gutter_w + badge_w;
    if (on_marker || on_number_gutter || on_badge) && row >= inner_y {
        let screen_offset = (row - inner_y) as usize;
        // 画面行のマッピングはインラインスレッド行と両方の表示モードを扱う
        // （削除行は新しい行番号を持たないので、Noneに解決される）。
        if let Some(line_1) = resolve_screen_line(app, screen_offset) {
            // ファイルごとのコメントキャッシュが古い場合（例えば別のファイルが
            // カレントだった時にMCP経由でコメントが作られた場合など）に備えて
            // 防御的に更新し、バッジと下のディスパッチの認識を一致させる。
            if app.review_state.file_comments_path.as_deref()
                != app.viewer_state.content.current_file.as_deref()
                && let Some(f) = app.viewer_state.content.current_file.clone()
            {
                app.review_state.build_file_comment_cache(&f);
            }
            let zone = if on_marker {
                MarginZone::Marker
            } else if on_badge {
                MarginZone::Badge
            } else {
                MarginZone::NumberGutter
            };
            let has_comment = app.review_state.file_comments.contains_key(&line_1);
            // ▶ マーカーはファイル表示でのみ描画される — diff表示ではヒットテスト
            // しない。
            let has_test_run = !app.viewer_state.diff_view.diff_mode
                && app.viewer_state.content.test_runs.contains_key(&line_1);
            let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
            match classify_margin_click(zone, has_comment, has_test_run, shift) {
                MarginClickAction::ToggleThread => toggle_inline_thread_at(app, line_1),
                MarginClickAction::RunTest => {
                    if let Some(run) = app.viewer_state.content.test_runs.get(&line_1).cloned() {
                        run_test(app, &run);
                    }
                }
                MarginClickAction::StartComment { extend: true } => {
                    // Shift+クリックは直前にクリックした行から範囲を延長し、
                    // その場で作成欄を開く。
                    app.viewer_state.gutter_comment_click(line_1, true);
                    open_viewer_comment(app);
                }
                MarginClickAction::StartComment { extend: false } => {
                    // 通常の押下: ガターのドラッグを開始する。選択はこの1行から
                    // 始まり、カーソルがドラッグされるにつれて複数行に広がる。
                    // 作成欄はマウスアップ時に開く（GitHub風: クリック = 1行、
                    // ドラッグ = 範囲）。
                    app.viewer_state.gutter_comment_click(line_1, false);
                    app.viewer_state.click.gutter_drag_anchor = Some(line_1);
                }
            }
        }
    }
}

/// クリックがviewerの左マージンのどのゾーンに落ちたか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarginZone {
    /// 一番左のコメントマーカー列（💬 / │）、行番号より前。
    Marker,
    /// 行番号ガター。
    NumberGutter,
    /// ガターの右にある2セル分のバッジ列（▶ / ホバー時の「+」）。
    Badge,
}

/// viewerの左マージンへの左クリックが何をするか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarginClickAction {
    /// クリックした行の下に差し込まれたインラインコメントスレッドをトグルする。
    ToggleThread,
    /// その行のテストコマンドをShell PTYへ送る。
    RunTest,
    /// 新しいコメントを開始する（extend = shift+クリックによる範囲延長）。
    StartComment { extend: bool },
}

/// viewerの左マージンへの左クリックが何をするかを決定する。
///
/// スレッドのフォーカスはマーカー列にのみ存在する（その💬/│のグリフがスレッドの
/// 目印になる）。行番号ガターと「+」バッジ列は、既存のコメント範囲に含まれる
/// 行であっても常に新しいコメントを開始する — そのため、他のコメント範囲と
/// 重なる/入れ子になる範囲も作成可能なままであり、「+」のアフォーダンスは
/// どの行でも同じ挙動になる。▶ テスト実行ボタンはバッジ列に自分の場所を
/// 保つ。
pub(super) fn classify_margin_click(
    zone: MarginZone,
    has_comment: bool,
    has_test_run: bool,
    shift: bool,
) -> MarginClickAction {
    match zone {
        MarginZone::Marker if has_comment => MarginClickAction::ToggleThread,
        MarginZone::Badge if has_test_run => MarginClickAction::RunTest,
        _ => MarginClickAction::StartComment { extend: shift },
    }
}

/// line_1へのバッジクリックに対するインラインスレッドがどこに固定されるか。
///
/// スレッドはコメントの終了行（その💬がある場所）の下に差し込まれる —
/// diffレンダラはそれ以外の場所には描画しない — なので、範囲の途中の│行への
/// クリックは、スレッドを決して表示しない行を空振りでトグルするのではなく、
/// 最も近い、その行をカバーしている終了行にリダイレクトする。終了行自体の
/// 場合、最小値はその行自身になる。
pub(super) fn thread_anchor_line(
    comments: &[crate::review_store::ReviewComment],
    line_1: usize,
) -> usize {
    comments
        .iter()
        .map(|c| c.line_end.unwrap_or(c.line_start) as usize)
        .min()
        .unwrap_or(line_1)
}

/// line_1をカバーするコメントに対するインラインコメントスレッドをトグルする。
/// 初回展開時に返信を読み込み、折りたたむ時は進行中の返信をキャンセルする。
/// マウス（マーカー列のクリック）とキーボードのトグルの両方で共有される。
pub(in crate::event) fn toggle_inline_thread_at(app: &mut App, line_1: usize) {
    let line_1 = app
        .review_state
        .file_comments
        .get(&line_1)
        .map_or(line_1, |comments| thread_anchor_line(comments, line_1));
    let threads = &mut app.viewer_state.explorer.expanded_inline_threads;
    if threads.contains(&line_1) {
        threads.remove(&line_1);
        if app.viewer_state.explorer.inline_reply_line == Some(line_1) {
            app.viewer_state.explorer.inline_reply_line = None;
            app.viewer_state.explorer.inline_reply_comment_id = None;
            app.viewer_state.explorer.inline_reply_buffer.clear();
        }
    } else {
        threads.insert(line_1);
        if let Some(comments) = app.review_state.file_comments.get(&line_1) {
            for comment in comments {
                if !app.review_state.cached_replies.contains_key(&comment.id)
                    && let Some(store) = app.review_store.as_ref()
                    && let Ok(replies) = store.get_replies(&comment.id)
                {
                    app.review_state
                        .cached_replies
                        .insert(comment.id.clone(), replies);
                }
            }
        }
    }
}

/// viewer内のシンボルに対するCmd+Clickでの定義へのジャンプを処理する。
fn handle_symbol_click_jump(app: &mut App, symbol: &str, source_screen_row: usize) {
    if !app.code_nav.index.is_available() {
        app.set_status(
            "Symbol index not ready yet".to_string(),
            StatusLevel::Warning,
        );
        return;
    }

    let defs = app.code_nav.index.find_definitions(symbol);

    // 文脈に応じた動作: カーソルが定義箇所にある場合は代わりに参照を表示する。
    if app.is_cursor_at_definition(symbol) {
        // 既に定義箇所にいる — 参照を表示する。
        let root = app.code_nav.index.root();
        let refs = app.code_nav.index.find_references(symbol, &root);
        if refs.is_empty() {
            app.set_status(
                format!("No references found for '{symbol}'"),
                StatusLevel::Warning,
            );
        } else {
            app.code_nav.references.active = true;
            app.code_nav.references.symbol_name = symbol.to_string();
            app.code_nav.references.results = refs;
            app.code_nav.references.selected = 0;
            app.code_nav.references.scroll = 0;
        }
        return;
    }

    match defs.len() {
        0 => {
            app.set_status(
                format!("No definition found for '{symbol}'"),
                StatusLevel::Warning,
            );
        }
        1 => {
            let file = defs[0].file_path.clone();
            let line = defs[0].line;
            app.jump_to_location(&file, line, source_screen_row);
            app.set_status(
                format!("Jumped to definition of '{symbol}' (Ctrl+O to go back)"),
                StatusLevel::Success,
            );
        }
        n => {
            app.code_nav.references.active = true;
            app.code_nav.references.symbol_name = format!("{symbol} (definitions)");
            app.code_nav.references.results = defs
                .iter()
                .map(|d| crate::symbol_index::Reference {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    content: format!("{:?} {}", d.kind, d.name),
                })
                .collect();
            app.code_nav.references.selected = 0;
            app.code_nav.references.scroll = 0;
            app.set_status(
                format!("{n} definitions found for '{symbol}'"),
                StatusLevel::Info,
            );
        }
    }
}
