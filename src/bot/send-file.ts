import { InputFile } from "grammy";
import type { Context } from "grammy";
import * as fs from "node:fs/promises";
import { open as fsOpen } from "node:fs/promises";
import * as path from "node:path";

// ── Path validation ──

const ALLOWED_DIRECTORIES = [
  "/Users/zack/claude-agent/data/files/",
  "/Users/zack/claude-agent/projects/",
  "/Users/zack/claude-agent/workspace/",
  "/tmp/",
];

const BLOCKED_PATTERNS = [
  ".ssh",
  ".env",
  ".gnupg",
  ".aws",
  ".config/",
  "credentials",
  ".pem",
  "id_rsa",
  "id_ed25519",
  "private",
  "secret",
  ".key",
];

const MAX_PHOTO_SIZE = 10 * 1024 * 1024; // 10MB
const MAX_DOCUMENT_SIZE = 50 * 1024 * 1024; // 50MB
const MAX_FILE_MARKERS = 10;

export async function validateFilePath(
  filePath: string,
): Promise<{ valid: boolean; error?: string; realPath?: string }> {
  // Require absolute path
  if (!path.isAbsolute(filePath)) {
    return { valid: false, error: "relative paths not allowed" };
  }

  // Block dot-directories anywhere in the path (e.g., .git/, .claude/, .mcp.json)
  const pathSegments = filePath.split("/");
  for (const segment of pathSegments) {
    if (segment.startsWith(".") && segment.length > 1) {
      return { valid: false, error: `path contains dot-file/directory: ${segment}` };
    }
  }

  // Check for blocked sensitive patterns
  const lowerPath = filePath.toLowerCase();
  for (const pattern of BLOCKED_PATTERNS) {
    if (lowerPath.includes(pattern)) {
      return { valid: false, error: `path contains blocked pattern: ${pattern}` };
    }
  }

  // Validate file is a regular file (not symlink, directory, device, etc.)
  let lstats: Awaited<ReturnType<typeof fs.lstat>>;
  try {
    lstats = await fs.lstat(filePath);
  } catch {
    return { valid: false, error: "file does not exist" };
  }

  if (!lstats.isFile()) {
    return { valid: false, error: "path is not a regular file" };
  }

  // Resolve symlinks and check real path against allowlist
  let realPath: string;
  try {
    realPath = await fs.realpath(filePath);
  } catch {
    return { valid: false, error: "cannot resolve file path" };
  }

  // Check real path against blocked patterns too
  const lowerReal = realPath.toLowerCase();
  for (const pattern of BLOCKED_PATTERNS) {
    if (lowerReal.includes(pattern)) {
      return { valid: false, error: `resolved path contains blocked pattern: ${pattern}` };
    }
  }

  // Block dot-directories/dot-files in resolved path too (prevents parent symlink bypass)
  const realSegments = realPath.split("/");
  for (const segment of realSegments) {
    if (segment.startsWith(".") && segment.length > 1) {
      return { valid: false, error: `resolved path contains dot-file/directory: ${segment}` };
    }
  }

  // Check allowlist with strict directory boundary
  const allowed = ALLOWED_DIRECTORIES.some(
    (dir) => realPath.startsWith(dir) && (realPath.length === dir.length || realPath[dir.length - 1] === "/" || realPath[dir.length] === "/")
  );
  if (!allowed) {
    return { valid: false, error: "file is outside allowed directories" };
  }

  return { valid: true, realPath };
}

/** Check file size against limits. Returns error string or null if OK. */
async function checkFileSize(
  filePath: string,
  isPhoto: boolean,
): Promise<string | null> {
  try {
    const stats = await fs.stat(filePath);
    const limit = isPhoto ? MAX_PHOTO_SIZE : MAX_DOCUMENT_SIZE;
    const limitLabel = isPhoto ? "10MB (photo)" : "50MB (document)";
    if (stats.size > limit) {
      return `file exceeds ${limitLabel} size limit`;
    }
    return null;
  } catch {
    return "cannot stat file";
  }
}

/** Escape HTML special characters in a string */
function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Send an image (PNG, JPG, etc.) to a chat */
export async function sendPhoto(
  ctx: Context,
  filePath: string,
  caption?: string,
): Promise<void> {
  const file = new InputFile(filePath);
  await ctx.replyWithPhoto(file, {
    caption: caption ? escapeHtml(caption) : undefined,
    parse_mode: "HTML",
  });
}

/** Send any file/document to a chat */
export async function sendDocument(
  ctx: Context,
  filePath: string,
  caption?: string,
): Promise<void> {
  const file = new InputFile(filePath);
  await ctx.replyWithDocument(file, {
    caption: caption ? escapeHtml(caption) : undefined,
    parse_mode: "HTML",
  });
}

/** Detect if a path is an image by extension (only types Telegram handles well as photos) */
export function isImageFile(filePath: string): boolean {
  const ext = path.extname(filePath).toLowerCase().slice(1);
  return ["png", "jpg", "jpeg", "webp"].includes(ext);
}

/** Smart send: validate path, read file atomically, then send buffer to Telegram.
 *  Opens the file immediately after validation so the checked content is the sent content (no TOCTOU gap). */
export async function sendFile(
  ctx: Context,
  filePath: string,
  caption?: string,
): Promise<void> {
  // Validate path security — returns the canonical realPath
  const validation = await validateFilePath(filePath);
  if (!validation.valid || !validation.realPath) {
    throw new Error(`File validation failed: ${validation.error}`);
  }

  const validatedPath = validation.realPath;
  const isPhoto = isImageFile(validatedPath);

  // Open file immediately after validation — captures the exact file that was checked
  const handle = await fsOpen(validatedPath, "r");
  try {
    // Verify inode is still a regular file via the open handle (not the path)
    const stats = await handle.stat();
    if (!stats.isFile()) {
      throw new Error("file changed to non-regular file after validation");
    }

    // Check size via the open handle
    const limit = isPhoto ? MAX_PHOTO_SIZE : MAX_DOCUMENT_SIZE;
    const limitLabel = isPhoto ? "10MB (photo)" : "50MB (document)";
    if (stats.size > limit) {
      throw new Error(`file exceeds ${limitLabel} size limit`);
    }

    // Read the validated file content into memory via the open handle
    const buffer = await handle.readFile();

    // Send the buffer — InputFile accepts Buffer, so we never re-open the path
    const fileName = path.basename(validatedPath);
    const file = new InputFile(buffer, fileName);
    const opts = {
      caption: caption ? escapeHtml(caption) : undefined,
      parse_mode: "HTML" as const,
    };

    if (isPhoto) {
      await ctx.replyWithPhoto(file, opts);
    } else {
      await ctx.replyWithDocument(file, opts);
    }
  } finally {
    await handle.close();
  }
}

/** Extract [SEND_FILE:/path] markers from text, return cleaned text and file paths */
export function extractFileMarkers(text: string): {
  cleanedText: string;
  filePaths: string[];
} {
  const pattern = /\[SEND_FILE:(\/[^\]\n]+)\]/g;
  const filePaths: string[] = [];
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text)) !== null) {
    if (filePaths.length >= MAX_FILE_MARKERS) break;
    filePaths.push(match[1]);
  }

  const cleanedText = text.replace(pattern, "").replace(/\n{3,}/g, "\n\n").trim();

  return { cleanedText, filePaths };
}
