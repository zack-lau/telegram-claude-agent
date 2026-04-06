import { describe, test, expect, mock } from "bun:test";
import { buildHooksForChat } from "../src/agent/hooks.js";

function makeMockBot() {
  return {
    api: {
      sendMessage: mock(() => Promise.resolve()),
    },
  } as any;
}

describe("buildHooksForChat", () => {
  test("returns hooks object with PreToolUse array", () => {
    const bot = makeMockBot();
    const hooks = buildHooksForChat(bot, 123);
    expect(hooks.PreToolUse).toBeDefined();
    expect(hooks.PreToolUse).toHaveLength(1);
    expect(hooks.PreToolUse[0].hooks).toHaveLength(1);
    expect(hooks.PreToolUse[0].timeout).toBe(180);
  });

  test("auto-allows non-gated tools", async () => {
    const bot = makeMockBot();
    const hooks = buildHooksForChat(bot, 123);
    const hookFn = hooks.PreToolUse[0].hooks[0];

    const result = await hookFn(
      { tool_name: "Read", tool_input: { file_path: "/tmp/foo" } },
      "tool-use-id-1",
    );

    expect(result.hookEventName).toBe("PreToolUse");
    expect(result.permissionDecision).toBeUndefined();
  });

  test("captures chatId per closure — two hooks route independently", () => {
    const bot = makeMockBot();
    const hooks1 = buildHooksForChat(bot, 111);
    const hooks2 = buildHooksForChat(bot, 222);

    expect(hooks1).not.toBe(hooks2);
    expect(hooks1.PreToolUse[0].hooks[0]).not.toBe(hooks2.PreToolUse[0].hooks[0]);
  });
});
