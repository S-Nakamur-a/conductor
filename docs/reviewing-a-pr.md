# Reviewing a Pull Request

PR review is built into the normal Explorer/Viewer/Terminal accordion — there's
no separate full-screen mode to enter or exit, so all the usual navigation and
terminal keybindings keep working while you review:

1. **Pull Request → local worktree** — menu: *Review ▸ Review Pull Request…*,
   enter a PR number or URL. Conductor asks `gh` for the PR's branch and checks
   it out of `origin` into a worktree named after it, the same way *Worktree ▸
   Switch Branch (Remote)…* does; if a worktree already has that branch, it is
   pulled up to `origin` instead. It then focuses the Explorer on the review
   comment list. A PR from a fork has no branch on `origin`, and a merged or closed one
   may have lost it, so those land on `pr-<N>` from GitHub's `pull/<N>/head`
   instead, and an existing `pr-<N>` is reused as it is.

   To have this happen on its own, add the repository's main worktree to
   `[review] auto_review_repos` in the config. Whichever repository the window
   has open, Conductor asks GitHub once a minute, in each of those repositories,
   for open PRs that request *your* review — directly, not through one of your
   teams — and takes in the first one that has no worktree yet, one PR at a
   time, without moving you off the worktree you are on. The status line names
   the PR; run `/revidere` there for the AI review. While your review stays
   requested, a PR is taken in once: if you delete its worktree it is not
   brought back, and one that failed to come in is not tried again (enter its
   number here by hand). Both are forgotten when the request is dropped, so a
   request made again is taken in again, and so is everything when you restart
   Conductor.
2. **Changed files and comments** — the Explorer's bottom pane cycles between
   the changed-files diff list and the review comment list.
3. **Jump into the code** — the diff pane supports the Viewer's `gd`/`gi`/`gr`
   symbol-jump hints (go to definition / implementation / references). **Symbol
   jumps only work for Rust, Go, and TypeScript** — other languages show no
   hints.
4. **AI review** — run the `/revidere` skill in a Claude Code session in that
   worktree (see `plugins/conductor/skills/revidere/SKILL.md`).
5. **Publish comments** — palette: *Review: Publish Comments to GitHub* posts
   your inline review comments (and replies) back to the PR via `gh`.

Requires the `gh` CLI (see Prerequisites) to be installed and authenticated.

> **Breaking change:** diff mode's "jump to top" moved from `g` to `gg` (vim-style),
> since `g` is now a prefix for symbol-jump hints (`gd`/`gi`/`gr`/`gg`).


