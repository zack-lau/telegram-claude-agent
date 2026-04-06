import { describe, test, expect, beforeEach } from "bun:test";
import {
  hasBackgroundJobs,
  addBackgroundJob,
  removeBackgroundJob,
  isJobRelevant,
  incrementGeneration,
  getGeneration,
  clearAllBackgroundJobs,
} from "../src/agent/sessions.js";

describe("background job registry", () => {
  const CHAT_ID = 123;

  beforeEach(() => {
    clearAllBackgroundJobs(CHAT_ID);
  });

  test("hasBackgroundJobs returns false when no jobs", () => {
    expect(hasBackgroundJobs(CHAT_ID)).toBe(false);
  });

  test("hasBackgroundJobs returns true after adding a job", () => {
    addBackgroundJob(CHAT_ID, "task-1", "session-aaa");
    expect(hasBackgroundJobs(CHAT_ID)).toBe(true);
  });

  test("removeBackgroundJob clears a specific job", () => {
    addBackgroundJob(CHAT_ID, "task-1", "session-aaa");
    addBackgroundJob(CHAT_ID, "task-2", "session-bbb");
    removeBackgroundJob(CHAT_ID, "task-1");
    expect(hasBackgroundJobs(CHAT_ID)).toBe(true);
    removeBackgroundJob(CHAT_ID, "task-2");
    expect(hasBackgroundJobs(CHAT_ID)).toBe(false);
  });

  test("isJobRelevant returns true for current generation", () => {
    addBackgroundJob(CHAT_ID, "task-1", "session-aaa");
    expect(isJobRelevant(CHAT_ID, "task-1")).toBe(true);
  });

  test("isJobRelevant returns false after incrementGeneration", () => {
    addBackgroundJob(CHAT_ID, "task-1", "session-aaa");
    incrementGeneration(CHAT_ID);
    expect(isJobRelevant(CHAT_ID, "task-1")).toBe(false);
  });

  test("isJobRelevant returns false for unknown task", () => {
    expect(isJobRelevant(CHAT_ID, "nonexistent")).toBe(false);
  });

  test("clearAllBackgroundJobs removes everything", () => {
    addBackgroundJob(CHAT_ID, "task-1", "session-aaa");
    addBackgroundJob(CHAT_ID, "task-2", "session-bbb");
    clearAllBackgroundJobs(CHAT_ID);
    expect(hasBackgroundJobs(CHAT_ID)).toBe(false);
  });

  test("getGeneration returns 0 initially", () => {
    expect(getGeneration(999)).toBe(0);
  });

  test("getGeneration increments", () => {
    const before = getGeneration(CHAT_ID);
    incrementGeneration(CHAT_ID);
    expect(getGeneration(CHAT_ID)).toBe(before + 1);
  });
});
