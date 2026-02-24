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
  version: "0.1.0",
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
      sql += " AND branch = ?";
      params.push(effectiveBranch);
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
