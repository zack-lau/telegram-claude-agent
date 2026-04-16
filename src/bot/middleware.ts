import { type Context, type NextFunction } from "grammy";
import { getConfig, log } from "../config.js";

/**
 * User allowlist middleware.
 * Only users in ALLOWED_USERS can interact with the bot.
 */
export async function authMiddleware(
  ctx: Context,
  next: NextFunction,
): Promise<void> {
  const cfg = getConfig();
  const userId = ctx.from?.id;

  if (!userId || !cfg.ALLOWED_USERS.includes(userId)) {
    log("warn", `Unauthorized access attempt from user ${userId}`);
    // Silent drop — don't reveal the bot exists to strangers
    return;
  }

  await next();
}

/**
 * Simple rate limiter to prevent accidental message floods.
 * Max 5 messages per 10 seconds per user.
 */
const rateLimitMap = new Map<number, number[]>();
const RATE_WINDOW_MS = 10_000;
const RATE_MAX = 5;

export async function rateLimitMiddleware(
  ctx: Context,
  next: NextFunction,
): Promise<void> {
  const userId = ctx.from?.id;
  if (!userId) return;

  const now = Date.now();
  const timestamps = rateLimitMap.get(userId) ?? [];

  // Remove old timestamps outside the window; evict entry if it goes empty
  const recent = timestamps.filter((t) => now - t < RATE_WINDOW_MS);

  if (recent.length >= RATE_MAX) {
    log("warn", `Rate limited user ${userId}`);
    await ctx.reply("⏳ Slow down — too many messages. Try again in a few seconds.");
    return;
  }

  recent.push(now);
  if (recent.length > 0) {
    rateLimitMap.set(userId, recent);
  } else {
    rateLimitMap.delete(userId);
  }

  await next();
}

/**
 * Ignore old messages that arrive after bot restart.
 * Drops any message older than 30 seconds.
 */
export async function staleFilterMiddleware(
  ctx: Context,
  next: NextFunction,
): Promise<void> {
  const messageDate = ctx.message?.date;
  if (messageDate) {
    const ageSeconds = Math.floor(Date.now() / 1000) - messageDate;
    if (ageSeconds > 30) {
      log("debug", `Dropped stale message (${ageSeconds}s old)`);
      return;
    }
  }
  await next();
}
