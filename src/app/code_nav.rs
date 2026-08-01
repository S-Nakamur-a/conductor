//! Code navigation: symbol lookup under the cursor, jump-to-definition,
//! jump history, background symbol-index builds, and on-screen symbol hints.

use super::App;
use super::focus::Focus;

impl App {
    // ── Code navigation helpers ────────────────────────────────────

    /// Extract the symbol under the cursor from the current viewer line.
    ///
    /// Still the top visible line, not the actual text cursor — that
    /// mismatch is tracked separately (S6) and out of scope here.
    pub fn get_symbol_at_cursor(&self) -> Option<String> {
        let scroll = self.viewer_state.content.file_scroll;
        let line = self.viewer_state.content.file_content.get(scroll)?;
        let line_1 = scroll + 1;
        extract_symbol_from_line(line, line_1, &self.viewer_state.content.code_mask)
    }

    /// Explicitly show the hover-info popup for the symbol under the viewer
    /// cursor (the first identifier on the top line — same lookup as `gd`).
    ///
    /// Bound to `K` as an instant, no-wait trigger. Because it's a deliberate
    /// press it gives feedback when it can't produce a popup (status flash),
    /// unlike the passive auto-hover which stays silent.
    pub fn show_hover_info(&mut self) {
        use crate::app::StatusLevel;

        let symbol = match self.get_symbol_at_cursor() {
            Some(s) => s,
            None => {
                self.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
                return;
            }
        };
        if !self.code_nav.index.is_available() {
            self.set_status(
                "Symbol index not ready yet".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        let current_file = self.viewer_state.content.current_file.clone();
        match crate::hover_info::build_hover_info(
            &self.code_nav.index,
            &symbol,
            current_file.as_deref(),
        ) {
            Some(info) => {
                self.code_nav.hover_info.shown_file = current_file.clone();
                self.code_nav.hover_info.info = Some(info);
            }
            None => {
                self.set_status(
                    format!("No definition indexed for '{symbol}'"),
                    StatusLevel::Info,
                );
            }
        }
    }

    /// Whether the passive auto-hover popup is allowed to appear right now:
    /// a file (plain or diff) open in the focused Viewer, with no blocking
    /// overlay or the summary pseudo-view stealing the surface.
    fn hover_auto_allowed(&self) -> bool {
        self.focus == Focus::Viewer
            && !self.viewer_state.is_summary()
            && self.overlays.active == crate::overlay::ActiveOverlay::None
            && !self.code_nav.references.active
            && !self.code_nav.symbol_action.active
            && !self.code_nav.symbol_hint.active
            && self.viewer_state.content.current_file.is_some()
    }

    /// Clear the whole hover modal stack (popup, pending candidate, refs list,
    /// preview, pin). Returns whether anything was actually showing.
    pub fn clear_hover(&mut self) -> bool {
        let had = self.code_nav.hover_info.info.is_some()
            || self.code_nav.hover_info.pending.is_some()
            || self.code_nav.hover_info.pinned;
        self.code_nav.hover_info.reset();
        had
    }

    /// Clear every piece of mouse-hover state at once (D7): the jump
    /// underline, the popup stack, and the Explorer's row-hover highlights.
    /// crossterm never reports the mouse leaving the terminal window, so this
    /// is called from the few events that *do* reliably mean "the mouse isn't
    /// resting on anything drawn right now" — any key press, `FocusLost`, and
    /// a blocking overlay opening (see call sites in `event_loop.rs` and
    /// `event/mouse/mod.rs`).
    pub fn clear_all_hover(&mut self) {
        self.clear_pointer_hover();
        // The popup stack is only safe to drop when it is *not* pinned: a
        // pinned modal is keyboard-driven and, by long-standing convention,
        // survives focus and idle loss (see `HoverInfoOverlay::pinned` and
        // `tick_hover`'s early return).
        if !self.code_nav.hover_info.pinned {
            self.clear_hover();
        }
    }

    /// Clear only the *pointer-driven* highlights: the jump underline and the
    /// row / chip / tab hovers.
    ///
    /// Deliberately does **not** touch the hover popup stack. `handle_key_event`
    /// already resolves the popup per keystroke with semantics this function
    /// cannot reproduce — a pinned modal consumes the key to drive itself, and
    /// a transient popup is dismissed while Esc specifically is swallowed so it
    /// doesn't also trigger the focused panel's Esc action. Clearing the stack
    /// here — which an earlier revision did, ahead of `handle_key_event` —
    /// reset `pinned` to false before that check ever ran, making the modal's
    /// entire keyboard path unreachable and letting Esc fire twice.
    ///
    /// Everything this *does* clear has no other escape hatch on a key press:
    /// crossterm never reports the pointer leaving the window, so without this
    /// a highlight would stay lit after the user switches to the keyboard.
    pub fn clear_pointer_hover(&mut self) {
        self.viewer_state.click.hover_symbol = None;
        self.viewer_state.click.underline_pending = None;
        self.list_hover.clear();
        // S7: bar/tab-bar hover (background-based, D1 revised).
        self.wtbar.hover = None;
        self.terminal.claude_tab_hover = None;
        self.terminal.shell_tab_hover = None;
    }

    /// Record the symbol the mouse is currently resting on (from a mouse-move
    /// event). `cand` is `(symbol, 1-indexed line, anchor_row, anchor_col,
    /// start_col, end_col)` — the anchor is in absolute screen coords, the
    /// cols are 0-indexed content columns (before h_scroll), carried through
    /// to `HoverInfoOverlay::target_*` once resolved (A8). `None` when the
    /// mouse is over blank space / a non-identifier. A *new* symbol restarts
    /// the idle countdown and drops any popup shown for the previous one.
    pub fn set_mouse_hover_candidate(
        &mut self,
        cand: Option<(String, usize, u16, u16, usize, usize)>,
    ) {
        match cand {
            Some((symbol, line, anchor_row, anchor_col, start_col, end_col)) => {
                let same = self
                    .code_nav.hover_info
                    .pending
                    .as_ref()
                    .is_some_and(|c| c.symbol == symbol && c.line == line);
                if same {
                    return;
                }
                let file = self.viewer_state.content.current_file.clone();
                self.code_nav.hover_info.pending = Some(crate::overlay::HoverCandidate {
                    symbol,
                    line,
                    file,
                    anchor_row,
                    anchor_col,
                    start_col,
                    end_col,
                    since: std::time::Instant::now(),
                    resolved: false,
                });
                self.code_nav.hover_info.leave_at = None;
                if self.code_nav.hover_info.info.take().is_some() {
                    self.dirty.mark_all();
                }
            }
            None => {
                // Mouse moved off the symbol onto blank space. If a popup is
                // showing, don't drop it instantly — start a short grace window
                // (see `tick_hover`) so the cursor can travel onto the popup to
                // click it. If nothing is shown yet, just drop the candidate.
                if self.code_nav.hover_info.info.is_some() {
                    self.code_nav.hover_info.pending = None;
                    if self.code_nav.hover_info.leave_at.is_none() {
                        self.code_nav.hover_info.leave_at = Some(std::time::Instant::now());
                    }
                } else if self.code_nav.hover_info.pending.take().is_some() {
                    self.dirty.mark_all();
                }
            }
        }
    }

    /// Record the symbol the mouse is resting on for the jump-underline (D8),
    /// separately from [`set_mouse_hover_candidate`]'s popup debounce. `cand`
    /// is `(symbol, 1-indexed line, start_col, end_col)`, or `None` off any
    /// symbol. `has_jump_modifier` is Cmd/Ctrl's state as of this move —
    /// stored on the resolved [`crate::viewer::HoverSymbol`] to drive the
    /// underline's color (A6/A7), and refreshed live even while resting on an
    /// already-resolved symbol so holding/releasing the modifier updates the
    /// color without re-running the debounce.
    ///
    /// D9: unlike the popup, there's no leave grace — moving off the symbol
    /// (or onto a different one) clears any shown underline instantly.
    pub fn set_underline_candidate(
        &mut self,
        cand: Option<(String, usize, usize, usize)>,
        has_jump_modifier: bool,
    ) {
        match cand {
            Some((symbol, line, start_col, end_col)) => {
                if let Some(hs) = self.viewer_state.click.hover_symbol.as_mut()
                    && hs.line == line
                    && hs.start_col == start_col
                    && hs.end_col == end_col
                {
                    hs.has_jump_modifier = has_jump_modifier;
                    return;
                }
                let same_pending = self
                    .viewer_state
                    .click
                    .underline_pending
                    .as_ref()
                    .is_some_and(|p| {
                        p.line == line && p.start_col == start_col && p.end_col == end_col
                    });
                if same_pending {
                    if let Some(p) = self.viewer_state.click.underline_pending.as_mut() {
                        p.has_jump_modifier = has_jump_modifier;
                    }
                    return;
                }
                self.viewer_state.click.underline_pending = Some(crate::viewer::PendingUnderline {
                    symbol,
                    line,
                    start_col,
                    end_col,
                    since: std::time::Instant::now(),
                    resolved: false,
                    has_jump_modifier,
                });
                // No grace (D9): a new rested-on candidate immediately hides
                // whatever underline was shown for the previous one.
                self.viewer_state.click.hover_symbol = None;
            }
            None => {
                self.viewer_state.click.underline_pending = None;
                self.viewer_state.click.hover_symbol = None;
            }
        }
    }

    /// True when the given absolute screen point lies within any part of the
    /// hover modal stack (base popup, refs list, or preview) — used to keep the
    /// popup alive while the mouse is over it and to route clicks.
    pub fn hover_point_hit(&self, col: u16, row: u16) -> bool {
        let hv = &self.code_nav.hover_info;
        let in_rect = |r: ratatui::layout::Rect| {
            r.width > 0
                && r.height > 0
                && col >= r.x
                && col < r.x + r.width
                && row >= r.y
                && row < r.y + r.height
        };
        if in_rect(hv.info_rect) {
            return true;
        }
        if let Some(refs) = &hv.refs {
            if in_rect(refs.rect) {
                return true;
            }
            if let Some(p) = &refs.preview
                && in_rect(p.rect)
            {
                return true;
            }
        }
        false
    }

    /// Per-frame auto-hover driver. When the mouse has rested on a symbol past
    /// the debounce, resolves its hover popup; stays silent when nothing is
    /// found. Also manages the grace window and stale-file/focus invalidation.
    pub fn tick_hover(&mut self) {
        /// How long the mouse must rest on a symbol before the popup appears.
        const HOVER_IDLE: std::time::Duration = std::time::Duration::from_millis(350);
        /// Grace window keeping a transient popup alive after the mouse leaves
        /// the symbol, so the cursor can reach the popup to click it.
        const HOVER_GRACE: std::time::Duration = std::time::Duration::from_millis(700);

        // A pinned modal is user-driven: it survives focus/idle loss and is only
        // dismissed by Esc or a click outside (handled in the event layer).
        if self.code_nav.hover_info.pinned {
            return;
        }

        // Stale-file guard: if the viewer switched files (via a jump, the file
        // tree, or an external reload) while a popup was up, the popup now
        // describes a symbol from a file no longer on screen. Drop it — even
        // within the grace window — so it can never linger over unrelated code.
        if self.code_nav.hover_info.info.is_some()
            && self.code_nav.hover_info.shown_file != self.viewer_state.content.current_file
        {
            if self.clear_hover() {
                self.dirty.mark_all();
            }
            return;
        }

        // Grace window: a popup whose symbol the mouse left stays up briefly, but
        // only while the mouse is actually over it or the timer hasn't expired.
        if let Some(left) = self.code_nav.hover_info.leave_at
            && left.elapsed() >= HOVER_GRACE
        {
            if self.clear_hover() {
                self.dirty.mark_all();
            }
            return;
        }

        if !self.hover_auto_allowed() {
            // Don't kill a popup that's within its grace window — the user may be
            // moving the mouse toward it (which briefly leaves the content area).
            if self.code_nav.hover_info.leave_at.is_some() {
                return;
            }
            if self.clear_hover() {
                self.dirty.mark_all();
            }
            return;
        }

        // Auto-hover is driven purely by the mouse resting on a symbol (set by the
        // mouse-move handler). A top-line/keyboard heuristic was tried and dropped:
        // with no per-line text cursor, "the cursor line" is always the top visible
        // line, so it fired for code the user wasn't pointing at. Mouse position is
        // exact — on a symbol shows the popup, on whitespace shows nothing.

        // Resolve a candidate that has rested long enough.
        let ready = self
            .code_nav.hover_info
            .pending
            .as_ref()
            .is_some_and(|c| !c.resolved && c.since.elapsed() >= HOVER_IDLE);
        if ready {
            let (symbol, file, anchor_row, anchor_col, start_col, end_col, line) = {
                let c = self.code_nav.hover_info.pending.as_ref().unwrap();
                (
                    c.symbol.clone(),
                    c.file.clone(),
                    c.anchor_row,
                    c.anchor_col,
                    c.start_col,
                    c.end_col,
                    c.line,
                )
            };
            let info =
                crate::hover_info::build_hover_info(&self.code_nav.index, &symbol, file.as_deref());
            if let Some(c) = self.code_nav.hover_info.pending.as_mut() {
                c.resolved = true;
            }
            self.code_nav.hover_info.anchor_row = anchor_row;
            self.code_nav.hover_info.anchor_col = anchor_col;
            // Remember which viewed file this popup describes, so the stale-file
            // guard can drop it the moment the viewer moves to another file.
            self.code_nav.hover_info.shown_file = if info.is_some() { file } else { None };
            // A8: keep the described symbol highlighted for as long as `info` is
            // shown, independent of `ClickTracker::hover_symbol` (which the mouse
            // may since have moved off, or which has no leave-grace at all — D9).
            if info.is_some() {
                self.code_nav.hover_info.target_line = line;
                self.code_nav.hover_info.target_start_col = start_col;
                self.code_nav.hover_info.target_end_col = end_col;
            }
            self.code_nav.hover_info.info = info;
            self.dirty.mark_all();
        }
    }

    /// Per-frame jump-underline driver (D8/D9): once the mouse has rested on
    /// a symbol past its own, faster debounce, resolves whether it's
    /// jumpable and shows/hides the underline accordingly (A7 — no underline
    /// for a non-jumpable word).
    pub fn tick_underline_hover(&mut self) {
        // D9: 150ms (`underline_debounce_ready`'s threshold) — long enough
        // that a mouse merely passing over code on its way elsewhere doesn't
        // paint-and-unpaint every symbol it crosses (0ms was tried and
        // produces a "Christmas tree" flicker), short enough to stay clearly
        // faster than — and independent of — the popup's 350ms `HOVER_IDLE`
        // in `tick_hover` above, since the underline is meant to read as
        // instantaneous compared to the popup's deliberate pause.
        let ready = self
            .viewer_state
            .click
            .underline_pending
            .as_ref()
            .is_some_and(|p| underline_debounce_ready(p.since.elapsed(), p.resolved));
        if !ready {
            return;
        }

        let (symbol, line, start_col, end_col, has_jump_modifier) = {
            let p = self.viewer_state.click.underline_pending.as_ref().unwrap();
            (
                p.symbol.clone(),
                p.line,
                p.start_col,
                p.end_col,
                p.has_jump_modifier,
            )
        };
        let jumpable = self.can_jump_to_symbol(&symbol);
        if let Some(p) = self.viewer_state.click.underline_pending.as_mut() {
            p.resolved = true;
        }
        self.viewer_state.click.hover_symbol = jumpable.then_some(crate::viewer::HoverSymbol {
            text: symbol,
            line,
            start_col,
            end_col,
            has_jump_modifier,
        });
        self.dirty.mark_all();
    }

    /// Cancel the grace window because the mouse is now over the popup itself.
    pub fn hover_keep_alive(&mut self) {
        self.code_nav.hover_info.leave_at = None;
    }

    /// Open the references list (level 1) for the currently-shown symbol and pin
    /// the popup. No-op when nothing is shown or the symbol has no references.
    pub fn open_hover_refs(&mut self) {
        let symbol = match self.code_nav.hover_info.info.as_ref() {
            Some(info) if info.ref_count > 0 => info.symbol_name.clone(),
            _ => return,
        };
        let root = self.code_nav.index.root();
        let results = self.code_nav.index.find_references(&symbol, &root);
        if results.is_empty() {
            return;
        }
        self.code_nav.hover_info.pinned = true;
        self.code_nav.hover_info.leave_at = None;
        self.code_nav.hover_info.refs = Some(crate::overlay::HoverRefs {
            symbol,
            results,
            selected: 0,
            scroll: 0,
            rect: ratatui::layout::Rect::default(),
            row_hits: Vec::new(),
            preview: None,
        });
        self.dirty.mark_all();
    }

    /// Open the code preview (level 2) for reference row `idx` in the list.
    pub fn open_hover_preview(&mut self, idx: usize) {
        let (file, line) = match self.code_nav.hover_info.refs.as_mut() {
            Some(refs) => match refs.results.get(idx) {
                Some(r) => {
                    refs.selected = idx;
                    (r.file_path.clone(), r.line)
                }
                None => return,
            },
            None => return,
        };
        let root = self.code_nav.index.root();
        let preview = build_hover_preview(&root, &file, line);
        if let Some(refs) = self.code_nav.hover_info.refs.as_mut() {
            refs.preview = preview;
        }
        self.dirty.mark_all();
    }

    /// Jump to the open preview's location and dismiss the whole hover stack.
    pub fn hover_jump_to_preview(&mut self) {
        let target = self
            .code_nav.hover_info
            .refs
            .as_ref()
            .and_then(|r| r.preview.as_ref())
            .map(|p| (p.file.clone(), p.center_line));
        if let Some((file, line)) = target {
            self.clear_hover();
            self.jump_to_location(&file, line, 0);
        }
    }

    /// Move the references-list selection by `delta` (keyboard nav), clamping.
    pub fn hover_refs_move(&mut self, delta: isize) {
        if let Some(refs) = self.code_nav.hover_info.refs.as_mut() {
            let n = refs.results.len();
            if n == 0 {
                return;
            }
            let cur = refs.selected as isize;
            refs.selected = (cur + delta).clamp(0, n as isize - 1) as usize;
            self.dirty.mark_all();
        }
    }

    /// Esc from the hover stack: close the deepest open level (preview → list →
    /// the whole popup). Returns whether a level was closed.
    pub fn hover_pop_level(&mut self) -> bool {
        if let Some(refs) = self.code_nav.hover_info.refs.as_mut() {
            if refs.preview.take().is_some() {
                self.dirty.mark_all();
                return true;
            }
            self.code_nav.hover_info.refs = None;
            self.code_nav.hover_info.pinned = false;
            self.dirty.mark_all();
            return true;
        }
        if self.clear_hover() {
            self.dirty.mark_all();
            return true;
        }
        false
    }

    /// Check if the cursor is currently at (or very near) a definition site
    /// for the given symbol. Returns `true` when the current file + line
    /// matches one of the symbol's definition locations.
    pub fn is_cursor_at_definition(&self, symbol: &str) -> bool {
        let cur_file = match &self.viewer_state.content.current_file {
            Some(f) => f,
            None => return false,
        };
        // Cursor line is 1-indexed (file_scroll is 0-indexed).
        let cursor_line = self.viewer_state.content.file_scroll + 1;
        let defs = self.code_nav.index.find_definitions(symbol);
        defs.iter().any(|d| {
            d.file_path == *cur_file && (d.line as isize - cursor_line as isize).unsigned_abs() <= 2
        })
    }

    /// Jump to a file location, pushing the current position onto the history.
    ///
    /// `source_screen_row` is the screen row (0-indexed) where the source
    /// symbol was displayed. The target line will be placed at the same row
    /// so the user's eye position is preserved.
    pub fn jump_to_location(&mut self, file_path: &str, line: usize, source_screen_row: usize) {
        // Skip self-referencing jumps (destination == current position).
        let target_line_0 = line.saturating_sub(1);
        if let Some(ref cur_file) = self.viewer_state.content.current_file {
            let current_line_0 = self.viewer_state.content.file_scroll + source_screen_row;
            if cur_file == file_path && current_line_0 == target_line_0 {
                return;
            }
        }

        // Save current location to history.
        if let Some(ref cur_file) = self.viewer_state.content.current_file.clone() {
            let loc = crate::jump_history::Location {
                file_path: cur_file.clone(),
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            };
            self.code_nav.history.push(loc);
        }

        // Open the target file.
        if let Some(wt) = self.worktrees.selected() {
            let wt_path = wt.path.clone();
            let tab_width = self.config.viewer.tab_width;
            self.viewer_state.open_file(&wt_path, file_path, tab_width);
            self.rehighlight_viewer();
            self.viewer_state.reveal_file_in_tree(file_path, &wt_path);
        }

        // Scroll so the target line appears at the same screen row as the source symbol.
        let target_0 = line.saturating_sub(1);
        let total = self.viewer_state.content.file_content.len();
        let scroll = target_0
            .saturating_sub(source_screen_row)
            .min(total.saturating_sub(1));
        self.viewer_state.content.file_scroll = scroll;
        self.viewer_state.content.h_scroll = 0;
        self.viewer_state.show_raw_for_line_target();
        self.set_focus(Focus::Viewer);
    }

    /// Navigate back in the jump history.
    pub fn jump_back(&mut self) {
        let current = match self.viewer_state.content.current_file.clone() {
            Some(f) => crate::jump_history::Location {
                file_path: f,
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            },
            None => return,
        };

        if let Some(loc) = self.code_nav.history.go_back(current) {
            if let Some(wt) = self.worktrees.selected() {
                let wt_path = wt.path.clone();
                let tab_width = self.config.viewer.tab_width;
                self.viewer_state
                    .open_file(&wt_path, &loc.file_path, tab_width);
                self.rehighlight_viewer();
                self.viewer_state
                    .reveal_file_in_tree(&loc.file_path, &wt_path);
            }
            let total = self.viewer_state.content.file_content.len();
            self.viewer_state.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer_state.content.h_scroll = loc.h_scroll;
            self.viewer_state.show_raw_for_line_target();
        }
    }

    /// Navigate forward in the jump history.
    pub fn jump_forward(&mut self) {
        let current = match self.viewer_state.content.current_file.clone() {
            Some(f) => crate::jump_history::Location {
                file_path: f,
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            },
            None => return,
        };

        if let Some(loc) = self.code_nav.history.go_forward(current) {
            if let Some(wt) = self.worktrees.selected() {
                let wt_path = wt.path.clone();
                let tab_width = self.config.viewer.tab_width;
                self.viewer_state
                    .open_file(&wt_path, &loc.file_path, tab_width);
                self.rehighlight_viewer();
                self.viewer_state
                    .reveal_file_in_tree(&loc.file_path, &wt_path);
            }
            let total = self.viewer_state.content.file_content.len();
            self.viewer_state.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer_state.content.h_scroll = loc.h_scroll;
            self.viewer_state.show_raw_for_line_target();
        }
    }

    /// Start building the symbol index in the background, over whichever
    /// worktree is currently selected.
    ///
    /// Re-aiming the index here rather than at each call site is what keeps the
    /// two from drifting: the index must describe the tree the viewer is
    /// showing, and every path that wants a build — startup, a worktree
    /// switch, a filesystem change — wants it for that same tree.
    /// [`SymbolIndex::set_root`] is a no-op when the root has not moved, so the
    /// filesystem-change path still just rebuilds in place.
    ///
    /// A build already running is left to finish rather than being replaced.
    /// Worktree selection changes arrive as fast as the user can scroll a list,
    /// and each one reaches here; without this, dragging through ten worktrees
    /// starts ten concurrent full-tree parses (`BackgroundOp` cannot cancel the
    /// one it replaces — it drops the join handle and the worker runs to
    /// completion regardless). The index stays unavailable for as long as the
    /// pile takes to drain, so navigation dies exactly while the user is moving
    /// around. Superseded builds discard their own results via the generation
    /// check, and the settled worktree gets its build from the caller below.
    pub fn start_symbol_index_build(&mut self) {
        self.code_nav.index.set_root(self.selected_worktree_path());
        if self.bg.symbol_index.is_running() {
            return;
        }
        let index = self.code_nav.index.clone();
        self.bg.symbol_index.start(move |tx| {
            let result = match index.build() {
                Ok(count) => Ok(count),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });
    }

    /// Check whether a symbol has definitions in the symbol index.
    pub fn can_jump_to_symbol(&self, name: &str) -> bool {
        if !self.code_nav.index.is_available() {
            return false;
        }
        !self.code_nav.index.find_definitions(name).is_empty()
    }

    /// Build symbol hints for visible lines in the viewer.
    /// Returns hints with 2-character labels for jumpable symbols on screen.
    pub fn build_symbol_hints(&self, inner_height: usize) -> Vec<crate::overlay::SymbolHint> {
        let scroll = self.viewer_state.content.file_scroll;
        let total = self.viewer_state.content.file_content.len();
        let end = (scroll + inner_height).min(total);

        let mask = &self.viewer_state.content.code_mask;
        let mut seen = std::collections::HashSet::new();
        let mut candidates = Vec::new();

        for line_idx in scroll..end {
            let line = &self.viewer_state.content.file_content[line_idx];
            let line_1 = line_idx + 1;
            // Enumerated with the same scan that built the mask — the mask is
            // keyed by position in this sequence, so it must be this one.
            for (k, (start, stop, word)) in
                crate::symbol_index::identifier_occurrences(line).enumerate()
            {
                if !mask.is_code(line_1, k) {
                    continue;
                }
                if word.len() <= 1 || is_rust_keyword(word) {
                    continue;
                }
                if !seen.insert(word.to_string()) {
                    continue;
                }
                if !self.can_jump_to_symbol(word) {
                    continue;
                }
                candidates.push((word.to_string(), line_1, start, stop));
            }
        }

        // Assign 2-character labels: aa, ab, ..., az, ba, bb, ...
        candidates
            .into_iter()
            .enumerate()
            .map(|(i, (name, line, start, end))| {
                let first = (b'a' + (i / 26) as u8) as char;
                let second = (b'a' + (i % 26) as u8) as char;
                crate::overlay::SymbolHint {
                    label: format!("{first}{second}"),
                    symbol_name: name,
                    line,
                    start_col: start,
                    end_col: end,
                }
            })
            .collect()
    }
}

/// Build a code preview window around `line_1` (1-indexed) in `rel_path`,
/// reading a few lines of context on each side. Returns `None` if the file
/// can't be read or the line is out of range.
fn build_hover_preview(
    root: &std::path::Path,
    rel_path: &str,
    line_1: usize,
) -> Option<crate::overlay::HoverPreview> {
    /// Lines of context shown on each side of the reference line.
    const CONTEXT: usize = 3;

    let source = std::fs::read_to_string(root.join(rel_path)).ok()?;
    let all: Vec<&str> = source.lines().collect();
    if line_1 == 0 || line_1 > all.len() {
        return None;
    }
    let idx = line_1 - 1;
    let start = idx.saturating_sub(CONTEXT);
    let end = (idx + CONTEXT + 1).min(all.len());
    let lines = (start..end)
        .map(|i| (i + 1, all[i].to_string()))
        .collect::<Vec<_>>();
    Some(crate::overlay::HoverPreview {
        file: rel_path.to_string(),
        center_line: line_1,
        lines,
        rect: ratatui::layout::Rect::default(),
    })
}

// ── Jump-underline decision helpers (pure, unit-tested directly) ──────

/// Which of the two underline colors to draw, or `None` to draw nothing.
///
/// The underline is now shown on any rest over a symbol, not just while
/// Cmd/Ctrl is held — its color is what still communicates the modifier
/// state: `Hint` reads as "there's a definition here", `Accent` as "press
/// now to jump" (the click itself still requires the modifier — this only
/// changes which promise the underline makes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineColorKind {
    Hint,
    Accent,
}

/// Decide the underline's color for a rested-on symbol (A7: non-jumpable
/// words — keywords, unresolved identifiers — get no underline at all,
/// matching the popup's silence for the same words).
pub fn underline_color_kind(
    is_jumpable: bool,
    has_jump_modifier: bool,
) -> Option<UnderlineColorKind> {
    if !is_jumpable {
        return None;
    }
    Some(if has_jump_modifier {
        UnderlineColorKind::Accent
    } else {
        UnderlineColorKind::Hint
    })
}

/// Whether the hover-info popup's target symbol (A8) covers `query_line`, and
/// if so, its highlight range. Returns `None` while the popup is hidden or
/// describing a different line — the renderer then falls back to whatever
/// `ClickTracker::hover_symbol` (the underline) says for that line instead.
pub fn popup_highlight_range(
    popup_shown: bool,
    target_line: usize,
    target_start_col: usize,
    target_end_col: usize,
    query_line: usize,
) -> Option<(usize, usize)> {
    if popup_shown && target_line == query_line {
        Some((target_start_col, target_end_col))
    } else {
        None
    }
}

/// D9's debounce check, factored out of `tick_underline_hover` so it can be
/// unit-tested without constructing an `App`: ready once `elapsed` has
/// crossed 150ms and the candidate hasn't already been resolved once.
fn underline_debounce_ready(elapsed: std::time::Duration, resolved: bool) -> bool {
    const HOVER_UNDERLINE_MS: u64 = 150;
    !resolved && elapsed >= std::time::Duration::from_millis(HOVER_UNDERLINE_MS)
}

// ── Free functions for symbol extraction ──────────────────────────────

/// Extract a symbol name from a source code line at the cursor position.
/// Returns the first identifier on `line_1` (1-indexed) that [`mask`] marks
/// as code — not the first identifier-shaped word. A word inside a comment
/// or string literal (e.g. `Building` in `//! Building …`, or `index` in
/// `let x = 1; // build the index`) is skipped the same way a comment-only
/// line used to be, but per-occurrence rather than per-line, so a trailing
/// comment on an otherwise real code line no longer hides the code before it.
///
/// Was a standalone prefix check (`//`, `/*`, `*`, `#`) before S2; that only
/// caught comments starting the line and had no way to see a mid-line
/// comment or a string literal, and duplicated what the mask now decides in
/// one place. Follows the same enumerate-and-gate shape as
/// [`App::build_symbol_hints`](crate::app::App::build_symbol_hints).
pub fn extract_symbol_from_line(
    line: &str,
    line_1: usize,
    mask: &crate::symbol_index::CodeMask,
) -> Option<String> {
    for (k, (_, _, word)) in crate::symbol_index::identifier_occurrences(line).enumerate() {
        if !mask.is_code(line_1, k) {
            continue;
        }
        if word.len() > 1 && !is_rust_keyword(word) {
            return Some(word.to_string());
        }
    }
    None
}

/// Check if a word is a Rust keyword (should not be treated as a symbol).
pub fn is_rust_keyword(word: &str) -> bool {
    matches!(
        word,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
    )
}

/// Extract the symbol (identifier) at a specific column in a line.
/// Returns `(symbol_text, start_col, end_col)` where cols are 0-indexed character offsets.
pub fn extract_symbol_at_column(line: &str, col: usize) -> Option<(String, usize, usize)> {
    if col >= line.len() {
        return None;
    }
    // Check that the character at `col` is part of an identifier.
    let ch = line.as_bytes().get(col).copied()?;
    if !(ch.is_ascii_alphanumeric() || ch == b'_') {
        return None;
    }
    // Walk backwards to find start of identifier.
    let start = line[..col]
        .bytes()
        .rev()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let start_col = col - start;
    // Walk forwards to find end of identifier.
    let end = line[col..]
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let end_col = col + end;
    let word = &line[start_col..end_col];
    if word.len() <= 1 || is_rust_keyword(word) {
        return None;
    }
    // Must start with letter or underscore.
    if !word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    Some((word.to_string(), start_col, end_col))
}

/// [`extract_symbol_at_column`], gated on `mask` so a word sitting in a
/// comment or string literal never resolves to a jump target.
///
/// Kept separate from the extraction itself, which stays a pure "what
/// identifier is at this column" lookup — it has no way to know whether that
/// occurrence is code or prose. Every call site that turns a mouse column
/// into a jumpable symbol (hover underline, auto-hover popup, Cmd+Click) goes
/// through this instead of calling `extract_symbol_at_column` directly.
pub fn masked_symbol_at_column(
    line: &str,
    col: usize,
    line_1: usize,
    mask: &crate::symbol_index::CodeMask,
) -> Option<(String, usize, usize)> {
    let (symbol, start_col, end_col) = extract_symbol_at_column(line, col)?;
    if !mask.is_code_at_column(line, line_1, col) {
        return None;
    }
    Some((symbol, start_col, end_col))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_symbol_at_column_basic() {
        let line = "    let foo = AppState::new();";
        // Click on 'A' of AppState at col 14
        let result = extract_symbol_at_column(line, 14);
        assert_eq!(result, Some(("AppState".to_string(), 14, 22)));
    }

    #[test]
    fn test_extract_symbol_at_column_middle() {
        let line = "    let foo = AppState::new();";
        // Click on 'S' of AppState at col 17
        let result = extract_symbol_at_column(line, 17);
        assert_eq!(result, Some(("AppState".to_string(), 14, 22)));
    }

    #[test]
    fn test_extract_symbol_at_column_on_keyword() {
        let line = "    let foo = bar;";
        // Click on 'l' of let at col 4
        let result = extract_symbol_at_column(line, 4);
        assert_eq!(result, None); // "let" is a keyword
    }

    #[test]
    fn test_extract_symbol_at_column_on_space() {
        let line = "fn main() {}";
        let result = extract_symbol_at_column(line, 2);
        assert_eq!(result, None); // space
    }

    #[test]
    fn test_extract_symbol_at_column_out_of_bounds() {
        let line = "short";
        let result = extract_symbol_at_column(line, 100);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_symbol_at_column_single_char() {
        let line = "x + y";
        // Single char identifiers are filtered out
        let result = extract_symbol_at_column(line, 0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_symbol_at_column_underscore_prefix() {
        let line = "    _handler.call();";
        let result = extract_symbol_at_column(line, 5);
        assert_eq!(result, Some(("_handler".to_string(), 4, 12)));
    }

    // ── S2: masked_symbol_at_column ─────────────────────────────────────

    #[test]
    fn masked_symbol_at_column_skips_trailing_line_comment() {
        // `extract_symbol_at_column` alone would happily return "build" —
        // it has no notion of comments. The mask is what makes hover /
        // Cmd+Click refuse to treat prose as a jump target.
        let src = "fn f() {\n    let x = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        let col = line.find("build").unwrap();
        assert_eq!(masked_symbol_at_column(line, col, 2, &mask), None);
    }

    #[test]
    fn masked_symbol_at_column_skips_string_literal() {
        let src = "fn f() {\n    let s = \"index\";\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        let col = line.find("index").unwrap();
        assert_eq!(masked_symbol_at_column(line, col, 2, &mask), None);
    }

    #[test]
    fn masked_symbol_at_column_allows_real_code() {
        // Same line shape as the comment case above, but pointing at the
        // identifier in code position — the mask must not over-mask.
        let src = "fn f() {\n    let value = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        let col = line.find("value").unwrap();
        assert_eq!(
            masked_symbol_at_column(line, col, 2, &mask),
            Some(("value".to_string(), col, col + 5))
        );
    }

    #[test]
    fn build_hover_preview_windows_around_line() {
        let dir = std::env::temp_dir().join(format!("hover_prev_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = (1..=10)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("f.rs"), src).unwrap();

        // Center line 5 → 3 lines of context each side (2..=8).
        let p = build_hover_preview(&dir, "f.rs", 5).expect("preview");
        assert_eq!(p.center_line, 5);
        assert_eq!(p.file, "f.rs");
        assert_eq!(
            p.lines,
            vec![
                (2, "line2".to_string()),
                (3, "line3".to_string()),
                (4, "line4".to_string()),
                (5, "line5".to_string()),
                (6, "line6".to_string()),
                (7, "line7".to_string()),
                (8, "line8".to_string()),
            ]
        );

        // Near the top the window clamps to the file start.
        let p = build_hover_preview(&dir, "f.rs", 1).expect("preview");
        assert_eq!(p.lines.first().unwrap().0, 1);

        // Out-of-range / missing file → None.
        assert!(build_hover_preview(&dir, "f.rs", 999).is_none());
        assert!(build_hover_preview(&dir, "nope.rs", 1).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_symbol_from_line_skips_comments_via_mask() {
        // Doc/line/block comments must not yield an English word that happens
        // to collide with a real type name (the "Building" bug) — now decided
        // by the mask (S1) rather than a line-prefix guess, so it needs real
        // multi-line source for the block comment to parse as one.
        let src = "\
//! Building and navigating
/// Create a new state
// Building the list
/* Building */
fn f() {
/* Building
 * Building (block cont.)
 */
}
#[derive(Debug)]
struct Marker;
fn g() {
    let state = DiffState::new();
}
pub struct Building {
    x: i32,
}
";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = |n: usize| src.lines().nth(n - 1).unwrap();

        assert_eq!(extract_symbol_from_line(line(1), 1, &mask), None);
        assert_eq!(extract_symbol_from_line(line(2), 2, &mask), None);
        assert_eq!(extract_symbol_from_line(line(3), 3, &mask), None);
        assert_eq!(extract_symbol_from_line(line(4), 4, &mask), None);
        // Continuation line of the multi-line block comment above.
        assert_eq!(extract_symbol_from_line(line(7), 7, &mask), None);

        // `#[derive(Debug)]` is no longer specially excluded: an attribute is
        // real syntax, not prose, and the mask only masks comments/strings
        // (D2) — so `derive` is in code position and comes back like any
        // other identifier. The old prefix check treated every `#`-led line
        // as unresolvable; the mask draws the line at "is this a comment or
        // a string", which an attribute is neither.
        assert_eq!(
            extract_symbol_from_line(line(10), 10, &mask),
            Some("derive".to_string())
        );

        // Real code lines still resolve to their first identifier.
        assert_eq!(
            extract_symbol_from_line(line(13), 13, &mask),
            Some("state".to_string())
        );
        assert_eq!(
            extract_symbol_from_line(line(15), 15, &mask),
            Some("Building".to_string())
        );
    }

    #[test]
    fn extract_symbol_from_line_skips_trailing_comment_and_string_hits() {
        // The bug this replaces the prefix check for: a real statement
        // followed by a trailing comment. `x` is a single character (dropped
        // on its own) and `build`/`the`/`index` sit inside the comment, so
        // the whole line now resolves to nothing — not "build", which the
        // old implementation returned because it only looked at how the line
        // *started*, never at what came after `//` mid-line.
        let src = "fn f() {\n    let x = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(extract_symbol_from_line(line, 2, &mask), None);

        // Same shape, but with a real identifier before the comment: it must
        // still resolve, proving the fix isn't over-masking the whole line.
        let src = "fn f() {\n    let value = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(
            extract_symbol_from_line(line, 2, &mask),
            Some("value".to_string())
        );

        // A string literal hides its contents the same way.
        let src = "fn f() {\n    let s = \"index\";\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(extract_symbol_from_line(line, 2, &mask), None);
    }

    // ── D8/D9/A7/A8: jump-underline decision functions ──────────────────

    #[test]
    fn viewer_hover_symbol_color_none_when_not_jumpable() {
        // A7: a non-jumpable word never gets an underline, modifier or not.
        assert_eq!(underline_color_kind(false, false), None);
        assert_eq!(underline_color_kind(false, true), None);
    }

    #[test]
    fn viewer_hover_symbol_color_hint_without_modifier() {
        // D8: shown on any rest now (no modifier needed) — hint-colored to
        // read as informational rather than actionable.
        assert_eq!(
            underline_color_kind(true, false),
            Some(UnderlineColorKind::Hint)
        );
    }

    #[test]
    fn viewer_hover_symbol_color_accent_with_modifier() {
        // D8: Cmd/Ctrl held promotes the same underline to "press now to jump".
        assert_eq!(
            underline_color_kind(true, true),
            Some(UnderlineColorKind::Accent)
        );
    }

    #[test]
    fn viewer_hover_symbol_popup_range_matches_target_line() {
        // A8: the popup's own target line/cols are returned regardless of
        // where the mouse currently is.
        assert_eq!(popup_highlight_range(true, 42, 4, 10, 42), Some((4, 10)));
    }

    #[test]
    fn viewer_hover_symbol_popup_range_none_off_target_line() {
        assert_eq!(popup_highlight_range(true, 42, 4, 10, 43), None);
    }

    #[test]
    fn viewer_hover_symbol_popup_range_none_when_hidden() {
        assert_eq!(popup_highlight_range(false, 42, 4, 10, 42), None);
    }

    #[test]
    fn viewer_hover_symbol_debounce_not_ready_before_150ms() {
        assert!(!underline_debounce_ready(
            std::time::Duration::from_millis(149),
            false
        ));
    }

    #[test]
    fn viewer_hover_symbol_debounce_ready_at_150ms() {
        assert!(underline_debounce_ready(
            std::time::Duration::from_millis(150),
            false
        ));
    }

    #[test]
    fn viewer_hover_symbol_debounce_not_ready_once_resolved() {
        // Already-resolved candidates don't get re-resolved every tick while
        // the mouse sits still on the same symbol.
        assert!(!underline_debounce_ready(
            std::time::Duration::from_millis(500),
            true
        ));
    }
}
