import { z } from "zod";
import { resolve } from "path";
import { statSync } from "fs";

const envSchema = z.object({
  TELEGRAM_BOT_TOKEN: z.string().min(1),
  ALLOWED_USERS: z
    .string()
    .transform((s) => s.split(",").map(Number))
    .refine((arr) => arr.length > 0 && arr.every((n) => Number.isInteger(n) && n > 0), {
      message: "ALLOWED_USERS must be a comma-separated list of positive Telegram user IDs",
    }),

  // ── Optional: Memory — LanceDB via MCP stdio subprocess ──
  // When set, the agent gets persistent long-term memory (store/recall/forget).
  MEMORY_MCP_COMMAND: z
    .string()
    .regex(/^[^\s;&|$`(){}\\]+$/, "MEMORY_MCP_COMMAND must be a plain executable path with no shell metacharacters")
    .optional(),
  MEMORY_MCP_SCRIPT: z.string().optional(),

  // ── Optional: Doc search — QMD MCP server ──
  // Enables the agent to search indexed markdown documents (notes, reports, docs).
  SPARK_QMD_MCP_URL: z.string().url().refine(
    (u) => u.startsWith("http://") || u.startsWith("https://"),
    { message: "SPARK_QMD_MCP_URL must use http or https" },
  ).optional(),

  // ── Optional: Voice transcription — Whisper (OpenAI-compatible endpoint) ──
  // When set, voice messages are transcribed and sent to the agent as text.
  SPARK_WHISPER_URL: z.string().url().optional(),

  // ── Optional: additional services used by workspace MCP config ──
  // These are not used directly by the bot process; configure in .mcp.json.
  SPARK_EMBED_URL: z.string().url().optional(),
  SPARK_EMBED_MODEL: z.string().default("BAAI/bge-m3"),
  SPARK_RERANK_URL: z.string().url().optional(),
  SPARK_RERANK_MODEL: z.string().default("BAAI/bge-reranker-v2-m3"),
  SPARK_TTS_URL: z.string().url().optional(),
  SPARK_SEARCH_URL: z.string().url().optional(),
  SPARK_CRAWL_URL: z.string().url().optional(),

  // ── Agent SDK ──
  AGENT_CWD: z
    .string()
    .default("./workspace")
    .transform((p) => resolve(p))
    .refine((p) => { try { return statSync(p).isDirectory(); } catch { return false; } }, {
      message: "AGENT_CWD must be an existing directory",
    }),
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

  // ── Context server — HTTP endpoint for ask_mira / notify_mira MCP tools ──
  CONTEXT_SERVER_PORT: z.coerce.number().default(3001),
  CONTEXT_SERVER_SECRET: z.string().min(1),
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
