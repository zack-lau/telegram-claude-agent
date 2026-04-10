import { Bot } from "grammy";
import { getConfig, log } from "../config.js";
import {
  authMiddleware,
  rateLimitMiddleware,
  staleFilterMiddleware,
} from "./middleware.js";
import {
  handleStart,
  handleNew,
  handleStatus,
  handleMemory,
  handleMessage,
  handleVoice,
  handlePhoto,
  handleDocument,
  setBot,
} from "./handlers.js";
import { registerApprovalHandler } from "./approvals.js";

export function createBot(): Bot {
  const cfg = getConfig();
  const bot = new Bot(cfg.TELEGRAM_BOT_TOKEN);

  // ── Middleware stack ──
  // Order matters: stale filter → auth → rate limit
  bot.use(staleFilterMiddleware);
  bot.use(authMiddleware);
  bot.use(rateLimitMiddleware);

  // ── Share bot instance with handlers ──
  setBot(bot);

  // ── Approval inline keyboard handler (must be before commands) ──
  registerApprovalHandler(bot);

  // ── Commands ──
  bot.command("start", handleStart);
  bot.command("new", handleNew);
  bot.command("status", handleStatus);
  bot.command("memory", handleMemory);

  // ── Message handlers ──
  bot.on("message:text", handleMessage);
  bot.on("message:voice", handleVoice);
  bot.on("message:audio", handleVoice);
  bot.on("message:photo", handlePhoto);
  bot.on("message:document", handleDocument);

  // ── Error handler ──
  bot.catch((err) => {
    log("error", "Bot error", err);
  });

  return bot;
}
