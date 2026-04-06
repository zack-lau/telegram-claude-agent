import { type Bot } from "grammy";
import { isGatedTool, requestApproval } from "../bot/approvals.js";
import { log } from "../config.js";

let _counter = 0;

/**
 * Build SDK-compatible hooks config with PreToolUse gating.
 * Each call captures bot and chatId in a closure — no global mutable state.
 * Safe to call concurrently for overlapping queries on the same or different chats.
 */
export function buildHooksForChat(bot: Bot, chatId: number) {
  return {
    PreToolUse: [
      {
        hooks: [
          async (input: any, toolUseId: string | undefined) => {
            const toolName: string = input.tool_name ?? "";
            const toolInput: Record<string, unknown> = input.tool_input ?? {};

            if (!isGatedTool(toolName, toolInput)) {
              return { hookEventName: "PreToolUse" };
            }

            if (!bot) {
              log("warn", `No bot context for approval of ${toolName}, auto-denying`);
              return {
                hookEventName: "PreToolUse",
                permissionDecision: "deny" as const,
                permissionDecisionReason: "No Telegram context available for approval",
              };
            }

            const requestId = `${Date.now()}-${++_counter}`;
            log("info", `Requesting Telegram approval for ${toolName} (${requestId})`);

            const approved = await requestApproval(
              bot,
              chatId,
              requestId,
              toolName,
              toolInput,
            );

            log("info", `Approval result for ${toolName}: ${approved ? "approved" : "denied"}`);

            return {
              hookEventName: "PreToolUse",
              permissionDecision: approved ? ("allow" as const) : ("deny" as const),
              permissionDecisionReason: approved
                ? "User approved via Telegram"
                : "User denied via Telegram",
            };
          },
        ],
        timeout: 180,
      },
    ],
  };
}
