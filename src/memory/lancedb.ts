import { spawn } from "child_process";
import { getConfig, log } from "../config.js";

/**
 * Lightweight helper to call the MCP Memory LanceDB `memory_stats` tool
 * via stdio transport (spawns the server process, sends JSON-RPC, reads result).
 */

let _statsCache: { data: Record<string, number>; ts: number } | null = null;
const CACHE_TTL_MS = 30_000;

/**
 * Spawn the memory MCP server as a one-shot stdio subprocess,
 * send a JSON-RPC initialize + tools/call request, and parse the result.
 */
async function fetchStats(): Promise<Record<string, number>> {
  if (_statsCache && Date.now() - _statsCache.ts < CACHE_TTL_MS) {
    return _statsCache.data;
  }

  const cfg = getConfig();

  if (!cfg.MEMORY_MCP_COMMAND || !cfg.MEMORY_MCP_SCRIPT) {
    return {};
  }

  try {
    const result = await new Promise<string>((resolve, reject) => {
      const proc = spawn(cfg.MEMORY_MCP_COMMAND!, [cfg.MEMORY_MCP_SCRIPT!], {
        stdio: ["pipe", "pipe", "pipe"],
        timeout: 15000,
      });

      let stdout = "";
      let stderr = "";
      proc.stdout.on("data", (d: Buffer) => { stdout += d.toString(); });
      proc.stderr.on("data", (d: Buffer) => { stderr += d.toString(); });

      proc.on("error", (err) => reject(err));
      proc.on("close", (code) => {
        if (code !== 0) {
          reject(new Error(`memory MCP exited ${code}: ${stderr}`));
        } else {
          resolve(stdout);
        }
      });

      // Send JSON-RPC messages over stdin
      const init = JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: {
          protocolVersion: "2024-11-05",
          capabilities: {},
          clientInfo: { name: "telegram-bot-stats", version: "0.1.0" },
        },
      });

      const call = JSON.stringify({
        jsonrpc: "2.0",
        id: 2,
        method: "tools/call",
        params: { name: "memory_stats", arguments: {} },
      });

      proc.stdin.write(init + "\n");
      proc.stdin.write(call + "\n");
      proc.stdin.end();
    });

    // Parse JSON-RPC responses (one per line)
    const lines = result.trim().split("\n").filter(Boolean);
    for (const line of lines) {
      try {
        const msg = JSON.parse(line);
        if (msg.id === 2) {
          if (msg.error) {
            log("warn", "memory_stats RPC error", msg.error);
            return {};
          }
          if (msg.result?.content?.[0]?.text) {
            const stats = JSON.parse(msg.result.content[0].text);
            const byCategory: Record<string, number> = stats.by_category ?? {};
            _statsCache = { data: byCategory, ts: Date.now() };
            return byCategory;
          }
        }
      } catch {
        continue;
      }
    }

    return {};
  } catch (err) {
    log("warn", "Failed to fetch memory stats", err);
    return {};
  }
}

/**
 * Get approximate row count for a memory category.
 */
export async function countRows(table: string): Promise<number> {
  const stats = await fetchStats();

  const tableToCategory: Record<string, string[]> = {
    episodes: ["other"],
    facts: ["fact"],
    goals: ["decision"],
    reflections: ["preference"],
  };

  const categories = tableToCategory[table];
  if (!categories) {
    return stats[table] ?? 0;
  }

  let count = 0;
  for (const cat of categories) {
    count += stats[cat] ?? 0;
  }
  return count;
}
