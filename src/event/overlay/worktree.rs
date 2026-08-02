//! オーバーレイ: worktree の作成・削除入力（名前入力、ベースブランチピッカー、
//! スマート説明モード、各種 y/n 確認プロンプト）。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, StatusLevel};

use crate::event::clipboard_paste;

pub(in crate::event) fn handle_worktree_input_key(app: &mut App, key: KeyEvent) {
    use crate::app::WorktreeInputMode;

    match app.worktree_mgr.input_mode {
        WorktreeInputMode::CreatingWorktree => match key.code {
            KeyCode::Esc => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.worktree_mgr.input_buffer.clear();
                app.status_message = None;
            }
            KeyCode::Tab => {
                // スマートモードに切り替える。
                let text = app.worktree_mgr.input_buffer.text().to_string();
                app.worktree_mgr.input_buffer.clear();
                app.worktree_mgr.smart_description_buffer.set_text(&text);
                app.worktree_mgr.input_mode = WorktreeInputMode::SmartDescription;
                app.set_status(
                    "Describe your task (Shift+Enter: newline, Enter: generate, Tab: manual mode, Esc: cancel)".to_string(),
                    StatusLevel::Info,
                );
            }
            KeyCode::Enter => {
                let name = app.worktree_mgr.input_buffer.text().to_string();
                if name.is_empty() {
                    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                    app.worktree_mgr.input_buffer.clear();
                    app.set_status("Cancelled (empty name).".to_string(), StatusLevel::Warning);
                } else {
                    // ステップ2（ベースブランチピッカー）に進む。
                    app.worktree_mgr.pending_branch = name;
                    app.worktree_mgr.input_buffer.clear();
                    app.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktreeBase;
                    app.load_base_branches();
                    app.status_message = None;
                }
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                app.worktree_mgr.input_buffer.delete_to_line_start();
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                clipboard_paste(app, |a| &mut a.worktree_mgr.input_buffer, false);
            }
            _ => {
                app.worktree_mgr.input_buffer.handle_key(key);
            }
        },
        WorktreeInputMode::CreatingWorktreeBase => {
            let filtered = app.filtered_base_branches();
            let count = filtered.len();

            match key.code {
                KeyCode::Esc => {
                    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                    app.worktree_mgr.base_branch_filter.clear();
                    app.worktree_mgr.pending_branch.clear();
                    app.set_status("Cancelled.".to_string(), StatusLevel::Warning);
                }
                KeyCode::Down => {
                    if count > 0 && app.worktree_mgr.base_branch_selected + 1 < count {
                        app.worktree_mgr.base_branch_selected += 1;
                    }
                }
                KeyCode::Up => {
                    if app.worktree_mgr.base_branch_selected > 0 {
                        app.worktree_mgr.base_branch_selected -= 1;
                    }
                }
                KeyCode::Enter => {
                    let filtered = app.filtered_base_branches();
                    let base_ref = if let Some(&(original_idx, _)) =
                        filtered.get(app.worktree_mgr.base_branch_selected)
                    {
                        app.worktree_mgr
                            .base_branch_list
                            .get(original_idx)
                            .cloned()
                            .unwrap_or_default()
                    } else if !app.worktree_mgr.base_branch_filter.is_empty() {
                        // マッチなし — フィルタのテキストをそのまま ref として使う。
                        app.worktree_mgr.base_branch_filter.text().to_string()
                    } else {
                        String::new() // origin/main にデフォルトフォールバックする
                    };
                    let branch_name = app.worktree_mgr.pending_branch.clone();
                    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                    app.worktree_mgr.base_branch_filter.clear();
                    app.worktree_mgr.pending_branch.clear();
                    app.create_worktree_from_base(&branch_name, &base_ref);
                }
                KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                    app.worktree_mgr.base_branch_filter.delete_to_line_start();
                    app.worktree_mgr.base_branch_selected = 0;
                }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    clipboard_paste(app, |a| &mut a.worktree_mgr.base_branch_filter, false);
                    app.worktree_mgr.base_branch_selected = 0;
                }
                _ => {
                    if app.worktree_mgr.base_branch_filter.handle_key(key) {
                        // テキストが変わった — フィルタキーによる選択をリセットする。
                        match key.code {
                            KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                                app.worktree_mgr.base_branch_selected = 0;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        WorktreeInputMode::ConfirmingDelete => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                // ブランチ削除は完了ハンドラ側で行う (delete_branch_after = true)。
                app.delete_selected_worktree(true);
            }
            _ => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.set_status("Deletion cancelled.".to_string(), StatusLevel::Warning);
            }
        },
        WorktreeInputMode::ConfirmingUngrab => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.execute_ungrab();
            }
            _ => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.set_status("Ungrab cancelled.".to_string(), StatusLevel::Warning);
            }
        },
        WorktreeInputMode::ConfirmingReset => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.perform_reset_main_to_origin();
            }
            _ => {
                app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                app.set_status("Reset cancelled.".to_string(), StatusLevel::Warning);
            }
        },
        WorktreeInputMode::SmartDescription => {
            // Shift+Enter で改行を挿入する（複数行編集）。
            if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
                app.worktree_mgr.smart_description_buffer.insert_char('\n');
                return;
            }
            match key.code {
                KeyCode::Esc => {
                    app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                    app.worktree_mgr.smart_description_buffer.clear();
                    app.status_message = None;
                }
                KeyCode::Tab => {
                    // 手動モードに戻す。
                    let text = app.worktree_mgr.smart_description_buffer.text().to_string();
                    app.worktree_mgr.smart_description_buffer.clear();
                    app.worktree_mgr.input_buffer.set_text(&text);
                    app.worktree_mgr.input_mode = WorktreeInputMode::CreatingWorktree;
                    app.set_status(
                        "New branch name (Tab: Smart Mode, Enter to continue, Esc to cancel):"
                            .to_string(),
                        StatusLevel::Info,
                    );
                }
                KeyCode::Enter => {
                    let desc = app.worktree_mgr.smart_description_buffer.trim().to_string();
                    if desc.is_empty() {
                        app.set_status("Description is empty.".to_string(), StatusLevel::Warning);
                    } else {
                        app.start_smart_worktree_async(&desc);
                        app.worktree_mgr.input_mode = WorktreeInputMode::Normal;
                        app.worktree_mgr.smart_description_buffer.clear();
                    }
                }
                KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                    app.worktree_mgr
                        .smart_description_buffer
                        .delete_to_line_start();
                }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    clipboard_paste(app, |a| &mut a.worktree_mgr.smart_description_buffer, true);
                }
                _ => {
                    app.worktree_mgr.smart_description_buffer.handle_key(key);
                }
            }
        }
        WorktreeInputMode::Normal => unreachable!(),
    }
}
