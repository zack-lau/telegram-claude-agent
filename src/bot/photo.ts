import { type Context } from "grammy";
import { getConfig, log } from "../config.js";
import type { ImageAttachment } from "../agent/agent.js";

/**
 * Download a photo from Telegram and return it as a base64 ImageAttachment.
 * Telegram sends photos in multiple sizes — we pick the largest one.
 */
export async function downloadPhoto(ctx: Context): Promise<ImageAttachment | null> {
  const cfg = getConfig();
  const photos = ctx.message?.photo;
  if (!photos || photos.length === 0) return null;

  // Last element is the largest resolution
  const largest = photos[photos.length - 1];

  try {
    const file = await ctx.api.getFile(largest.file_id);
    if (!file.file_path) {
      log("warn", "Telegram returned no file_path for photo");
      return null;
    }

    const fileUrl = `https://api.telegram.org/file/bot${cfg.TELEGRAM_BOT_TOKEN}/${file.file_path}`;
    const resp = await fetch(fileUrl);
    if (!resp.ok) {
      log("warn", `Failed to download photo: ${resp.status}`);
      return null;
    }

    const buffer = await resp.arrayBuffer();
    const base64 = Buffer.from(buffer).toString("base64");

    // Telegram photos are always JPEG
    return { base64, mediaType: "image/jpeg" };
  } catch (err) {
    log("error", "Photo download error", err);
    return null;
  }
}
