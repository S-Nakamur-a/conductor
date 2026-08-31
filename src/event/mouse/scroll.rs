//! 全パネル（worktreeリスト、explorer、viewer、ターミナル、埋め込みエディタ、
//! reflowトランスクリプトビュー）のホイールスクロール処理。

use crate::app::{App, Focus};

/// マウスカーソル下のパネルをスクロールする。
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_mouse_scroll(
    app: &mut App,
    col: u16,
    row: u16,
    geom: &super::ClickGeometry,
    delta: i32,
) {
    // 境界はクリック処理と同じ ClickGeometry から取る — 別々に渡していた頃は
    // 片方だけ更新して当たり判定がズレる余地があった。
    let super::ClickGeometry {
        main_area,
        left_end,
        explorer_end,
        viewer_end,
        explorer_mid_y: _,
        terminal_split_y,
        ..
    } = *geom;
    if row < main_area.y || row >= main_area.y + main_area.height {
        return;
    }

    // エディタはExplorer+Viewerの合体領域を占有する。ホイールを内部プログラム用の
    // 矢印キーに変換する（オルタネートスクリーン上で動くため）。裏に隠れている
    // Explorer/Viewerの状態は絶対にスクロールしない。
    if app.editor.is_some() && col >= left_end && col < viewer_end {
        if let Some(idx) = app.editor.as_ref().map(|e| e.session_idx) {
            // PTYグリッドは1始まり。合体したエディタ領域は left_end（左枠）から始まり、
            // コンテンツはそこから1セル内側・1行下。
            let pty_col = col.saturating_sub(left_end).max(1);
            let pty_row = row.saturating_sub(main_area.y).max(1);
            app.terminal.pty_manager.forward_scroll_to_session(
                idx,
                delta.unsigned_abs() as usize,
                delta < 0,
                pty_col,
                pty_row,
            );
        }
        return;
    }

    if col < left_end {
        // Worktreeパネルのスクロール。
        let prev_wt = app.worktrees.selected_index();
        if delta > 0 {
            if !app.worktrees.rows.is_empty() {
                app.worktrees.row_selected = (app.worktrees.row_selected + 1)
                    .min(app.worktrees.rows.len().saturating_sub(1));
                app.sync_selected_worktree();
            }
        } else {
            app.worktrees.row_selected = app.worktrees.row_selected.saturating_sub(1);
            app.sync_selected_worktree();
        }
        if app.worktrees.selected_index() != prev_wt {
            app.on_worktree_changed();
        }
    } else if col < explorer_end {
        app.explorer_scroll(delta as isize, row);
    } else if col < viewer_end {
        // Viewerのスクロール。
        //
        // まずSUMMARY擬似ファイルを確認する。これは diff_mode がfalseのままパネル
        // 全体に描画され、current_file は裏で開かれていたファイルを指し続けている。
        // これがないとホイールがその隠れたファイルをスクロールしてしまい、サマリーは
        // 動かないままになる。
        if app.viewer.is_summary() {
            let total = app.viewer.summary_total_lines;
            if total > 0 {
                if delta > 0 {
                    app.viewer.summary_scroll = (app.viewer.summary_scroll
                        + delta.unsigned_abs() as usize)
                        .min(total.saturating_sub(1));
                } else {
                    app.viewer.summary_scroll = app
                        .viewer
                        .summary_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        } else if app.viewer.is_showing_rendered_markdown() {
            // レンダリング済みmarkdownは折り返し後の行数でスクロールするため、
            // file_scroll が指すソースの行数とは無関係。
            let total = app.viewer.md_total_lines;
            if total > 0 {
                if delta > 0 {
                    app.viewer.md_scroll = (app.viewer.md_scroll + delta.unsigned_abs() as usize)
                        .min(total.saturating_sub(1));
                } else {
                    app.viewer.md_scroll = app
                        .viewer
                        .md_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        } else if app.viewer.diff_view.diff_mode {
            // 統合差分ビューのスクロール。
            let total = app.viewer.diff_view.diff_view_lines.len();
            if total > 0 {
                if delta > 0 {
                    app.viewer.diff_view.diff_view_scroll = (app.viewer.diff_view.diff_view_scroll
                        + delta.unsigned_abs() as usize)
                        .min(total.saturating_sub(1));
                } else {
                    app.viewer.diff_view.diff_view_scroll = app
                        .viewer
                        .diff_view
                        .diff_view_scroll
                        .saturating_sub(delta.unsigned_abs() as usize);
                }
            }
        } else {
            // 折りたたんだ行を跨がないよう、可視行を歩いて動かす。生の加減算だと
            // 畳んだぶんだけ行き過ぎ、着地点が隠れていれば描画側が畳みを開いて
            // しまう（ホイールで畳みが勝手に開く）。
            app.viewer.move_cursor_lines(delta as isize);
        }
    } else {
        // ターミナルパネル（右カラム）。
        //
        // スクロール対象のパネルにフォーカスを移し、そのパネルが現在キーボード
        // フォーカスを持っていなくてもホイールイベントが即座に反映されるようにする。
        // これは terminal_claude.rs の focus == TerminalClaude という描画ガードも
        // 満たすので、reflowへの入場と表示の整合性が保たれる。
        // 補足: set_focus(TerminalShell) はreflowがアクティブなら閉じるが、これは
        // 意図的なもの — ユーザは意図的にClaudeから離れてスクロールしている。
        if row < terminal_split_y {
            if app.focus.current() != Focus::TerminalClaude {
                app.set_focus(Focus::TerminalClaude);
            }
        } else if app.focus.current() != Focus::TerminalShell {
            app.set_focus(Focus::TerminalShell);
        }

        let abs_delta = delta.unsigned_abs() as usize;
        // ScrollUp（delta < 0）は古いコンテンツ方向 / 履歴方向へ移動する。
        let up = delta < 0;
        let (session_idx, content_y) = if row < terminal_split_y {
            (app.terminal.claude.active_session, main_area.y + 1)
        } else {
            (app.terminal.shell.active_session, terminal_split_y + 1)
        };

        // 画面全体を占有するフルスクリーンアプリはホイールを自分で処理する:
        // マウスレポートが有効なアプリ（vim/neovim、less --mouse）にはエンコード
        // されたマウスイベントを渡し、マウスレポートなしのalt-screenページャーには
        // 矢印キーを渡す。いずれの場合もローカルのスクロールバックオフセットには
        // 触れない。PTYグリッドは1始まり。ターミナルの列は viewer_end（左枠）から
        // 始まり、パネルのコンテンツは content_y から始まる。
        let pty_col = col.saturating_sub(viewer_end).max(1);
        let pty_row = row.saturating_sub(content_y).saturating_add(1);
        if let Some(idx) = session_idx
            && app
                .terminal
                .pty_manager
                .forward_scroll_to_session(idx, abs_delta, up, pty_col, pty_row)
        {
            return;
        }

        if row < terminal_split_y {
            if app.reflow.active {
                // reflowビューがアクティブな間は、ホイールイベントをそのスクロール
                // オフセットへ流し込む。
                //
                // スクロールの規約: scroll=0が最も古い/一番上のコンテンツ、maxが
                // 最新/一番下。ホイールアップは古いコンテンツ方向へ移動する（減算）。
                //
                // 論理的な一番下を超えてホイールダウンすると退出スイープを開始する。
                // これによりトラックパッドの慣性でユーザが自然に最新のライブ末尾へ
                // 戻れる（ドキュメントの末尾を超えてスクロールするのと同じ体感）。
                // 一番下より上でのホイールアップ・ダウンは通常通りスクロール値を調整する。
                if up {
                    app.reflow.scroll = app.reflow.scroll.saturating_sub(abs_delta);
                } else {
                    let inner = app.reflow.last_inner_height as usize;
                    if crate::reflow::input::at_bottom(
                        app.reflow.scroll,
                        app.reflow.total_lines,
                        inner,
                    ) {
                        // 既に一番下 — さらに下スクロールすると退出スイープに入る。
                        app.close_reflow();
                        return;
                    }
                    app.reflow.scroll = app.reflow.scroll.saturating_add(abs_delta);
                }
                let inner = app.reflow.last_inner_height as usize;
                app.reflow.scroll = crate::reflow::input::clamp_scroll(
                    app.reflow.scroll,
                    app.reflow.total_lines,
                    inner,
                );
                // ホイールアップでビューは末尾から切り離され、ホイールダウンで最新行が
                // 画面に戻ると再び追従する。これがないと、ホイールアップの後にリサイズ
                // すると一番下に再固定されてスクロールが取り消されてしまう。
                app.reflow.follow = crate::reflow::input::at_bottom(
                    app.reflow.scroll,
                    app.reflow.total_lines,
                    inner,
                );
            } else if up {
                // 制限のあるvt100スクロールバックバッファではなく、ライブ末尾
                // （scroll_claude == 0）からの最初の上スクロールでreflowトランス
                // クリプトビューに入る。ホイールダウンは入場のトリガーにならない。
                // 意図しない上方向の慣性でもビューは開くが、ユーザはすぐEscで戻れる。
                //
                // worktreeがgrabされている間は入場をスキップする: 表示中のPTYは
                // メインworktreeのセッション上で動いているが、open_reflowはgrab元
                // （ソース）worktreeの履歴を参照してしまい、不整合が起きるため。
                // キーボードからの入場は既に handle_terminal_only_action の
                // grabbed-worktreeゲートでブロックされている。
                // open_reflow はパネルにピン留めされたセッションがない、またはログが
                // 見つからない場合は何もしない（ステータス表示のみ）ので、ホイールを
                // 永遠に飲み込むのではなくvt100バッファへフォールバックする。
                let opened =
                    if app.terminal.claude.scroll == 0 && !app.is_selected_worktree_grabbed() {
                        app.open_reflow();
                        app.reflow.active
                    } else {
                        false
                    };
                if !opened {
                    app.terminal.claude.scroll =
                        app.terminal.claude.scroll.saturating_add(abs_delta);
                }
            } else {
                app.terminal.claude.scroll = app.terminal.claude.scroll.saturating_sub(abs_delta);
            }
        } else if up {
            app.terminal.shell.scroll = app.terminal.shell.scroll.saturating_add(abs_delta);
        } else {
            app.terminal.shell.scroll = app.terminal.shell.scroll.saturating_sub(abs_delta);
        }
    }
}
