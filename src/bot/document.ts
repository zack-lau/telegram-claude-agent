import { type Context } from "grammy";
import { mkdirSync, writeFileSync, existsSync } from "fs";
import { join, basename, extname, resolve } from "path";
import { getConfig, log } from "../config.js";

// Use absolute path so the agent (running in AGENT_CWD) can access these files
const FILES_DIR = resolve("./data/files");
const MAX_FILE_SIZE = 20 * 1024 * 1024; // Telegram Bot API limit

export interface FileAttachment {
  filePath: string;
  fileName: string;
  mimeType: string;
  sizeBytes: number;
}

/**
 * Download a document from Telegram and save it to disk.
 * Returns the local file path and metadata, or null with a reason string.
 */
export async function downloadDocument(ctx: Context): Promise<FileAttachment | null> {
  const cfg = getConfig();
  const doc = ctx.message?.document;
  if (!doc) return null;

  if (doc.file_size && doc.file_size > MAX_FILE_SIZE) {
    return null;
  }

  try {
    const file = await ctx.api.getFile(doc.file_id);
    if (!file.file_path) {
      log("warn", "Telegram returned no file_path for document");
      return null;
    }

    const fileUrl = `https://api.telegram.org/file/bot${cfg.TELEGRAM_BOT_TOKEN}/${file.file_path}`;
    const resp = await fetch(fileUrl);
    if (!resp.ok) {
      log("warn", `Failed to download document: ${resp.status}`);
      return null;
    }

    const buffer = Buffer.from(await resp.arrayBuffer());
    const rawName = basename(doc.file_name || `file_${Date.now()}`);
    const fileName = uniqueName(rawName);

    mkdirSync(FILES_DIR, { recursive: true });
    const localPath = join(FILES_DIR, fileName);
    writeFileSync(localPath, buffer);

    log("info", `Downloaded document: ${fileName} (${buffer.length} bytes)`);

    return {
      filePath: localPath,
      fileName,
      mimeType: doc.mime_type || "application/octet-stream",
      sizeBytes: buffer.length,
    };
  } catch (err) {
    log("error", "Document download error", err);
    return null;
  }
}

function uniqueName(name: string): string {
  const target = join(FILES_DIR, name);
  if (!existsSync(target)) return name;
  const ext = extname(name);
  const stem = name.slice(0, name.length - ext.length);
  const ts = Date.now();
  return `${stem}_${ts}${ext}`;
}
