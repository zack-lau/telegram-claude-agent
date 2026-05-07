import { query } from "@anthropic-ai/claude-agent-sdk";
import { randomUUID, timingSafeEqual } from "crypto";
import { type Config, log } from "./config.js";
import { createProjectMcpServer } from "./memory/project-tools.js";

const notifyProjectServer = createProjectMcpServer();

const MAX_BODY_BYTES = 8 * 1024;
const QUERY_TIMEOUT_MS = 60_000;
const NOTIFY_TIMEOUT_MS = 240_000;

// ── Notification types ──

interface Notification {
  id: string;
  message: string;
  project?: string;
  drive_url?: string;
  action: "info" | "review" | "integrate" | "followup";
  priority: "low" | "normal" | "urgent";
  artifacts?: string[];
  received_at: string;
}

const VALID_ACTIONS = new Set(["info", "review", "integrate", "followup"]);
const VALID_PRIORITIES = new Set(["low", "normal", "urgent"]);

// ── Deduplication ──
// Tracks message hashes to prevent duplicate processing from retries.
// Key: hash of (message + project + drive_url), Value: timestamp
const DEDUP_WINDOW_MS = 5 * 60 * 1000; // 5 minutes
const recentNotifications = new Map<string, number>();

function notificationHash(message: string, project?: string, driveUrl?: string): string {
  // Use \0 as separator — cannot appear in JSON string fields
  const raw = `${message}\0${project ?? ""}\0${driveUrl ?? ""}`;
  // Simple djb2 hash — good enough for dedup
  let hash = 5381;
  for (let i = 0; i < raw.length; i++) {
    hash = ((hash << 5) + hash + raw.charCodeAt(i)) >>> 0;
  }
  return hash.toString(36);
}

function isDuplicate(hash: string): boolean {
  const now = Date.now();
  // Clean expired entries — collect first to avoid mutating map during iteration
  const staleKeys = [...recentNotifications.entries()]
    .filter(([, ts]) => now - ts > DEDUP_WINDOW_MS)
    .map(([key]) => key);
  for (const key of staleKeys) recentNotifications.delete(key);
  if (recentNotifications.has(hash)) return true;
  recentNotifications.set(hash, now);
  return false;
}

// ── Concurrency limiter ──
// Limits concurrent notification processing to prevent resource exhaustion from bursts.
const MAX_CONCURRENT_NOTIFICATIONS = 3;
const MAX_QUEUED_NOTIFICATIONS = 20;
let activeNotifications = 0;
const notificationQueue: Array<() => void> = [];

async function withConcurrencyLimit<T>(fn: () => Promise<T>): Promise<T> {
  if (activeNotifications >= MAX_CONCURRENT_NOTIFICATIONS) {
    if (notificationQueue.length >= MAX_QUEUED_NOTIFICATIONS) {
      throw new Error("notification queue full");
    }
    // Wait for a slot
    await new Promise<void>((resolve) => notificationQueue.push(resolve));
  }
  activeNotifications++;
  try {
    return await fn();
  } finally {
    activeNotifications--;
    const next = notificationQueue.shift();
    if (next) next();
  }
}

/** Returns null if port is free, error message if in use. */
async function checkPortFree(port: number): Promise<string | null> {
  const conn = await Bun.connect({
    hostname: "127.0.0.1",
    port,
    socket: { data() {}, open() {}, close() {}, error() {} },
  }).catch(() => null);
  if (conn) {
    conn.end();
    return `Port ${port} is already in use. Stop the existing process before starting.`;
  }
  return null;
}

async function askMira(message: string, cfg: Config, signal: AbortSignal): Promise<string> {
  const options: Record<string, unknown> = {
    maxTurns: 10,
    permissionMode: "bypassPermissions",
    cwd: cfg.AGENT_CWD,
    settingSources: [],
    systemPrompt:
      "You are a read-only assistant. Answer questions about projects, " +
      "decisions, and stored context using memory and document search. " +
      "Be factual and concise. Do not execute tasks or take any actions.",
    mcpServers: {
      ...(cfg.MEMORY_MCP_COMMAND && cfg.MEMORY_MCP_SCRIPT
        ? { memory: { command: cfg.MEMORY_MCP_COMMAND, args: [cfg.MEMORY_MCP_SCRIPT] } }
        : {}),
      ...(cfg.SPARK_QMD_MCP_URL
        ? { qmd: { type: "sse", url: cfg.SPARK_QMD_MCP_URL } }
        : {}),
    },
    allowedTools: [
      ...(cfg.MEMORY_MCP_COMMAND ? ["mcp__memory__memory_recall"] : []),
      ...(cfg.SPARK_QMD_MCP_URL
        ? ["mcp__qmd__query", "mcp__qmd__get", "mcp__qmd__multi_get"]
        : []),
    ],
  };

  const stream = query({ prompt: message, options: options as any });
  signal.addEventListener("abort", () => { try { stream.close(); } catch {} }, { once: true });

  const iterator = stream[Symbol.asyncIterator]();
  let answer = "";
  let done = false;

  while (!done) {
    const next = await iterator.next();
    if (next.done) { done = true; break; }
    const msg = next.value;
    if (msg.type === "assistant" && Array.isArray(msg.message?.content)) {
      for (const block of msg.message.content) {
        if (block.type === "text" && block.text) {
          answer += block.text;
        }
      }
    }
  }

  return answer;
}

async function processNotification(notification: Notification, cfg: Config): Promise<void> {
  const parts: string[] = [];
  parts.push(`[Notification from Perplexity Computer]`);
  parts.push(`Priority: ${notification.priority}`);
  parts.push(`Action: ${notification.action}`);
  if (notification.project) parts.push(`Project: ${notification.project}`);
  if (notification.drive_url) parts.push(`Drive URL: ${notification.drive_url.replace(/[\r\n]/g, "")}`);
  if (notification.artifacts?.length) {
    parts.push(`Artifacts: ${notification.artifacts.map(a => a.replace(/[\r\n]/g, "")).join(", ")}`);
  }
  parts.push("");
  parts.push(notification.message);
  parts.push("");
  parts.push(
    "Process this notification:\n\n" +
    "1. CHECK PROJECT STATUS:\n" +
    "   - If no project slug is specified, treat as an untagged research update: store to memory " +
    "with tag 'thread:untagged' and include all details. Skip remaining steps.\n" +
    "   - If a project slug IS specified, use project_list to check if it exists.\n" +
    "   - If the project EXISTS: proceed normally with steps 2-7.\n" +
    "   - If the project DOES NOT EXIST: this is a research thread. First, check memory " +
    "for existing 'thread:<slug>' entries. If ALL previous entries are archived " +
    "(prefixed with '[archived'), the thread was intentionally closed or promoted — " +
    "store the notification to memory with tag 'thread:<slug>' but also note " +
    "'(thread was previously archived)' so Mira can flag this to the user. " +
    "Otherwise, store to memory with the tag convention 'thread:<slug>' in the text. " +
    "Include all notification details (message, drive URL, artifacts, timestamp). " +
    "Then count how many non-archived 'thread:<slug>' entries exist. " +
    "If there are 3+ notifications OR they span 3+ days, store a memory entry flagged " +
    "'thread-promotion:<slug>' noting that the thread is ready for promotion and why " +
    "(notification count, timespan, summary of work). Mira will surface this to the user " +
    "on their next interaction.\n\n" +
    "2. RETRIEVE DRIVE FILES: If a Drive URL is provided, retrieve the file content " +
    "using Google Drive tools (read_file_content or download_file_content), then save " +
    "it to the project folder (/Users/zack/claude-agent/projects/<project>/). " +
    "Use a sensible filename based on the artifact name or document title. " +
    "If no project exists yet (thread), save to /Users/zack/claude-agent/workspace/incoming/<slug>/ instead.\n\n" +
    "3. STORE TO MEMORY: Store relevant info including Drive file IDs, artifact names, " +
    "and a summary of the work product.\n\n" +
    "4. UPDATE PROJECT: If a project is specified and exists, update its context " +
    "(CLAUDE.md or project memory) with the new information.\n\n" +
    "5. TAG PRIORITY in your memory entry so Mira knows urgency " +
    "(low = routine, normal = mention on next chat, urgent = flag prominently).\n\n" +
    "6. If action is 'integrate', delegate to the project agent via project_work " +
    "to pull in the artifact.\n\n" +
    "7. If action is 'followup', store as a pending action item for the user.\n\n" +
    "8. THREAD CLEANUP: If this notification is for an existing project and you find " +
    "old 'thread:<slug>' memory entries for the same slug, it means the thread was " +
    "already promoted. Update those old entries with memory_update to prepend " +
    "'[archived — promoted to project]' so they don't trigger further promotion logic."
  );

  const prompt = parts.join("\n");

  const options: Record<string, unknown> = {
    maxTurns: 15,
    permissionMode: "bypassPermissions",
    cwd: cfg.AGENT_CWD,
    systemPrompt:
      "You are Mira, a personal AI assistant. You received an async notification " +
      "from Perplexity Computer about completed work.\n\n" +
      "You manage two concepts:\n" +
      "- PROJECTS: Full project dirs under /Users/zack/claude-agent/projects/<slug>/ with CLAUDE.md. " +
      "Created via project_create, listed via project_list, delegated via project_work.\n" +
      "- RESEARCH THREADS: Lightweight tagged memory entries for work that doesn't have a project yet. " +
      "Tagged as 'thread:<slug>' in memory. Threads can be promoted to projects when they accumulate enough activity.\n\n" +
      "You have access to memory, project tools, QMD doc search, and filesystem tools. " +
      "You do NOT have direct Telegram access — store info in memory for Mira to relay. " +
      "Follow the processing instructions carefully.",
    settingSources: ["user", "project"],
    mcpServers: {
      ...(cfg.MEMORY_MCP_COMMAND && cfg.MEMORY_MCP_SCRIPT
        ? { memory: { command: cfg.MEMORY_MCP_COMMAND, args: [cfg.MEMORY_MCP_SCRIPT] } }
        : {}),
      ...(cfg.SPARK_QMD_MCP_URL
        ? { qmd: { type: "sse", url: cfg.SPARK_QMD_MCP_URL } }
        : {}),
      projects: notifyProjectServer,
    },
    allowedTools: [
      ...(cfg.MEMORY_MCP_COMMAND ? ["mcp__memory__*"] : []),
      ...(cfg.SPARK_QMD_MCP_URL ? ["mcp__qmd__*"] : []),
      "mcp__projects__project_list",
      "mcp__projects__project_work",
      // Note: no project_create — threads don't auto-create projects
      "Read", "Write", "Bash", "Glob",
    ],
  };

  const ac = new AbortController();
  const timer = setTimeout(() => ac.abort(), NOTIFY_TIMEOUT_MS);

  try {
    const stream = query({ prompt, options: options as any });
    ac.signal.addEventListener("abort", () => stream.close(), { once: true });

    const iterator = stream[Symbol.asyncIterator]();
    let done = false;
    while (!done) {
      const next = await iterator.next();
      if (next.done) { done = true; }
    }
    clearTimeout(timer);
    log("info", `[context-server] notification ${notification.id} processed`);
  } catch (err) {
    clearTimeout(timer);
    if (ac.signal.aborted) {
      log("warn", `[context-server] notification ${notification.id} timed out after ${NOTIFY_TIMEOUT_MS / 1000}s`);
    } else {
      log("error", `[context-server] notification ${notification.id} processing failed`, err);
    }
  }
}

export async function startContextServer(cfg: Config): Promise<ReturnType<typeof Bun.serve>> {
  const portErr = await checkPortFree(cfg.CONTEXT_SERVER_PORT);
  if (portErr) {
    throw new Error(`Context server: ${portErr}`);
  }

  const server = Bun.serve({
    port: cfg.CONTEXT_SERVER_PORT,
    maxRequestBodySize: MAX_BODY_BYTES,
    fetch: async (req) => {
      const requestId = randomUUID().slice(0, 8);
      const start = Date.now();

      // Auth
      const auth = req.headers.get("authorization") ?? "";
      const token = auth.startsWith("Bearer ") ? auth.slice(7) : "";
      const secretBuf = Buffer.from(cfg.CONTEXT_SERVER_SECRET);
      const tokenBuf = Buffer.from(token);
      const authed =
        token.length === cfg.CONTEXT_SERVER_SECRET.length &&
        timingSafeEqual(tokenBuf, secretBuf);
      if (!authed) {
        log("warn", `[context-server] ${requestId} 401 unauthorized`);
        return new Response(null, { status: 401 });
      }

      // Route
      const { pathname } = new URL(req.url);
      if (req.method !== "POST" || (pathname !== "/ask" && pathname !== "/notify")) {
        return new Response(JSON.stringify({ error: "not found" }), {
          status: 404,
          headers: { "content-type": "application/json" },
        });
      }

      // ── /notify — async notification from Perplexity ──
      if (pathname === "/notify") {
        let body: Record<string, unknown>;
        try {
          body = await req.json() as Record<string, unknown>;
        } catch {
          return new Response(JSON.stringify({ error: "invalid JSON", id: requestId }), {
            status: 400,
            headers: { "content-type": "application/json" },
          });
        }

        const message = typeof body.message === "string" ? body.message.trim() : "";
        if (!message) {
          return new Response(JSON.stringify({ error: "message is required", id: requestId }), {
            status: 400,
            headers: { "content-type": "application/json" },
          });
        }

        const action = typeof body.action === "string" && VALID_ACTIONS.has(body.action) ? body.action : "info";
        const priority = typeof body.priority === "string" && VALID_PRIORITIES.has(body.priority) ? body.priority : "normal";

        // Sanitize project slug: lowercase, alphanumeric + hyphens only
        const rawProject = typeof body.project === "string" ? body.project : undefined;
        const project = rawProject
          ? rawProject.toLowerCase().replace(/[^a-z0-9-]/g, "-").replace(/-+/g, "-").replace(/^-|-$/g, "")
          : undefined;

        const notification: Notification = {
          id: requestId,
          message,
          project: project || undefined,
          drive_url: typeof body.drive_url === "string" && body.drive_url.startsWith("https://") ? body.drive_url : undefined,
          action: action as Notification["action"],
          priority: priority as Notification["priority"],
          artifacts: Array.isArray(body.artifacts) ? body.artifacts.filter((a): a is string => typeof a === "string").slice(0, 50).map(s => s.slice(0, 500)) : undefined,
          received_at: new Date().toISOString(),
        };

        // Dedup check
        const hash = notificationHash(message, project, notification.drive_url);
        if (isDuplicate(hash)) {
          log("info", `[context-server] ${requestId} duplicate notification skipped (hash=${hash})`);
          return new Response(JSON.stringify({ status: "duplicate", id: requestId }), {
            status: 200,
            headers: { "content-type": "application/json" },
          });
        }

        log("info", `[context-server] ${requestId} notification received (priority=${priority}, action=${action}, project=${project ?? "none"}, queue=${activeNotifications}/${MAX_CONCURRENT_NOTIFICATIONS})`);

        // Check queue capacity before dispatching — reject early with 429 if full
        if (activeNotifications >= MAX_CONCURRENT_NOTIFICATIONS && notificationQueue.length >= MAX_QUEUED_NOTIFICATIONS) {
          log("warn", `[context-server] ${requestId} rejected — notification queue full (${notificationQueue.length} queued)`);
          return new Response(JSON.stringify({ error: "queue full, retry later", id: requestId }), {
            status: 429,
            headers: { "content-type": "application/json", "retry-after": "60" },
          });
        }

        // Process async with concurrency limit — return 202 immediately
        withConcurrencyLimit(() => processNotification(notification, cfg)).catch((err) => {
          log("error", `[context-server] ${requestId} async processing failed`, err);
        });

        return new Response(JSON.stringify({ status: "accepted", id: requestId }), {
          status: 202,
          headers: { "content-type": "application/json" },
        });
      }

      // ── /ask — synchronous Q&A ──

      // Parse body
      let message: string;
      try {
        const body = await req.json() as { message?: unknown };
        if (!body.message || typeof body.message !== "string") throw new Error();
        message = body.message;
      } catch {
        return new Response(JSON.stringify({ error: "body must be {message: string}", request_id: requestId }), {
          status: 400,
          headers: { "content-type": "application/json" },
        });
      }

      // Query with timeout
      const ac = new AbortController();
      const timer = setTimeout(() => ac.abort(), QUERY_TIMEOUT_MS);
      try {
        const answer = await askMira(message, cfg, ac.signal);
        clearTimeout(timer);
        const dur = Date.now() - start;
        log("info", `[context-server] ${requestId} 200 ${dur}ms`);
        return new Response(JSON.stringify({ answer, request_id: requestId }), {
          headers: { "content-type": "application/json" },
        });
      } catch (err) {
        clearTimeout(timer);
        const isTimeout = ac.signal.aborted;
        const dur = Date.now() - start;
        log(isTimeout ? "warn" : "error", `[context-server] ${requestId} ${isTimeout ? "503" : "500"} ${dur}ms`, err);
        const status = isTimeout ? 503 : 500;
        const error = isTimeout
          ? "query timed out"
          : (err instanceof Error ? err.message : "internal error");
        return new Response(JSON.stringify({ error, request_id: requestId }), {
          status,
          headers: { "content-type": "application/json" },
        });
      }
    },
  });

  log("info", `Context server listening on port ${cfg.CONTEXT_SERVER_PORT}`);
  return server;
}
