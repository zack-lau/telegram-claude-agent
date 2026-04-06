import { query } from "@anthropic-ai/claude-agent-sdk";
import { type Bot } from "grammy";
import { createProjectMcpServer } from "../memory/project-tools.js";
import { getSessionId, setSessionId } from "./sessions.js";
import { buildHooksForChat } from "./hooks.js";
import { getConfig, log } from "../config.js";

const projectServer = createProjectMcpServer();

export interface ImageAttachment {
  base64: string;
  mediaType: "image/jpeg" | "image/png" | "image/gif" | "image/webp";
}

export interface StreamingResult {
  backgroundPromise: Promise<string> | null;
  sessionId: string | null;
}

export async function sendMessageStreaming(
  chatId: number,
  userMessage: string,
  onText: (text: string) => Promise<void>,
  onBackgroundStarted: (taskId: string) => void,
  bot?: Bot,
  images?: ImageAttachment[],
  skipResume?: boolean,
): Promise<StreamingResult> {
  const cfg = getConfig();
  const start = performance.now();

  const existingSessionId = getSessionId(chatId);

  const options: Record<string, unknown> = {
    maxTurns: cfg.AGENT_MAX_TURNS,
    permissionMode: cfg.AGENT_PERMISSION_MODE,
    cwd: cfg.AGENT_CWD,
    settingSources: ["project"],
    ...(bot ? { hooks: buildHooksForChat(bot, chatId) } : {}),
    mcpServers: {
      memory: { type: "sse", url: cfg.SPARK_MEMORY_MCP_URL },
      qmd: { type: "sse", url: cfg.SPARK_QMD_MCP_URL },
      projects: projectServer,
    },
    allowedTools: [
      "mcp__memory__*",
      "mcp__qmd__*",
      "mcp__projects__project_list",
      "mcp__projects__project_work",
      "mcp__projects__project_create",
      "Read", "Glob", "Grep", "Bash",
      "Agent",
    ],
  };

  if (!skipResume && existingSessionId) {
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

  try {
    const stream = query({ prompt, options: options as any });
    // Use raw iterator protocol so we never call .return() on the generator.
    // for-await-of calls .return() on early exit, which closes the stream
    // and prevents the background IIFE from continuing iteration.
    const iterator = stream[Symbol.asyncIterator]();

    let done = false;
    while (!done) {
      const next = await iterator.next();
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
            await onText(block.text);
          }
        }
      }

      // Detect background agent — task_started is our early signal
      if (message.type === "system" && message.subtype === "task_started") {
        const taskId = (message as any).task_id as string;
        onBackgroundStarted(taskId);

        // Hand the iterator to a background promise — it continues from here
        const bgSessionId = sessionId;
        const backgroundPromise = (async (): Promise<string> => {
          let followUpText = "";
          try {
            let bgDone = false;
            while (!bgDone) {
              const bgNext = await iterator.next();
              if (bgNext.done) { bgDone = true; break; }
              const bgMessage = bgNext.value;

              if (bgMessage.type === "assistant" && Array.isArray(bgMessage.message?.content)) {
                for (const block of bgMessage.message.content) {
                  if (block.type === "text" && block.text) {
                    followUpText += block.text;
                  }
                }
              }
              // Capture task_notification summary as follow-up text
              if (bgMessage.type === "system" && bgMessage.subtype === "task_notification") {
                const summary = (bgMessage as any).summary as string | undefined;
                if (summary) {
                  followUpText += summary;
                }
              }
              if (bgMessage.type === "result") {
                if (bgMessage.session_id) {
                  sessionId = bgMessage.session_id;
                }
              }
            }
          } catch (err) {
            log("error", `Background stream failed for chat ${chatId}`, err);
            throw err;
          }
          const elapsed = (performance.now() - start).toFixed(0);
          log("info", `Chat ${chatId}: background completed in ${elapsed}ms (${followUpText.length} chars)`);
          // Do NOT persist session — background sessions are tracked separately
          return followUpText;
        })();

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
    log("error", `Query failed for chat ${chatId}`, err);
    throw err;
  }

  // Foreground path — persist session
  if (sessionId) {
    setSessionId(chatId, sessionId);
  }

  const elapsed = (performance.now() - start).toFixed(0);
  log("info", `Chat ${chatId}: response in ${elapsed}ms`);

  return { backgroundPromise: null, sessionId };
}
