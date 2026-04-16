import { z } from "zod";

const envSchema = z.object({
  TELEGRAM_BOT_TOKEN: z.string().min(1),
  ALLOWED_USERS: z
    .string()
    .transform((s) => s.split(",").map(Number))
    .refine((arr) => arr.length > 0 && arr.every((n) => Number.isInteger(n) && n > 0), {
      message: "ALLOWED_USERS must be a comma-separated list of positive Telegram user IDs",
    }),

  // ── DGX Spark services (your-server) ──

  // Memory MCP LanceDB (stdio subprocess)
  MEMORY_MCP_COMMAND: z.string().default("./workspace/mcp-memory-lancedb/.venv/bin/python"),
  MEMORY_MCP_SCRIPT: z.string().default("./workspace/mcp-memory-lancedb/server.py"),

  // Existing QMD Doc Search MCP (port 8181)
  SPARK_QMD_MCP_URL: z.string().url(),

  // BGE-M3 embeddings (port 8001)
  SPARK_EMBED_URL: z.string().url(),
  SPARK_EMBED_MODEL: z.string().default("BAAI/bge-m3"),

  // BGE Reranker v2 M3 (port 8002)
  SPARK_RERANK_URL: z.string().url(),
  SPARK_RERANK_MODEL: z.string().default("BAAI/bge-reranker-v2-m3"),

  // Whisper transcription (port 8007)
  SPARK_WHISPER_URL: z.string().url(),

  // Kokoro TTS (port 8008)
  SPARK_TTS_URL: z.string().url(),

  // SearXNG web search (port 8080)
  SPARK_SEARCH_URL: z.string().url(),

  // Crawl4AI (port 11235)
  SPARK_CRAWL_URL: z.string().url(),

  // ── Agent SDK ──
  AGENT_CWD: z.string().default("./workspace"),
  AGENT_MAX_TURNS: z.coerce.number().default(15),
  AGENT_PERMISSION_MODE: z
    .enum(["bypassPermissions", "acceptEdits", "default"])
    .default("bypassPermissions"),

  // Projects
  PROJECTS_ROOT: z.string().default("./projects"),

  // Memory
  MEMORY_TOKEN_BUDGET: z.coerce.number().default(2000),

  // Logging
  LOG_LEVEL: z.enum(["debug", "info", "warn", "error"]).default("info"),
});

export type Config = z.infer<typeof envSchema>;

let _config: Config | null = null;

export function getConfig(): Config {
  if (!_config) {
    _config = envSchema.parse(process.env);
  }
  return _config;
}

export function log(
  level: "debug" | "info" | "warn" | "error",
  msg: string,
  data?: unknown,
) {
  const levels = { debug: 0, info: 1, warn: 2, error: 3 };
  const cfg = getConfig();
  if (levels[level] >= levels[cfg.LOG_LEVEL]) {
    const ts = new Date().toISOString();
    const prefix = `[${ts}] [${level.toUpperCase()}]`;
    if (data) {
      console[level](`${prefix} ${msg}`, data);
    } else {
      console[level](`${prefix} ${msg}`);
    }
  }
}
