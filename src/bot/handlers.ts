import { type Bot, type Context } from "grammy";
import { sendMessageStreaming } from "../agent/agent.js";
import {
  clearSession,
  getSessionId,
  getAllSessions,
  hasBackgroundJobs,
  addBackgroundJob,
  removeBackgroundJob,
  isJobRelevant,
  incrementGeneration,
  clearAllBackgroundJobs,
} from "../agent/sessions.js";
import { countRows } from "../memory/lancedb.js";
import { markdownToTelegramHtml } from "./format.js";
import { transcribeVoice } from "./voice.js";
import { downloadPhoto } from "./photo.js";
import { downloadDocument } from "./document.js";
import { log, getConfig } from "../config.js";
import type { ImageAttachment } from "../agent/agent.js";

let _bot: Bot | null = null;

export function setBot(bot: Bot): void {
  _bot = bot;
}

// ── Per-chat message queue with generation-based cancellation ──

const chatQueues = new Map<number, Array<() => Promise<void>>>();
const queueGenerations = new Map<number, number>();
const MAX_QUEUE_SIZE = 5;

function enqueue(chatId: number, task: () => Promise<void>): boolean {
  if (!chatQueues.has(chatId)) {
    chatQueues.set(chatId, []);
    queueGenerations.set(chatId, (queueGenerations.get(chatId) ?? 0) + 1);
  }
  const queue = chatQueues.get(chatId)!;
  if (queue.length >= MAX_QUEUE_SIZE) return false;
  queue.push(task);
  if (queue.length === 1) {
    const gen = queueGenerations.get(chatId)!;
    drainQueue(chatId, gen).catch((err) => {
      log("error", `Queue drain failed for chat ${chatId}`, err);
    });
  }
  return true;
}

async function drainQueue(chatId: number, generation: number): Promise<void> {
  const queue = chatQueues.get(chatId);
  while (queue && queue.length > 0) {
    if ((queueGenerations.get(chatId) ?? 0) !== generation) return;
    const task = queue[0];
    try {
      await task();
    } catch (err) {
      log("error", `Queue task failed for chat ${chatId}`, err);
    }
    queue.shift();
  }
  chatQueues.delete(chatId);
  queueGenerations.delete(chatId);
}

// ── Per-chat outbound send lock ──

const sendLocks = new Map<number, Promise<void>>();

async function sendFormattedResponse(ctx: Context, raw: string): Promise<void> {
  const chatId = ctx.chat!.id;
  const prev = sendLocks.get(chatId) ?? Promise.resolve();
  const current = prev.then(async () => {
    const html = markdownToTelegramHtml(raw);
    if (html.length <= 4096) {
      await ctx.reply(html, { parse_mode: "HTML" }).catch(async () => {
        await ctx.reply(raw).catch((e) => {
          log("error", `Failed to send message to chat ${chatId}`, e);
        });
      });
    } else {
      const chunks = splitMessage(raw, 3500);
      for (const chunk of chunks) {
        const chunkHtml = markdownToTelegramHtml(chunk);
        await ctx.reply(chunkHtml, { parse_mode: "HTML" }).catch(async () => {
          await ctx.reply(chunk).catch((e) => {
            log("error", `Failed to send chunk to chat ${chatId}`, e);
          });
        });
      }
    }
  });
  sendLocks.set(chatId, current.catch(() => {}));
  await current;
}

// ── Command handlers ──

export async function handleStart(ctx: Context): Promise<void> {
  await ctx.reply(
    "hey i'm your claude-powered assistant.\n\n" +
      "send me any message and i'll process it through claude code.\n\n" +
      "commands:\n" +
      "/new — start a fresh conversation\n" +
      "/status — check system status\n" +
      "/memory — view memory stats",
  );
}

export async function handleNew(ctx: Context): Promise<void> {
  const chatId = ctx.chat?.id;
  if (!chatId) return;

  queueGenerations.set(chatId, (queueGenerations.get(chatId) ?? 0) + 1);
  chatQueues.delete(chatId);
  incrementGeneration(chatId);
  clearAllBackgroundJobs(chatId);
  clearSession(chatId);

  await ctx.reply("fresh conversation started. previous context cleared.");
}

export async function handleStatus(ctx: Context): Promise<void> {
  const chatId = ctx.chat?.id;
  if (!chatId) return;

  const sessionId = getSessionId(chatId);
  const allSessions = getAllSessions();
  const hasBg = hasBackgroundJobs(chatId);

  let episodeCount = 0;
  let factCount = 0;
  let goalCount = 0;
  let reflectionCount = 0;

  try {
    episodeCount = await countRows("episodes");
    factCount = await countRows("facts");
    goalCount = await countRows("goals");
    reflectionCount = await countRows("reflections");
  } catch {}

  const status = [
    "system status",
    "",
    `session: ${sessionId ? `active (${sessionId.slice(0, 8)}...)` : "none"}`,
    `total sessions: ${allSessions.length}`,
    `background agents: ${hasBg ? "running" : "none"}`,
    "",
    "memory:",
    `  episodes: ${episodeCount}`,
    `  facts: ${factCount}`,
    `  goals: ${goalCount}`,
    `  reflections: ${reflectionCount}`,
  ].join("\n");

  await ctx.reply(status);
}

export async function handleMemory(ctx: Context): Promise<void> {
  const chatId = ctx.chat?.id;
  if (!chatId) return;

  let factCount = 0;
  let goalCount = 0;
  try {
    factCount = await countRows("facts");
    goalCount = await countRows("goals");
  } catch {}

  await ctx.reply(
    `memory\n\nstored facts: ${factCount}\ngoals/decisions: ${goalCount}\n\nask me to search my memory for specific topics.`,
  );
}

// ── Message handlers ──

export async function handleMessage(ctx: Context): Promise<void> {
  const chatId = ctx.chat?.id;
  const text = ctx.message?.text;
  if (!chatId || !text) return;

  const queued = enqueue(chatId, async () => {
    await ctx.replyWithChatAction("typing").catch(() => {});
    await processQuery(ctx, chatId, text);
  });
  if (!queued) {
    await ctx.reply("too many messages queued — hold on a sec.");
  }
}

export async function handleVoice(ctx: Context): Promise<void> {
  const chatId = ctx.chat?.id;
  if (!chatId) return;

  const queued = enqueue(chatId, async () => {
    await ctx.replyWithChatAction("typing").catch(() => {});
    try {
      const text = await transcribeVoice(ctx);
      if (!text) {
        const cfg = getConfig();
        if (!cfg.SPARK_WHISPER_URL) {
          await ctx.reply("voice transcription isn't set up on this instance.");
        } else {
          await ctx.reply("couldn't transcribe that voice message.");
        }
        return;
      }
      await processQuery(ctx, chatId, text);
    } catch (err) {
      log("error", `Voice handling failed for chat ${chatId}`, err);
      await ctx.reply("something went wrong with the voice message.");
    }
  });
  if (!queued) {
    await ctx.reply("too many messages queued — hold on a sec.");
  }
}

export async function handlePhoto(ctx: Context): Promise<void> {
  const chatId = ctx.chat?.id;
  if (!chatId) return;

  const queued = enqueue(chatId, async () => {
    await ctx.replyWithChatAction("typing").catch(() => {});
    try {
      const image = await downloadPhoto(ctx);
      if (!image) {
        await ctx.reply("couldn't download that image");
        return;
      }
      const caption = ctx.message?.caption || "";
      await processQuery(ctx, chatId, caption, [image]);
    } catch (err) {
      log("error", `Photo handling failed for chat ${chatId}`, err);
      await ctx.reply("something went wrong with the image.");
    }
  });
  if (!queued) {
    await ctx.reply("too many messages queued — hold on a sec.");
  }
}

export async function handleDocument(ctx: Context): Promise<void> {
  const chatId = ctx.chat?.id;
  if (!chatId) return;

  const queued = enqueue(chatId, async () => {
    await ctx.replyWithChatAction("typing").catch(() => {});
    try {
      const doc = await downloadDocument(ctx);
      if (!doc) {
        await ctx.reply("couldn't download that file");
        return;
      }
      const caption = ctx.message?.caption || "";
      const prompt = `[File received: ${doc.fileName} (${doc.mimeType}, ${doc.sizeBytes} bytes), saved to ${doc.filePath}]\n\n${caption}`.trim();
      await processQuery(ctx, chatId, prompt);
    } catch (err) {
      log("error", `Document handling failed for chat ${chatId}`, err);
      await ctx.reply("something went wrong with the file.");
    }
  });
  if (!queued) {
    await ctx.reply("too many messages queued — hold on a sec.");
  }
}

// ── Core query processor ──

async function processQuery(
  ctx: Context,
  chatId: number,
  text: string,
  images?: ImageAttachment[],
): Promise<void> {
  const typingInterval = setInterval(async () => {
    try {
      await ctx.replyWithChatAction("typing");
    } catch {}
  }, 4000);

  try {
    let backgroundTaskId: string | null = null;

    const { backgroundPromise, sessionId } = await sendMessageStreaming(
      chatId,
      text,
      async (responseText) => {
        await sendFormattedResponse(ctx, responseText);
      },
      (taskId) => {
        backgroundTaskId = taskId;
        clearInterval(typingInterval);
      },
      _bot ?? undefined,
      images,
    );

    if (backgroundPromise && backgroundTaskId && sessionId) {
      clearInterval(typingInterval);
      addBackgroundJob(chatId, backgroundTaskId, sessionId);

      const taskId = backgroundTaskId;
      backgroundPromise
        .then(async (followUpText) => {
          try {
            if (!isJobRelevant(chatId, taskId)) {
              log("info", `Background job ${taskId} stale for chat ${chatId}, discarding`);
              removeBackgroundJob(chatId, taskId);
              return;
            }
            removeBackgroundJob(chatId, taskId);
            if (followUpText.trim()) {
              await sendFormattedResponse(ctx, followUpText);
            }
          } catch (err) {
            log("error", `Background job result delivery failed for chat ${chatId}`, err);
          }
        })
        .catch(async (err) => {
          log("error", `Background job ${taskId} failed for chat ${chatId}`, err);
          const stale = !isJobRelevant(chatId, taskId);
          removeBackgroundJob(chatId, taskId);
          if (!stale) {
            await ctx.reply("background agent errored out").catch(() => {});
          }
        });

      return; // Release the queue
    }
  } catch (err) {
    log("error", `Message handling failed for chat ${chatId}`, err);
    await ctx.reply(
      "something went wrong processing your message. try again or /new",
    );
  } finally {
    clearInterval(typingInterval);
  }
}

// ── Utilities ──

function splitMessage(text: string, maxLength: number): string[] {
  if (text.length <= maxLength) return [text];

  const chunks: string[] = [];
  let remaining = text;

  while (remaining.length > 0) {
    if (remaining.length <= maxLength) {
      chunks.push(remaining);
      break;
    }

    let splitIdx = remaining.lastIndexOf("\n\n", maxLength);
    if (splitIdx === -1 || splitIdx < maxLength * 0.3) {
      splitIdx = remaining.lastIndexOf("\n", maxLength);
    }
    if (splitIdx === -1 || splitIdx < maxLength * 0.3) {
      splitIdx = remaining.lastIndexOf(" ", maxLength);
    }
    if (splitIdx === -1) {
      splitIdx = maxLength;
    }

    chunks.push(remaining.slice(0, splitIdx));
    remaining = remaining.slice(splitIdx).trimStart();
  }

  return chunks;
}
