import { query } from "@anthropic-ai/claude-agent-sdk";
import { type Bot } from "grammy";
import { createProjectMcpServer } from "../memory/project-tools.js";
import { getSessionId, setSessionId, getMessageCount, getGeneration } from "./sessions.js";
import { buildHooksForChat } from "./hooks.js";
import { getConfig, log } from "../config.js";

const projectServer = createProjectMcpServer();

// Input size guards
const MAX_MESSAGE_LENGTH = 8_000;   // chars
const MAX_IMAGE_SIZE_BYTES = 5 * 1024 * 1024; // 5 MB per image
const MAX_IMAGES = 5;
const ALLOWED_MEDIA_TYPES = new Set(["image/jpeg", "image/png", "image/gif", "image/webp"]);

// Per-turn timeout: generous to allow slow tool calls, but prevents forever-hung network.
// Note: when the timeout fires, the underlying SDK stream cannot be cancelled (no AbortSignal
// support in the current SDK). The stream will drain in the background, but the bot moves on.
const TURN_TIMEOUT_MS = 10 * 60 * 1000; // 10 minutes

async function nextWithTimeout<T>(
  iter: AsyncIterator<T>,
  label: string,
): Promise<IteratorResult<T>> {
  let timeoutId: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timeoutId = setTimeout(
      () => reject(new Error(`Stream stalled: ${label} timed out after ${TURN_TIMEOUT_MS / 1000}s`)),
      TURN_TIMEOUT_MS,
    );
  });
  try {
    return await Promise.race([iter.next(), timeout]);
  } finally {
    clearTimeout(timeoutId);
  }
}

export interface ImageAttachment {
  base64: string;
  mediaType: "image/jpeg" | "image/png" | "image/gif" | "image/webp";
}

export interface StreamingResult {
  backgroundPromise: Promise<string[]> | null;
  sessionId: string | null;
}

export async function sendMessageStreaming(
  chatId: number,
  userMessage: string,
  onText: (text: string) => Promise<void>,
  onBackgroundStarted: (taskId: string) => void,
  bot?: Bot,
  images?: ImageAttachment[],
): Promise<StreamingResult> {
  // ── Input validation ──
  if (userMessage.length > MAX_MESSAGE_LENGTH) {
    throw new Error(`Message too long (${userMessage.length} chars, max ${MAX_MESSAGE_LENGTH})`);
  }
  if (images) {
    if (images.length > MAX_IMAGES) {
      throw new Error(`Too many images (${images.length}, max ${MAX_IMAGES})`);
    }
    for (const img of images) {
      if (!ALLOWED_MEDIA_TYPES.has(img.mediaType)) {
        throw new Error(`Unsupported image type: ${img.mediaType}`);
      }
      // Exact base64 decode size: each 4-char group = 3 bytes, minus padding chars
      const padding = (img.base64.match(/=+$/) ?? [""])[0].length;
      const exactBytes = Math.floor(img.base64.length * 3 / 4) - padding;
      if (exactBytes > MAX_IMAGE_SIZE_BYTES) {
        throw new Error(`Image too large (${Math.round(exactBytes / 1024)}KB, max ${MAX_IMAGE_SIZE_BYTES / 1024}KB)`);
      }
    }
  }

  const cfg = getConfig();
  const start = performance.now();

  // Concurrency note: handlers.ts serialises per-chat requests via chatQueues,
  // so getSessionId/setSessionId are never called concurrently for the same chatId
  // from the foreground path. The background IIFE uses generation/messageCount
  // snapshots (bgGeneration, bgMessageCount) to detect stale sessions and avoid
  // clobbering state advanced by a subsequent foreground message.
  const existingSessionId = getSessionId(chatId);
  const startGeneration = getGeneration(chatId);

  const options: Record<string, unknown> = {
    maxTurns: cfg.AGENT_MAX_TURNS,
    permissionMode: cfg.AGENT_PERMISSION_MODE,
    cwd: cfg.AGENT_CWD,
    settingSources: ["project"],
    ...(bot ? { hooks: buildHooksForChat(bot, chatId) } : {}),
    mcpServers: {
      // MEMORY_MCP_COMMAND/SCRIPT come from env vars validated by config.ts.
      // The SDK invokes them via spawn (not shell), so metacharacter injection
      // is not possible as long as the SDK does not use shell:true.
      ...(cfg.MEMORY_MCP_COMMAND && cfg.MEMORY_MCP_SCRIPT ? {
        memory: { command: cfg.MEMORY_MCP_COMMAND, args: [cfg.MEMORY_MCP_SCRIPT] },
      } : {}),
      ...(cfg.SPARK_QMD_MCP_URL ? {
        qmd: { type: "sse", url: cfg.SPARK_QMD_MCP_URL },
      } : {}),
      projects: projectServer,
    },
    allowedTools: [
      ...(cfg.MEMORY_MCP_COMMAND ? ["mcp__memory__*"] : []),
      ...(cfg.SPARK_QMD_MCP_URL ? ["mcp__qmd__*"] : []),
      "mcp__projects__project_list",
      "mcp__projects__project_work",
      "mcp__projects__project_create",
      "Read", "Glob", "Grep",
      // Bash is intentionally exposed: this is a single-owner personal bot.
      // The only user is also the system owner, so arbitrary shell execution
      // is a feature, not a vulnerability.
      "Bash",
    ],
  };

  if (existingSessionId) {
    options.resume = existingSessionId;
    log("debug", `Resuming session ${existingSessionId} for chat ${chatId}`);
  }

  // Build prompt
  let prompt: any = userMessage;
  if (images && images.length > 0) {
    const content: any[] = images.map((img) => ({
      type: "image",
      source: { type: "base64", media_type: img.mediaType, data: img.base64 },
    }));
    content.push({ type: "text", text: userMessage || "What's in this image?" });
    const msg = {
      type: "user",
      message: { role: "user", content },
      parent_tool_use_id: null,
    };
    prompt = (async function* () { yield msg; })();
  }

  let sessionId: string | null = null;

  // Raw iterator protocol: we intentionally avoid for-await-of because it calls
  // .return() on early exit, which would close the stream before the background
  // IIFE can continue reading from the same iterator. The iterator is a single
  // consumer — foreground reads until task_started, then hands off to background.
  const stream = query({ prompt, options: options as any });
  const iterator = stream[Symbol.asyncIterator]();

  // Once-guard: ensures iterator.return() is called exactly once across all paths
  // (foreground catch, foreground normal exit, background finally).
  let iteratorClosed = false;
  function closeIterator() {
    if (!iteratorClosed) {
      iteratorClosed = true;
      iterator.return?.();
    }
  }

  // Tracks whether the iterator was handed off to the background IIFE.
  // The outer finally skips cleanup when true — background owns the iterator.
  let handedOffToBackground = false;

  try {
    let done = false;
    while (!done) {
      const next = await nextWithTimeout(iterator, "foreground");
      if (next.done) { done = true; break; }
      const message = next.value;

      // Capture session ID from init (hold in local var, don't persist yet)
      if (message.type === "system" && message.subtype === "init") {
        sessionId = message.session_id;
      }

      // Stream assistant text immediately
      if (message.type === "assistant" && Array.isArray(message.message?.content)) {
        for (const block of message.message.content) {
          if (block.type === "text" && block.text) {
            try {
              await onText(block.text);
            } catch (cbErr) {
              log("error", `onText callback failed for chat ${chatId}`, cbErr);
            }
          }
        }
      }

      // Detect background agent — task_started is our early signal
      if (message.type === "system" && message.subtype === "task_started") {
        const taskId = String((message as Record<string, unknown>).task_id ?? "");
        try {
          onBackgroundStarted(taskId);
        } catch (cbErr) {
          log("error", `onBackgroundStarted callback failed for chat ${chatId}`, cbErr);
          closeIterator();
          throw cbErr;
        }

        // Hand the iterator to a background promise — it continues from here
        const bgSessionId = sessionId;

        // Persist the foreground session so the next message can resume continuity
        // even while the background job is still running.
        if (bgSessionId) {
          setSessionId(chatId, bgSessionId);
        }
        // Snapshot message count and generation — the background closure uses these
        // to avoid clobbering a session advanced by subsequent foreground messages.
        const bgMessageCount = getMessageCount(chatId);
        const bgGeneration = getGeneration(chatId);

        const backgroundPromise = (async (): Promise<string[]> => {
          // Each assistant turn becomes a separate Telegram message so that
          // "working on it..." and the actual result don't collapse into one.
          // Use a local sessionId to avoid mutating the outer variable from inside
          // the closure, which is a code smell even though the generation gate
          // prevents it from causing actual harm.
          let localSessionId: string | null = bgSessionId;
          const messages: string[] = [];
          let currentText = "";
          try {
            let bgDone = false;
            while (!bgDone) {
              const bgNext = await nextWithTimeout(iterator, "background");
              if (bgNext.done) { bgDone = true; break; }
              const bgMessage = bgNext.value;

              if (bgMessage.type === "assistant" && Array.isArray(bgMessage.message?.content)) {
                // Flush previous turn before starting a new one
                if (currentText) { messages.push(currentText); currentText = ""; }
                for (const block of bgMessage.message.content) {
                  if (block.type === "text" && block.text) {
                    currentText += (currentText ? "\n\n" : "") + block.text;
                  }
                }
              }
              // task_notification gets its own message
              if (bgMessage.type === "system" && bgMessage.subtype === "task_notification") {
                const summary = (bgMessage as Record<string, unknown>).summary as string | undefined;
                if (summary) {
                  if (currentText) { messages.push(currentText); currentText = ""; }
                  messages.push(summary);
                }
              }
              if (bgMessage.type === "result") {
                if (bgMessage.session_id) {
                  localSessionId = bgMessage.session_id;
                }
              }
            }
          } catch (err) {
            log("error", `Background stream failed for chat ${chatId}`, err);
            throw err;
          } finally {
            closeIterator();
          }
          if (currentText) messages.push(currentText);
          const elapsed = (performance.now() - start).toFixed(0);
          const totalChars = messages.reduce((s, m) => s + m.length, 0);
          log("info", `Chat ${chatId}: background completed in ${elapsed}ms (${totalChars} chars, ${messages.length} messages)`);
          // Only persist the background session if no foreground message has
          // advanced the session since we started.
          if (localSessionId && getMessageCount(chatId) === bgMessageCount && getGeneration(chatId) === bgGeneration) {
            setSessionId(chatId, localSessionId);
          }
          return messages;
        })();

        // Attach a logging catch immediately so the promise is never unhandled
        // in the window before handlers.ts attaches its own .catch().
        // This does NOT swallow the rejection — handlers.ts's .catch() is chained
        // on the original promise reference and still fires independently.
        backgroundPromise.catch((err) => {
          log("error", `Background promise rejected for chat ${chatId} (pre-handler window)`, err);
        });

        handedOffToBackground = true;
        return { backgroundPromise, sessionId: bgSessionId };
      }

      // Capture session ID from result (foreground path)
      if (message.type === "result") {
        if (message.session_id) {
          sessionId = message.session_id;
        }
        if (message.subtype === "error_max_turns") {
          log("warn", `Chat ${chatId}: hit max turns limit`);
        }
        if (message.subtype === "error_during_execution") {
          log("error", `Chat ${chatId}: execution error`);
        }
      }
    }
  } catch (err) {
    if (!handedOffToBackground) closeIterator();
    log("error", `Query failed for chat ${chatId}`, err);
    throw err;
  } finally {
    // Normal foreground completion (no task_started): release the iterator.
    // Skipped when handed off — background IIFE owns it and calls closeIterator() itself.
    if (!handedOffToBackground) closeIterator();
  }

  // Foreground path — persist session unless /new was called while we were running
  if (sessionId && getGeneration(chatId) === startGeneration) {
    setSessionId(chatId, sessionId);
  }

  const elapsed = (performance.now() - start).toFixed(0);
  log("info", `Chat ${chatId}: response in ${elapsed}ms`);

  return { backgroundPromise: null, sessionId };
}
