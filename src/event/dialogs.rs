//! Key handling for the two simple modal confirm dialogs that aren't part of
//! the `overlay` submodule tree: the self-update dialog and the
//! publish-review-to-GitHub confirmation.

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, UpdateState};

pub(super) fn handle_update_key(app: &mut App, key: KeyEvent) {
    match app.update.state {
        UpdateState::Confirming => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                app.start_update_download();
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.update.state = UpdateState::Idle;
            }
            _ => {}
        },
        UpdateState::InProgress => {
            if key.code == KeyCode::Esc {
                app.update.op.clear();
                app.update.state = UpdateState::Idle;
            }
        }
        UpdateState::Failed => {
            // Any key dismisses the error.
            app.update.state = UpdateState::Idle;
        }
        UpdateState::Restarting | UpdateState::Idle => {}
    }
}

pub(super) fn handle_publish_confirm_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            app.confirm_publish_review();
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            app.cancel_publish_review();
        }
        _ => {}
    }
}
