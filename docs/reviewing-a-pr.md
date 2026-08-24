# Reviewing a Pull Request

PR review is built into the normal Explorer/Viewer/Terminal accordion — there's
no separate full-screen mode to enter or exit, so all the usual navigation and
terminal keybindings keep working while you review:

1. **Pull Request → local worktree** — menu: *Review ▸ Review Pull Request…*,
   enter a PR number or URL. Conductor fetches it (via `gh`) into a worktree,
   focuses the Explorer's changed-files list, and offers to run the AI review
   below on it.
2. **Changed files and comments** — the Explorer's bottom pane cycles between
   the changed-files diff list and the review comment list.
3. **Jump into the code** — the diff pane supports the Viewer's `gd`/`gi`/`gr`
   symbol-jump hints (go to definition / implementation / references). **Symbol
   jumps only work for Rust, Go, and TypeScript** — other languages show no
   hints.
4. **AI review** — `W` (menu: *Review ▸ Review current branch*) asks first, then
   runs **revidere** over the worktree's
   diff. It sorts every changed line into sections by importance
   (core / ripple / follow / minor) and checks that **no changed line is left
   unexplained**. `w` then opens the result as a full-screen two-column view:
   the reading order on the left, the diff in that order on the right. `j`/`k`
   scroll, `n`/`N` move between sections, `Enter` opens the section's location
   in the Viewer (where you can leave a comment), `q` closes it.

   revidere lives in this repository (`crates/revidere`) and ships inside the
   `conductor` binary, so there is nothing extra to install. It calls the AI
   through the same `[api]` section as every other AI feature here — but note
   that this one needs `provider = "command"` pointing at an agentic CLI: the
   model is expected to read the repository itself, which a plain HTTP
   completion cannot do. Replies are cached on the diff itself, so re-running on
   an unchanged diff returns instantly — the confirmation offers a re-analysis
   that ignores the cache when a review for this commit already exists, and
   `alt+w` skips both the question and the cache.
5. **Publish comments** — palette: *Review: Publish Comments to GitHub* posts
   your inline review comments (and replies) back to the PR via `gh`.

Requires the `gh` CLI (see Prerequisites) to be installed and authenticated.

> **Breaking change:** diff mode's "jump to top" moved from `g` to `gg` (vim-style),
> since `g` is now a prefix for symbol-jump hints (`gd`/`gi`/`gr`/`gg`).


