import { type Bot, InlineKeyboard } from "grammy";
import { log } from "../config.js";

/**
 * Tool approval requests that need user confirmation via Telegram.
 *
 * Flow:
 *   1. PreToolUse hook fires → matches a gated tool
 *   2. sendApprovalRequest() sends inline keyboard to Telegram
 *   3. User taps Approve / Deny
 *   4. callback_query handler resolves the pending promise
 *   5. Hook returns allow / deny to the SDK
 */

interface PendingApproval {
  resolve: (approved: boolean) => void;
  chatId: number;
  toolName: string;
  createdAt: number;
}

const pending = new Map<string, PendingApproval>();

// Auto-deny after 2 minutes
const APPROVAL_TIMEOUT_MS = 120_000;

/**
 * Tools that require Telegram approval even in bypassPermissions mode.
 * Everything else is auto-allowed by the SDK.
 *
 * Patterns are tested with startsWith, so "Bash(rm" matches "Bash(rm -rf /)".
 */
const GATED_PATTERNS: RegExp[] = [
  // Destructive shell commands
  /^Bash\(rm\b/,
  /^Bash\(git\s+(push|reset|rebase|checkout\s+--)/,
  /^Bash\(docker\s+(rm|rmi|system\s+prune)/,
  /^Bash\(kill\b/,
  /^Bash\(pkill\b/,
  /^Bash\(systemctl\s+(stop|disable|restart)/,
  /^Bash\(launchctl\s+(unload|remove)/,
  // Sending messages to external services
  /^mcp__claude_ai_Gmail__gmail_create_draft/,
];

/**
 * Check if a tool + input combination should be gated.
 */
export function isGatedTool(toolName: string, input: Record<string, unknown>): boolean {
  // Build a signature like "Bash(rm -rf /tmp)" for matching
  let signature = toolName;
  if (toolName === "Bash" && typeof input.command === "string") {
    signature = `Bash(${input.command})`;
  }

  return GATED_PATTERNS.some((p) => p.test(signature));
}

/**
 * Format tool info for the Telegram approval message.
 */
function formatToolDescription(toolName: string, input: Record<string, unknown>): string {
  if (toolName === "Bash" && typeof input.command === "string") {
    const cmd = input.command.length > 200
      ? input.command.slice(0, 200) + "..."
      : input.command;
    return `\`${cmd}\``;
  }

  // MCP tools — show tool name + relevant input keys
  const summary = Object.entries(input)
    .filter(([, v]) => typeof v === "string" || typeof v === "number")
    .map(([k, v]) => {
      const val = String(v);
      return `${k}: ${val.length > 80 ? val.slice(0, 80) + "..." : val}`;
    })
    .join("\n");

  return `**${toolName}**\n${summary || "(no input)"}`;
}

/**
 * Send an approval request to Telegram and wait for the user's response.
 * Returns true if approved, false if denied or timed out.
 */
export async function requestApproval(
  bot: Bot,
  chatId: number,
  requestId: string,
  toolName: string,
  input: Record<string, unknown>,
): Promise<boolean> {
  const description = formatToolDescription(toolName, input);

  const keyboard = new InlineKeyboard()
    .text("✅ Approve", `approve:${requestId}`)
    .text("❌ Deny", `deny:${requestId}`);

  try {
    await bot.api.sendMessage(
      chatId,
      `🔐 <b>Approval needed</b>\n\n${description}`,
      {
        parse_mode: "HTML",
        reply_markup: keyboard,
      },
    );
  } catch (err) {
    log("error", `Failed to send approval request to chat ${chatId}`, err);
    return false;
  }

  return new Promise<boolean>((resolve) => {
    const timeout = setTimeout(() => {
      if (pending.has(requestId)) {
        pending.delete(requestId);
        log("info", `Approval timed out for ${toolName} (${requestId})`);
        bot.api
          .sendMessage(chatId, `⏰ Approval timed out — auto-denied.`)
          .catch(() => {});
        resolve(false);
      }
    }, APPROVAL_TIMEOUT_MS);

    pending.set(requestId, {
      resolve: (approved) => {
        clearTimeout(timeout);
        pending.delete(requestId);
        resolve(approved);
      },
      chatId,
      toolName,
      createdAt: Date.now(),
    });
  });
}

/**
 * Handle callback_query from Telegram inline keyboard buttons.
 * Call this from the bot setup.
 */
export function registerApprovalHandler(bot: Bot): void {
  bot.on("callback_query:data", async (ctx) => {
    const data = ctx.callbackQuery.data;
    if (!data) return;

    const [action, requestId] = data.split(":");
    if (!requestId || (action !== "approve" && action !== "deny")) return;

    const entry = pending.get(requestId);
    if (!entry) {
      await ctx.answerCallbackQuery({ text: "Request expired or already handled." });
      return;
    }

    const approved = action === "approve";
    entry.resolve(approved);

    await ctx.answerCallbackQuery({
      text: approved ? "Approved ✅" : "Denied ❌",
    });

    // Update the message to show the decision
    try {
      await ctx.editMessageText(
        `${approved ? "✅ Approved" : "❌ Denied"}: ${entry.toolName}`,
      );
    } catch {
      // Message may have been deleted
    }
  });
}
