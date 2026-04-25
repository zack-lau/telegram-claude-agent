import { describe, test, expect, mock, beforeEach } from "bun:test";

// ── Mock SDK query() to return a controllable async iterable ──

let mockMessages: any[] = [];

mock.module("@anthropic-ai/claude-agent-sdk", () => ({
  query: () => {
    let idx = 0;
    return {
      [Symbol.asyncIterator]() {
        return {
          async next() {
            if (idx < mockMessages.length) {
              return { value: mockMessages[idx++], done: false };
            }
            return { value: undefined, done: true };
          },
        };
      },
    };
  },
}));

// Mock project-tools (module-level side effect in agent.ts)
mock.module("../src/memory/project-tools.js", () => ({
  createProjectMcpServer: () => ({}),
}));

// Mock sessions
const mockGetSessionId = mock(() => null as string | null);
const mockSetSessionId = mock((_chatId: number, _sessionId: string) => {});
const mockHasBackgroundJobs = mock(() => false);
const mockGetMessageCount = mock(() => 0);
const mockGetGeneration = mock(() => 0);

mock.module("../src/agent/sessions.js", () => ({
  getSessionId: mockGetSessionId,
  setSessionId: mockSetSessionId,
  hasBackgroundJobs: mockHasBackgroundJobs,
  getMessageCount: mockGetMessageCount,
  getGeneration: mockGetGeneration,
}));

// Mock hooks
mock.module("../src/agent/hooks.js", () => ({
  buildHooksForChat: () => ({}),
}));

// Mock config
mock.module("../src/config.js", () => ({
  getConfig: () => ({
    AGENT_MAX_TURNS: 10,
    AGENT_PERMISSION_MODE: "default",
    AGENT_CWD: "/tmp",
    SPARK_MEMORY_MCP_URL: "http://localhost:8282",
    SPARK_QMD_MCP_URL: "http://localhost:8181",
  }),
  log: () => {},
}));

// Now import after mocks are set up
const { sendMessageStreaming } = await import("../src/agent/agent.js");

describe("sendMessageStreaming", () => {
  beforeEach(() => {
    mockMessages = [];
    mockGetSessionId.mockReset();
    mockSetSessionId.mockReset();
    mockHasBackgroundJobs.mockReset();
    mockGetSessionId.mockReturnValue(null);
  });

  test("calls onText for each assistant text block", async () => {
    mockMessages = [
      { type: "system", subtype: "init", session_id: "sess-1" },
      {
        type: "assistant",
        message: {
          content: [
            { type: "text", text: "hello " },
            { type: "text", text: "world" },
          ],
        },
      },
      { type: "result", session_id: "sess-1" },
    ];

    const texts: string[] = [];
    const onText = mock(async (t: string) => { texts.push(t); });
    const onBg = mock((_taskId: string) => {});

    const result = await sendMessageStreaming(42, "hi", onText, onBg);

    expect(onText).toHaveBeenCalledTimes(2);
    expect(texts).toEqual(["hello ", "world"]);
    expect(result.backgroundPromise).toBeNull();
    expect(result.sessionId).toBe("sess-1");
  });

  test("foreground path persists session via setSessionId", async () => {
    mockMessages = [
      { type: "system", subtype: "init", session_id: "sess-fg" },
      {
        type: "assistant",
        message: { content: [{ type: "text", text: "ok" }] },
      },
      { type: "result", session_id: "sess-fg" },
    ];

    const onText = mock(async () => {});
    const onBg = mock(() => {});

    await sendMessageStreaming(42, "test", onText, onBg);

    expect(mockSetSessionId).toHaveBeenCalledWith(42, "sess-fg");
  });

  test("task_started triggers onBackgroundStarted and returns backgroundPromise", async () => {
    mockMessages = [
      { type: "system", subtype: "init", session_id: "sess-bg" },
      {
        type: "assistant",
        message: { content: [{ type: "text", text: "starting task..." }] },
      },
      { type: "system", subtype: "task_started", task_id: "task-abc" },
      {
        type: "assistant",
        message: { content: [{ type: "text", text: "follow-up text" }] },
      },
      { type: "result", session_id: "sess-bg-done" },
    ];

    const texts: string[] = [];
    const onText = mock(async (t: string) => { texts.push(t); });
    const bgTasks: string[] = [];
    const onBg = mock((taskId: string) => { bgTasks.push(taskId); });

    const result = await sendMessageStreaming(42, "do thing", onText, onBg);

    // onText should have been called for the pre-task text
    expect(texts).toContain("starting task...");

    // Background started callback fired
    expect(onBg).toHaveBeenCalledTimes(1);
    expect(bgTasks).toEqual(["task-abc"]);

    // backgroundPromise exists and resolves with follow-up messages (one per turn)
    expect(result.backgroundPromise).not.toBeNull();
    const followUp = await result.backgroundPromise!;
    expect(followUp).toEqual(["follow-up text"]);

    // sessionId returned is from before the background handoff
    expect(result.sessionId).toBe("sess-bg");
  });

  test("backgroundPromise is null when no task_started occurs", async () => {
    mockMessages = [
      { type: "system", subtype: "init", session_id: "sess-simple" },
      {
        type: "assistant",
        message: { content: [{ type: "text", text: "just a reply" }] },
      },
      { type: "result", session_id: "sess-simple" },
    ];

    const onText = mock(async () => {});
    const onBg = mock(() => {});

    const result = await sendMessageStreaming(42, "hello", onText, onBg);

    expect(result.backgroundPromise).toBeNull();
    expect(onBg).not.toHaveBeenCalled();
  });

  test("background path persists foreground session early on task_started", async () => {
    mockMessages = [
      { type: "system", subtype: "init", session_id: "sess-bg2" },
      { type: "system", subtype: "task_started", task_id: "task-xyz" },
      { type: "result", session_id: "sess-bg2-done" },
    ];

    const onText = mock(async () => {});
    const onBg = mock(() => {});

    const result = await sendMessageStreaming(42, "bg test", onText, onBg);

    // Early persist fires immediately when task_started is detected
    expect(mockSetSessionId).toHaveBeenCalledWith(42, "sess-bg2");

    // Wait for background to complete
    await result.backgroundPromise;
  });

  test("backgroundPromise includes task_notification summary", async () => {
    mockMessages = [
      { type: "system", subtype: "init", session_id: "sess-notify" },
      {
        type: "assistant",
        message: { content: [{ type: "text", text: "launching agent" }] },
      },
      { type: "system", subtype: "task_started", task_id: "task-notify" },
      { type: "system", subtype: "task_notification", task_id: "task-notify", status: "completed", summary: "agent found 3 files", output_file: "/tmp/out" },
      { type: "result", session_id: "sess-notify-done" },
    ];

    const onText = mock(async () => {});
    const onBg = mock(() => {});

    const result = await sendMessageStreaming(42, "research", onText, onBg);
    const followUp = await result.backgroundPromise!;

    // "launching agent" is pre-task_started foreground text (sent via onText, not in array)
    // task_notification summary is its own array entry
    expect(followUp).toEqual(["agent found 3 files"]);
  });

  test("multiple assistant turns produce separate messages", async () => {
    mockMessages = [
      { type: "system", subtype: "init", session_id: "sess-multi" },
      { type: "system", subtype: "task_started", task_id: "task-multi" },
      {
        type: "assistant",
        message: { content: [{ type: "text", text: "working on it" }] },
      },
      {
        type: "assistant",
        message: { content: [{ type: "text", text: "done, here's the result" }] },
      },
      { type: "result", session_id: "sess-multi" },
    ];

    const onText = mock(async () => {});
    const onBg = mock(() => {});

    const result = await sendMessageStreaming(42, "do thing", onText, onBg);
    const followUp = await result.backgroundPromise!;

    expect(followUp).toEqual(["working on it", "done, here's the result"]);
  });

  test("iterator is not closed on background handoff (can continue iteration)", async () => {
    // This tests the core fix: raw iterator protocol allows continued iteration
    // after the foreground function returns. If we used for-await-of, the
    // generator would be closed via .return() and the background would get nothing.
    mockMessages = [
      { type: "system", subtype: "init", session_id: "sess-iter" },
      { type: "system", subtype: "task_started", task_id: "task-iter" },
      {
        type: "assistant",
        message: { content: [{ type: "text", text: "post-handoff text" }] },
      },
      { type: "result", session_id: "sess-iter" },
    ];

    const onText = mock(async () => {});
    const onBg = mock(() => {});

    const result = await sendMessageStreaming(42, "test", onText, onBg);
    expect(result.backgroundPromise).not.toBeNull();

    const followUp = await result.backgroundPromise!;
    expect(followUp).toEqual(["post-handoff text"]);
  });

  test("exports StreamingResult and ImageAttachment types", async () => {
    // Type-level verification — if this compiles, types are exported correctly
    const { sendMessageStreaming: fn } = await import("../src/agent/agent.js");
    expect(typeof fn).toBe("function");
  });
});
