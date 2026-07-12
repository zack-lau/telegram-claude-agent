import { describe, expect, test } from "bun:test";
import { CONTEXT_SERVER_HOST, isPublicHealthRequest } from "../src/health.js";

describe("public health endpoint", () => {
  test("binds to loopback", () => {
    expect(CONTEXT_SERVER_HOST).toBe("127.0.0.1");
  });

  test("allows only an exact GET health path", () => {
    expect(isPublicHealthRequest("GET", "http://127.0.0.1:3001/healthz")).toBe(true);
    expect(isPublicHealthRequest("POST", "http://127.0.0.1:3001/healthz")).toBe(false);
    expect(isPublicHealthRequest("GET", "http://127.0.0.1:3001/healthz?full=1")).toBe(false);
    expect(isPublicHealthRequest("GET", "http://127.0.0.1:3001/ask")).toBe(false);
    expect(isPublicHealthRequest("GET", "http://127.0.0.1:3001/notify")).toBe(false);
  });
});
