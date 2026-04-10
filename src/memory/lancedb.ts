import { getConfig, log } from "../config.js";

/**
 * Lightweight helper to call the MCP Memory LanceDB `memory_stats` tool
 * via SSE transport and return row counts by category.
 *
 * The memory server uses a single "memories" table with a `category` field.
 * The handlers call countRows("facts"), countRows("goals"), etc. — these
 * map to category counts from memory_stats.
 */

let _statsCache: { data: Record<string, number>; ts: number } | null = null;
const CACHE_TTL_MS = 30_000; // Cache stats for 30 seconds

/**
 * Fetch memory stats from the MCP server via SSE transport.
 * Returns category counts like { fact: 12, decision: 3, ... }
 */
async function fetchStats(): Promise<Record<string, number>> {
  // Return cached if fresh
  if (_statsCache && Date.now() - _statsCache.ts < CACHE_TTL_MS) {
    return _statsCache.data;
  }

  const cfg = getConfig();
  const sseUrl = cfg.SPARK_MEMORY_MCP_URL; // e.g. http://your-server:8282/sse

  try {
    // Step 1: Connect to SSE endpoint to get the messages URL
    const sseRes = await fetch(sseUrl, {
      signal: AbortSignal.timeout(5000),
      headers: { Accept: "text/event-stream" },
    });

    if (!sseRes.ok || !sseRes.body) {
      throw new Error(`SSE connect failed: ${sseRes.status}`);
    }

    // Read the first SSE event to get the endpoint URL
    const reader = sseRes.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let messagesUrl = "";

    while (!messagesUrl) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });

      // Parse SSE events — look for "endpoint" event
      const lines = buffer.split("\n");
      for (const line of lines) {
        if (line.startsWith("data: ")) {
          const data = line.slice(6).trim();
          // The endpoint event contains the messages URL path
          if (data.startsWith("/") || data.startsWith("http")) {
            messagesUrl = data;
          }
        }
      }
    }

    // Clean up the SSE connection
    reader.cancel().catch(() => {});

    if (!messagesUrl) {
      throw new Error("Could not get messages endpoint from SSE");
    }

    // Make absolute URL if relative
    if (messagesUrl.startsWith("/")) {
      const base = new URL(sseUrl);
      messagesUrl = `${base.origin}${messagesUrl}`;
    }

    // Step 2: Initialize MCP session
    await fetch(messagesUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: {
          protocolVersion: "2024-11-05",
          capabilities: {},
          clientInfo: { name: "telegram-bot", version: "0.1.0" },
        },
      }),
      signal: AbortSignal.timeout(5000),
    });

    // Step 3: Call memory_stats tool
    const toolRes = await fetch(messagesUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: {
          name: "memory_stats",
          arguments: {},
        },
      }),
      signal: AbortSignal.timeout(10000),
    });

    if (!toolRes.ok) {
      throw new Error(`tools/call failed: ${toolRes.status}`);
    }

    const result = await toolRes.json();
    const content = result?.result?.content?.[0]?.text;

    if (!content) {
      throw new Error("No content in memory_stats response");
    }

    const stats = JSON.parse(content);
    const byCategory: Record<string, number> = stats.by_category ?? {};

    // Cache it
    _statsCache = { data: byCategory, ts: Date.now() };
    return byCategory;
  } catch (err) {
    log("debug", "Failed to fetch memory stats", err);
    return {};
  }
}

/**
 * Get approximate row count for a memory category.
 *
 * @param table - Category name: "episodes", "facts", "goals", "reflections"
 *                Maps to MCP memory categories: fact, decision, preference, entity, other
 * @param _filter - Unused, kept for API compatibility with handlers.ts
 */
export async function countRows(
  table: string,
  _filter?: string,
): Promise<number> {
  const stats = await fetchStats();

  // Map handler table names to memory categories
  const tableToCategory: Record<string, string[]> = {
    episodes: ["other"],
    facts: ["fact"],
    goals: ["decision"],
    reflections: ["preference"],
  };

  const categories = tableToCategory[table];
  if (!categories) {
    // Direct category lookup
    return stats[table] ?? 0;
  }

  let count = 0;
  for (const cat of categories) {
    count += stats[cat] ?? 0;
  }
  return count;
}
