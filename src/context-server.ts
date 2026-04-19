import { query } from "@anthropic-ai/claude-agent-sdk";
import { randomUUID } from "crypto";
import { type Config, log } from "./config.js";

const MAX_BODY_BYTES = 8 * 1024;
const QUERY_TIMEOUT_MS = 60_000;

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

async function askMira(message: string, cfg: Config): Promise<string> {
  const options: Record<string, unknown> = {
    maxTurns: 10,
    permissionMode: "default",
    cwd: cfg.AGENT_CWD,
    settingSources: ["project", "user"],
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

export async function startContextServer(cfg: Config): Promise<void> {
  const portErr = await checkPortFree(cfg.CONTEXT_SERVER_PORT);
  if (portErr) {
    throw new Error(`Context server: ${portErr}`);
  }

  Bun.serve({
    port: cfg.CONTEXT_SERVER_PORT,
    fetch: async (req) => {
      const requestId = randomUUID().slice(0, 8);
      const start = Date.now();

      // Auth
      const auth = req.headers.get("authorization") ?? "";
      const token = auth.startsWith("Bearer ") ? auth.slice(7) : "";
      if (!token || token !== cfg.CONTEXT_SERVER_SECRET) {
        log("warn", `[context-server] ${requestId} 401 unauthorized`);
        return new Response(null, { status: 401 });
      }

      // Route
      const { pathname } = new URL(req.url);
      if (req.method !== "POST" || pathname !== "/ask") {
        return new Response(JSON.stringify({ error: "not found" }), {
          status: 404,
          headers: { "content-type": "application/json" },
        });
      }

      // Body size guard
      const contentLength = parseInt(req.headers.get("content-length") ?? "0", 10);
      if (contentLength > MAX_BODY_BYTES) {
        return new Response(JSON.stringify({ error: "request too large", request_id: requestId }), {
          status: 400,
          headers: { "content-type": "application/json" },
        });
      }

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
      try {
        const answer = await Promise.race([
          askMira(message, cfg),
          new Promise<never>((_, reject) =>
            setTimeout(() => reject(new Error("timeout")), QUERY_TIMEOUT_MS),
          ),
        ]);
        const dur = Date.now() - start;
        log("info", `[context-server] ${requestId} 200 ${dur}ms`);
        return new Response(JSON.stringify({ answer, request_id: requestId }), {
          headers: { "content-type": "application/json" },
        });
      } catch (err) {
        const isTimeout = err instanceof Error && err.message === "timeout";
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
}
