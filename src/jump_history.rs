//! Jump history stack for code navigation (back/forward).
//!
//! Tracks file locations so the user can jump back and forward
//! between definition sites, similar to IDE "go back" / "go forward".

/// A saved location in the codebase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// Relative file path from the worktree root.
    pub file_path: String,
    /// 0-indexed line number (scroll position).
    pub line: usize,
    /// Horizontal scroll offset.
    pub h_scroll: usize,
}

/// Maximum number of entries in the history stack.
const MAX_HISTORY: usize = 200;

/// A back/forward navigation history.
pub struct JumpHistory {
    /// Stack of past locations (most recent at the end).
    back: Vec<Location>,
    /// Stack of forward locations (populated when going back).
    forward: Vec<Location>,
}

impl JumpHistory {
    /// Create a new empty history.
    pub fn new() -> Self {
        Self {
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    /// Push a location onto the back stack.
    /// Clears the forward stack (new navigation branch).
    pub fn push(&mut self, location: Location) {
        self.forward.clear();
        self.back.push(location);
        if self.back.len() > MAX_HISTORY {
            self.back.remove(0);
        }
    }

    /// Go back to the previous location.
    /// Pushes `current` onto the forward stack and returns the previous location.
    pub fn go_back(&mut self, current: Location) -> Option<Location> {
        let prev = self.back.pop()?;
        self.forward.push(current);
        Some(prev)
    }

    /// Go forward to the next location.
    /// Pushes `current` onto the back stack and returns the next location.
    pub fn go_forward(&mut self, current: Location) -> Option<Location> {
        let next = self.forward.pop()?;
        self.back.push(current);
        Some(next)
    }

    /// Whether there are entries to go back to.
    #[allow(dead_code)]
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether there are entries to go forward to.
    #[allow(dead_code)]
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

impl Default for JumpHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(file: &str, line: usize) -> Location {
        Location {
            file_path: file.to_string(),
            line,
            h_scroll: 0,
        }
    }

    #[test]
    fn test_push_and_go_back() {
        let mut h = JumpHistory::new();
        h.push(loc("a.rs", 10));
        h.push(loc("b.rs", 20));

        let prev = h.go_back(loc("c.rs", 30));
        assert_eq!(prev, Some(loc("b.rs", 20)));

        let prev = h.go_back(loc("b.rs", 20));
        assert_eq!(prev, Some(loc("a.rs", 10)));

        // No more history.
        assert!(h.go_back(loc("a.rs", 10)).is_none());
    }

    #[test]
    fn test_go_forward() {
        let mut h = JumpHistory::new();
        h.push(loc("a.rs", 10));
        h.push(loc("b.rs", 20));

        // Go back.
        let prev = h.go_back(loc("c.rs", 30)).unwrap();
        assert_eq!(prev, loc("b.rs", 20));

        // Go forward.
        let next = h.go_forward(loc("b.rs", 20)).unwrap();
        assert_eq!(next, loc("c.rs", 30));
    }

    #[test]
    fn test_push_clears_forward() {
        let mut h = JumpHistory::new();
        h.push(loc("a.rs", 10));
        h.push(loc("b.rs", 20));

        h.go_back(loc("c.rs", 30));
        assert!(h.can_go_forward());

        // New push should clear forward.
        h.push(loc("d.rs", 40));
        assert!(!h.can_go_forward());
    }

    #[test]
    fn test_max_history() {
        let mut h = JumpHistory::new();
        for i in 0..250 {
            h.push(loc("file.rs", i));
        }
        // Should be capped at MAX_HISTORY.
        assert_eq!(h.back.len(), 200);
    }
}
