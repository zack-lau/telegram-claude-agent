import { describe, expect, test } from "bun:test";
import {
  formatFatalError,
  sanitizeLogText,
  summarizeLogData,
  writeLog,
} from "../src/config.js";

const SYNTHETIC_TOKEN = "1234567890:ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghi";

describe("credential-safe logging", () => {
  test("redacts Telegram-shaped tokens from text", () => {
    const result = sanitizeLogText(`request /bot${SYNTHETIC_TOKEN}/getUpdates failed`);

    expect(result).not.toContain(SYNTHETIC_TOKEN);
    expect(result).toContain("[REDACTED_TELEGRAM_TOKEN]");
  });

  test("drops nested error objects and request metadata", () => {
    const result = summarizeLogData({
      name: "HttpError",
      message: `request /bot${SYNTHETIC_TOKEN}/setMyCommands failed`,
      error: { path: `/bot${SYNTHETIC_TOKEN}/setMyCommands` },
    });
    const serialized = JSON.stringify(result);

    expect(serialized).not.toContain(SYNTHETIC_TOKEN);
    expect(serialized).not.toContain("path");
    expect(serialized).not.toContain('"error"');
  });

  test("does not trust a custom constructor name", () => {
    const result = summarizeLogData({
      constructor: { name: SYNTHETIC_TOKEN },
      nested: "ignored",
    });

    expect(result).toEqual({ type: "object" });
  });

  test("formats fatal errors without credentials", () => {
    const result = formatFatalError(
      new Error(`request /bot${SYNTHETIC_TOKEN}/getUpdates failed`),
    );

    expect(result).not.toContain(SYNTHETIC_TOKEN);
    expect(result).toContain("[REDACTED_TELEGRAM_TOKEN]");
  });

  test("the actual log sink sanitizes message and data", () => {
    const calls: unknown[][] = [];
    const original = console.error;
    console.error = (...args: unknown[]) => { calls.push(args); };
    try {
      writeLog(
        "error",
        `request /bot${SYNTHETIC_TOKEN}/sendMessage failed`,
        {
          message: `request /bot${SYNTHETIC_TOKEN}/sendMessage failed`,
          error: { path: `/bot${SYNTHETIC_TOKEN}/sendMessage` },
        },
        "info",
      );
    } finally {
      console.error = original;
    }
    const serialized = JSON.stringify(calls);

    expect(serialized).not.toContain(SYNTHETIC_TOKEN);
    expect(serialized).not.toContain('"error"');
  });
});
