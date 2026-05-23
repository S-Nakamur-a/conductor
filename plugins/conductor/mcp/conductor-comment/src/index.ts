#!/usr/bin/env node

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import Database from "better-sqlite3";
import { execSync } from "node:child_process";
import path from "node:path";
import fs from "node:fs";
import crypto from "node:crypto";
import { z } from "zod";

// ---------------------------------------------------------------------------
// DB discovery
// ---------------------------------------------------------------------------

function findDbPath(): string {
  // 1. Env override
  if (process.env.CONDUCTOR_DB_PATH) {
    return process.env.CONDUCTOR_DB_PATH;
  }

  // 2. Find git repo root from cwd, then look for .conductor/conductor.db
  //    Try --show-toplevel first (works for main worktree), then fall back to
  //    --git-common-dir which resolves to the main repo even from linked worktrees.
  try {
    const root = execSync("git rev-parse --show-toplevel", {
      encoding: "utf-8",
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
    const candidate = path.join(root, ".conductor", "conductor.db");
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  } catch {
    // not in a git repo — fall through
  }

  // 3. Worktree-aware fallback: --git-common-dir returns the shared .git dir
  //    (e.g. /main-repo/.git), so its parent is the main repo root.
  try {
    const gitCommonDir = execSync("git rev-parse --git-common-dir", {
      encoding: "utf-8",
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
    const mainRoot = path.resolve(gitCommonDir, "..");
    const candidate = path.join(mainRoot, ".conductor", "conductor.db");
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  } catch {
    // fall through
  }

  throw new Error(
    "Cannot find conductor.db. Set CONDUCTOR_DB_PATH or run from within a git repo that has .conductor/conductor.db"
  );
}

function currentBranch(): string | null {
  try {
    return execSync("git rev-parse --abbrev-ref HEAD", {
      encoding: "utf-8",
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// UI refresh signal
// ---------------------------------------------------------------------------

/**
 * Write to the named pipe `.conductor/refresh.pipe` to trigger a UI refresh
 * in the Conductor TUI.  Non-blocking and best-effort — silently ignores
 * errors (pipe may not exist if TUI is not running).
 */
function signalUiRefresh(): void {
  // Derive the pipe path from the DB path (sibling file in .conductor/).
  try {
    const dbPath = findDbPath();
    const pipePath = path.join(path.dirname(dbPath), "refresh.pipe");

    // Open with O_WRONLY | O_NONBLOCK (flag 0x0001 | 0x0004 on macOS,
    // but we use fs constants).  If the pipe doesn't exist or no reader
    // is attached, this will throw — which we silently catch.
    const fd = fs.openSync(pipePath, fs.constants.O_WRONLY | fs.constants.O_NONBLOCK);
    fs.writeSync(fd, "r");
    fs.closeSync(fd);
  } catch {
    // Pipe not available — TUI is not running or pipe doesn't exist.
    // This is expected and harmless.
  }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface ReviewComment {
  id: string;
  worktree: string;
  file_path: string;
  line_start: number;
  line_end: number | null;
  kind: string;
  body: string;
  status: string;
  commit_ref: string;
  author: string;
  branch: string | null;
  created_at: string;
  updated_at: string;
}

interface ReviewReply {
  id: string;
  review_id: string;
  body: string;
  author: string;
  created_at: string;
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

const server = new McpServer({
  name: "conductor",
  version: "0.3.0",
});

let db: Database.Database;

function getDb(): Database.Database {
  if (!db) {
    const dbPath = findDbPath();
    db = new Database(dbPath, { readonly: false });
    db.pragma("foreign_keys = ON");
    db.pragma("journal_mode = WAL");
  }
  return db;
}

// ---------------------------------------------------------------------------
// Tool: get_pending_comments
// ---------------------------------------------------------------------------

server.tool(
  "get_pending_comments",
  "List unresolved (pending) review comments. By default, only comments for the current git branch are returned. Set all_branches=true to see comments across all branches. Use get_comment_thread to read full details and replies for a specific comment.",
  {
    worktree: z.string().optional().describe("Filter by worktree name"),
    branch: z
      .string()
      .optional()
      .describe(
        "Filter by branch name. If omitted, defaults to the current git branch (auto-detected)."
      ),
    all_branches: z
      .boolean()
      .optional()
      .describe(
        "Set to true to return comments from all branches (disables auto branch filter)"
      ),
    file_path: z
      .string()
      .optional()
      .describe("Filter by file path (exact match)"),
  },
  async ({ worktree, branch, all_branches, file_path }) => {
    const d = getDb();

    // Resolve effective branch filter:
    // 1. Explicit branch param takes priority
    // 2. If all_branches is true, no branch filter
    // 3. Otherwise, auto-detect current branch
    const effectiveBranch =
      branch ?? (all_branches ? undefined : currentBranch() ?? undefined);

    let sql = `
      SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
             commit_ref, author, branch, created_at, updated_at
      FROM reviews
      WHERE status = 'pending'
    `;
    const params: unknown[] = [];

    if (worktree) {
      sql += " AND worktree = ?";
      params.push(worktree);
    }
    if (effectiveBranch) {
      sql += " AND (branch = ? OR worktree = ?)";
      params.push(effectiveBranch, effectiveBranch);
    }
    if (file_path) {
      sql += " AND file_path = ?";
      params.push(file_path);
    }

    sql += " ORDER BY file_path, line_start";

    const rows = d.prepare(sql).all(...params) as ReviewComment[];

    if (rows.length === 0) {
      const branchNote = effectiveBranch
        ? ` (branch: ${effectiveBranch})`
        : "";
      return {
        content: [
          {
            type: "text" as const,
            text: `No pending comments found${branchNote}.`,
          },
        ],
      };
    }

    const lines = rows.map((r) => {
      const loc = r.line_end
        ? `${r.file_path}:${r.line_start}-${r.line_end}`
        : `${r.file_path}:${r.line_start}`;
      return `[${r.kind.toUpperCase()}] ${loc} (id: ${r.id.slice(0, 8)})\n  ${r.body}`;
    });

    const branchNote = effectiveBranch
      ? ` on branch "${effectiveBranch}"`
      : " across all branches";
    const summary = `${rows.length} pending comment(s)${branchNote}:\n\n${lines.join("\n\n")}`;

    return {
      content: [{ type: "text" as const, text: summary }],
    };
  }
);

// ---------------------------------------------------------------------------
// Tool: get_comment_thread
// ---------------------------------------------------------------------------

server.tool(
  "get_comment_thread",
  "Get full details of a review comment and all its replies. Use the comment ID (or prefix) from get_pending_comments.",
  {
    comment_id: z
      .string()
      .describe("Comment ID or unique prefix (min 8 chars)"),
  },
  async ({ comment_id }) => {
    const d = getDb();

    // Support prefix matching
    let comment: ReviewComment | undefined;
    if (comment_id.length < 36) {
      comment = d
        .prepare(
          `SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                  commit_ref, author, branch, created_at, updated_at
           FROM reviews WHERE id LIKE ?`
        )
        .get(`${comment_id}%`) as ReviewComment | undefined;
    } else {
      comment = d
        .prepare(
          `SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                  commit_ref, author, branch, created_at, updated_at
           FROM reviews WHERE id = ?`
        )
        .get(comment_id) as ReviewComment | undefined;
    }

    if (!comment) {
      return {
        content: [
          {
            type: "text" as const,
            text: `Comment not found: ${comment_id}`,
          },
        ],
        isError: true,
      };
    }

    const replies = d
      .prepare(
        `SELECT id, review_id, body, author, created_at
         FROM review_replies WHERE review_id = ? ORDER BY created_at`
      )
      .all(comment.id) as ReviewReply[];

    const loc = comment.line_end
      ? `${comment.file_path}:${comment.line_start}-${comment.line_end}`
      : `${comment.file_path}:${comment.line_start}`;

    let text = `## ${comment.kind.toUpperCase()} — ${loc}\n`;
    text += `ID: ${comment.id}\n`;
    text += `Status: ${comment.status} | Author: ${comment.author}\n`;
    text += `Worktree: ${comment.worktree}`;
    if (comment.branch) text += ` | Branch: ${comment.branch}`;
    text += `\nCreated: ${comment.created_at}\n`;
    text += `\n${comment.body}\n`;

    if (replies.length > 0) {
      text += `\n### Replies (${replies.length})\n`;
      for (const r of replies) {
        text += `\n**${r.author}** (${r.created_at}):\n${r.body}\n`;
      }
    }

    return {
      content: [{ type: "text" as const, text }],
    };
  }
);

// ---------------------------------------------------------------------------
// Tool: reply_to_comment
// ---------------------------------------------------------------------------

server.tool(
  "reply_to_comment",
  "Add a reply to a review comment. Author is automatically set to 'claude'.",
  {
    comment_id: z
      .string()
      .describe("Comment ID or unique prefix (min 8 chars)"),
    body: z.string().describe("Reply text"),
  },
  async ({ comment_id, body }) => {
    const d = getDb();

    // Resolve prefix
    let resolvedId: string;
    if (comment_id.length < 36) {
      const row = d
        .prepare("SELECT id FROM reviews WHERE id LIKE ?")
        .get(`${comment_id}%`) as { id: string } | undefined;
      if (!row) {
        return {
          content: [
            {
              type: "text" as const,
              text: `Comment not found: ${comment_id}`,
            },
          ],
          isError: true,
        };
      }
      resolvedId = row.id;
    } else {
      resolvedId = comment_id;
    }

    const id = crypto.randomUUID();
    d.prepare(
      "INSERT INTO review_replies (id, review_id, body, author) VALUES (?, ?, ?, 'claude')"
    ).run(id, resolvedId, body);

    signalUiRefresh();

    return {
      content: [
        {
          type: "text" as const,
          text: `Reply added (id: ${id.slice(0, 8)}) to comment ${resolvedId.slice(0, 8)}.`,
        },
      ],
    };
  }
);

// ---------------------------------------------------------------------------
// Tool: resolve_comment
// ---------------------------------------------------------------------------

server.tool(
  "resolve_comment",
  "Mark a review comment as resolved.",
  {
    comment_id: z
      .string()
      .describe("Comment ID or unique prefix (min 8 chars)"),
  },
  async ({ comment_id }) => {
    const d = getDb();

    // Resolve prefix
    let resolvedId: string;
    if (comment_id.length < 36) {
      const row = d
        .prepare("SELECT id FROM reviews WHERE id LIKE ?")
        .get(`${comment_id}%`) as { id: string } | undefined;
      if (!row) {
        return {
          content: [
            {
              type: "text" as const,
              text: `Comment not found: ${comment_id}`,
            },
          ],
          isError: true,
        };
      }
      resolvedId = row.id;
    } else {
      resolvedId = comment_id;
    }

    const result = d
      .prepare(
        "UPDATE reviews SET status = 'resolved', updated_at = datetime('now') WHERE id = ?"
      )
      .run(resolvedId);

    if (result.changes === 0) {
      return {
        content: [
          {
            type: "text" as const,
            text: `Comment not found: ${resolvedId}`,
          },
        ],
        isError: true,
      };
    }

    signalUiRefresh();

    return {
      content: [
        {
          type: "text" as const,
          text: `Comment ${resolvedId.slice(0, 8)} marked as resolved.`,
        },
      ],
    };
  }
);

// ---------------------------------------------------------------------------
// Tool: create_comment
// ---------------------------------------------------------------------------

server.tool(
  "create_comment",
  "Leave an inline self-review comment on a specific file and line range in the current branch's diff. Author is automatically set to 'claude' and the comment appears inline in the Conductor diff view. " +
    "Use this SPARINGLY and with high signal: flag ONLY what a human reviewer genuinely needs to know — non-obvious or tricky logic, deliberate tradeoffs, decisions worth a second look, or places you are unsure about. " +
    "Do NOT narrate routine changes or restate what the diff already makes obvious; a flood of low-value comments defeats the purpose. Prefer a handful of important notes over many.",
  {
    file_path: z
      .string()
      .describe("Repo-relative file path the comment attaches to (e.g. src/foo.rs)"),
    line_start: z
      .number()
      .int()
      .positive()
      .describe("1-based line number the comment starts on"),
    line_end: z
      .number()
      .int()
      .positive()
      .optional()
      .describe(
        "1-based end line for a multi-line range; omit for a single-line comment"
      ),
    body: z.string().min(1).describe("The comment text"),
    kind: z
      .enum(["suggest", "question"])
      .optional()
      .describe(
        "'suggest' (default) for a note/observation/tradeoff; 'question' when you want the human to answer something"
      ),
  },
  async ({ file_path, line_start, line_end, body, kind }) => {
    const d = getDb();

    if (line_end !== undefined && line_end < line_start) {
      return {
        content: [
          {
            type: "text" as const,
            text: `Invalid range: line_end (${line_end}) is before line_start (${line_start}).`,
          },
        ],
        isError: true,
      };
    }

    // Comments are rendered inline keyed on the file path. An absolute or
    // dot-prefixed path won't match the repo-relative key the TUI uses, so the
    // comment would insert successfully but never appear — fail loudly instead.
    if (path.isAbsolute(file_path)) {
      return {
        content: [
          {
            type: "text" as const,
            text: `file_path must be repo-relative (e.g. src/foo.rs), got absolute path: ${file_path}`,
          },
        ],
        isError: true,
      };
    }
    const relPath = file_path.replace(/^\.\//, "");

    // Comments are keyed to a branch/worktree. The Rust side stores the
    // worktree's branch name in both the `worktree` and `branch` columns
    // (see add_review in review_store.rs), and uses the symbolic ref "HEAD"
    // as commit_ref — mirror that exactly so the comment shows up in the TUI.
    // A detached HEAD reports the literal "HEAD", which is not a usable key.
    const branch = currentBranch();
    if (!branch || branch === "HEAD") {
      return {
        content: [
          {
            type: "text" as const,
            text: "Cannot determine the current git branch (detached HEAD?); a comment must be attached to a branch.",
          },
        ],
        isError: true,
      };
    }

    const id = crypto.randomUUID();
    try {
      d.prepare(
        `INSERT INTO reviews
           (id, worktree, file_path, line_start, line_end, kind, body, commit_ref, author, branch)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'HEAD', 'claude', ?)`
      ).run(
        id,
        branch,
        relPath,
        line_start,
        line_end ?? null,
        kind ?? "suggest",
        body,
        branch
      );
    } catch (err) {
      return {
        content: [
          {
            type: "text" as const,
            text: `Failed to create comment: ${err instanceof Error ? err.message : String(err)}`,
          },
        ],
        isError: true,
      };
    }

    signalUiRefresh();

    const loc = line_end
      ? `${relPath}:${line_start}-${line_end}`
      : `${relPath}:${line_start}`;

    return {
      content: [
        {
          type: "text" as const,
          text: `Comment created (id: ${id.slice(0, 8)}) at ${loc} on branch "${branch}".`,
        },
      ],
    };
  }
);

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch((err) => {
  console.error("Fatal:", err);
  process.exit(1);
});
