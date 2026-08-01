//! Embedded editor panel: spawning `$VISUAL`/`$EDITOR` in a PTY that occupies
//! the merged Explorer+Viewer region, and tearing it down on exit.

use std::path::PathBuf;

use super::focus::Focus;
use super::{App, StatusLevel};

/// State for an active embedded editor panel (vim/emacs in a PTY).
///
/// Transient: created when the user opens a file in `$EDITOR` and dropped when
/// the editor process exits. Owns the render cache so the editor panel renders
/// independently of the Claude/Shell terminal caches.
pub struct EditorPanel {
    /// Index of the editor's PTY session in the `PtyManager` session list.
    /// Kept in sync (shifted/cleared) when other sessions are removed.
    pub session_idx: usize,
    /// Absolute path of the file being edited — used for the reload-on-exit and
    /// the panel title.
    pub path: PathBuf,
    /// Cached PTY render output for the editor panel (mirrors the Claude/Shell
    /// caches in `TerminalState`).
    pub cache: crate::ui::common::PtyRenderCache,
    /// Set when the PTY reader thread produced new output to re-render.
    pub dirty: bool,
}

impl App {
    /// Open the file currently shown in the Viewer in an embedded editor panel
    /// (`$VISUAL` / `$EDITOR` in a PTY occupying the merged Explorer+Viewer
    /// region). Resolves the viewer's relative `current_file` against the
    /// selected worktree; if no file is open, flashes a hint instead. A no-op if
    /// an editor is already open.
    pub fn open_in_editor(&mut self) {
        if self.editor.is_some() {
            return;
        }
        // A grabbed worktree's terminals are locked (its sessions run on main),
        // and §1c would freeze an editor opened here. Refuse rather than trap the
        // user in an undrivable editor.
        if self.is_selected_worktree_grabbed() {
            self.set_status(
                "Cannot edit while this worktree is grabbed".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let Some(path) =
            editor_target(self.viewer_state.content.current_file.as_deref(), &working_dir)
        else {
            self.set_status("No file open to edit".to_string(), StatusLevel::Warning);
            return;
        };

        let argv = resolve_editor_command(
            std::env::var("VISUAL").ok().as_deref(),
            std::env::var("EDITOR").ok().as_deref(),
            "vi",
        );
        // `resolve_editor_command` never returns an empty vec.
        let (program, args) = argv.split_first().expect("editor command is non-empty");

        let (rows, cols) = self.editor_pty_size();
        match self.terminal.pty_manager.spawn_editor_session(
            &worktree_name,
            "editor",
            &working_dir,
            rows,
            cols,
            program,
            args,
            &path,
        ) {
            Ok(idx) => {
                self.terminal.pty_manager.activate_session(idx);
                let fname = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                self.editor = Some(EditorPanel {
                    session_idx: idx,
                    path,
                    cache: Default::default(),
                    dirty: true,
                });
                self.set_focus(Focus::Editor);
                // Repaint from scratch so the editor's alternate screen draws
                // cleanly over the panels it replaces.
                self.terminal.needs_clear = true;
                self.dirty.mark_all();
                self.set_status(
                    format!("Editing {fname} — Ctrl+Esc: Claude · :q: close · ctrl+alt+z: zoom"),
                    StatusLevel::Info,
                );
            }
            Err(e) => {
                self.set_status(format!("Failed to launch editor: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Tear down the embedded editor panel: kill/remove its PTY session, restore
    /// focus to the Viewer, and reload the just-edited file so the change is
    /// visible immediately (mirrors the debounced file-watcher refresh pair).
    pub fn exit_editor(&mut self) {
        let Some(path) = self.take_down_editor() else {
            return;
        };
        // Reload the just-edited file immediately (mirror the file-watcher pair).
        self.refresh_viewer();
        self.refresh_diff();
        self.dirty.mark_all();
        let fname = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.set_status(format!("Edited {fname}"), StatusLevel::Success);
    }

    /// Tear down the editor PTY and drop the panel, returning the edited path
    /// (or `None` if no editor was open). Shared core of [`Self::exit_editor`]
    /// (which adds reload + status) and worktree switching (which discards the
    /// editor silently because the surrounding context is being reloaded anyway).
    fn take_down_editor(&mut self) -> Option<PathBuf> {
        let panel = self.editor.take()?;
        // Remove the editor PTY (kill is harmless if the child already exited),
        // adjusting other session indices.
        self.close_terminal_session(panel.session_idx);
        // Move focus off the (now-gone) editor only if it was focused — the usual
        // `:q` flow. If the user had stepped over to Claude and the editor exited
        // from under them, leave their focus put; just drop any stale "editor
        // maximized" state. Assigned directly (not via `set_focus`) so callers
        // control any reload.
        if self.focus == Focus::Editor {
            self.focus = Focus::Viewer;
        }
        if self.expanded_panel == Some(Focus::Editor) {
            self.expanded_panel = None;
        }
        self.terminal.needs_clear = true;
        Some(panel.path)
    }

    /// Discard the editor panel when the worktree it belongs to is being left.
    /// No reload/flash — the caller ([`on_worktree_changed`]) reloads the new
    /// worktree's view regardless.
    pub fn discard_editor_on_worktree_change(&mut self) {
        self.take_down_editor();
    }

    /// If an embedded editor is open and its process has exited (e.g. `:q`),
    /// tear it down and restore the normal layout. Returns `true` if it closed.
    /// Called every main-loop iteration so the panel disappears promptly rather
    /// than waiting on the slow dead-session cleanup timer.
    pub fn poll_editor_exit(&mut self) -> bool {
        let Some(idx) = self.editor.as_ref().map(|e| e.session_idx) else {
            return false;
        };
        if self.terminal.pty_manager.is_session_alive(idx) {
            return false;
        }
        self.exit_editor();
        true
    }

    /// Compute the editor PTY's content size (rows, cols) from the cached
    /// layout: the editor occupies the merged Explorer+Viewer region, minus the
    /// title row and borders (which collapse when the panel is maximized).
    pub(super) fn editor_pty_size(&self) -> (u16, u16) {
        let cols = &self.layout.cache.columns;
        let region_w = cols[1].width.saturating_add(cols[2].width);
        let region_h = cols[1].height;
        let expanded = self.expanded_panel == Some(Focus::Editor);
        editor_content_size(region_w, region_h, expanded)
    }
}

/// Resolve the absolute path to hand an external editor from the viewer's
/// relative `current_file` and the worktree root. `None` (no file open, or an
/// empty path) means "nothing to edit" — the caller flashes a hint rather than
/// launching an editor on a bogus target.
fn editor_target(current_file: Option<&str>, worktree_root: &std::path::Path) -> Option<PathBuf> {
    let rel = current_file?;
    if rel.is_empty() {
        return None;
    }
    Some(worktree_root.join(rel))
}

/// Content size (rows, cols) for the embedded editor PTY given its region size
/// and whether it is maximized. The title row is always present; non-maximized
/// also has a bottom border row and left/right border columns. A zero region
/// (layout not computed yet) seeds a reasonable default — the per-frame resize
/// in `sync_pty_sizes` corrects it. Never returns 0 in either dimension (vt100
/// needs at least 1×1).
fn editor_content_size(region_w: u16, region_h: u16, expanded: bool) -> (u16, u16) {
    if region_w == 0 || region_h == 0 {
        return (24, 80);
    }
    let border_rows: u16 = if expanded { 1 } else { 2 };
    let border_cols: u16 = if expanded { 0 } else { 2 };
    (
        region_h.saturating_sub(border_rows).max(1),
        region_w.saturating_sub(border_cols).max(1),
    )
}

/// Resolve the editor command line from `$VISUAL` / `$EDITOR`, falling back to
/// `fallback`. Empty or whitespace-only values are ignored so a stray
/// `EDITOR=""` doesn't produce an empty command. The chosen value is split on
/// whitespace into program + arguments (so `"code -w"` works); an editor whose
/// *path* contains spaces is intentionally not supported (no shell-style
/// quoting — editor-flavor handling is out of scope).
fn resolve_editor_command(
    visual: Option<&str>,
    editor: Option<&str>,
    fallback: &str,
) -> Vec<String> {
    let chosen = [visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or(fallback);
    let parts: Vec<String> = chosen.split_whitespace().map(str::to_string).collect();
    if parts.is_empty() {
        vec![fallback.to_string()]
    } else {
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_target_resolves_relative_against_worktree() {
        let root = std::path::Path::new("/repo/wt");
        assert_eq!(
            editor_target(Some("src/main.rs"), root),
            Some(PathBuf::from("/repo/wt/src/main.rs"))
        );
    }

    #[test]
    fn editor_target_is_none_when_no_file_open() {
        // The load-bearing branch: no current file → no editor launch.
        assert_eq!(editor_target(None, std::path::Path::new("/repo/wt")), None);
    }

    #[test]
    fn editor_target_is_none_for_empty_path() {
        assert_eq!(editor_target(Some(""), std::path::Path::new("/repo/wt")), None);
    }

    #[test]
    fn resolve_editor_falls_back_when_unset() {
        assert_eq!(resolve_editor_command(None, None, "vi"), vec!["vi"]);
    }

    #[test]
    fn resolve_editor_visual_takes_precedence() {
        assert_eq!(
            resolve_editor_command(Some("vim"), Some("nano"), "vi"),
            vec!["vim"]
        );
    }

    #[test]
    fn resolve_editor_uses_editor_when_visual_unset() {
        assert_eq!(resolve_editor_command(None, Some("nano"), "vi"), vec!["nano"]);
    }

    #[test]
    fn resolve_editor_splits_args() {
        assert_eq!(
            resolve_editor_command(Some("code -w"), None, "vi"),
            vec!["code", "-w"]
        );
        assert_eq!(
            resolve_editor_command(Some("code\t-w  -n"), None, "vi"),
            vec!["code", "-w", "-n"]
        );
    }

    #[test]
    fn resolve_editor_ignores_blank_values() {
        // A blank/whitespace-only VISUAL is skipped so EDITOR (or the fallback)
        // still wins, rather than producing an empty command.
        assert_eq!(resolve_editor_command(Some(""), None, "vi"), vec!["vi"]);
        assert_eq!(resolve_editor_command(Some("   "), None, "vi"), vec!["vi"]);
        assert_eq!(
            resolve_editor_command(Some(""), Some("nano"), "vi"),
            vec!["nano"]
        );
        assert_eq!(resolve_editor_command(Some("  vim  "), None, "vi"), vec!["vim"]);
    }

    #[test]
    fn editor_content_size_subtracts_borders() {
        // Non-maximized: title row + bottom border (2 rows) and L/R borders (2 cols).
        assert_eq!(editor_content_size(80, 40, false), (38, 78));
        // Maximized: only the title row, no borders.
        assert_eq!(editor_content_size(80, 40, true), (39, 80));
    }

    #[test]
    fn editor_content_size_defaults_on_zero_region() {
        assert_eq!(editor_content_size(0, 40, false), (24, 80));
        assert_eq!(editor_content_size(80, 0, false), (24, 80));
    }

    #[test]
    fn editor_content_size_never_returns_zero() {
        // Tiny regions clamp to 1×1 rather than underflowing (vt100 needs ≥1).
        for w in 1..=3u16 {
            for h in 1..=3u16 {
                let (rows, c) = editor_content_size(w, h, false);
                assert!(rows >= 1 && c >= 1, "w={w} h={h} → ({rows},{c})");
            }
        }
    }

    #[test]
    fn resolve_editor_naive_split_does_not_honor_quotes() {
        // Documented limitation: no shell-style quoting. A quoted argument is
        // split on its inner spaces. This pins the intentional behavior.
        assert_eq!(
            resolve_editor_command(Some("vim -c 'set ft=rust'"), None, "vi"),
            vec!["vim", "-c", "'set", "ft=rust'"]
        );
    }
}
