import { readFileSync, writeFileSync, existsSync } from "fs";
import { log } from "../config.js";
import type { SessionEntry } from "../types.js";

const SESSION_FILE = "./data/sessions.json";

let sessions: Map<number, SessionEntry> = new Map();

/**
 * Load session map from disk.
 */
export function loadSessions(): void {
  if (existsSync(SESSION_FILE)) {
    try {
      const raw = readFileSync(SESSION_FILE, "utf-8");
      const entries: SessionEntry[] = JSON.parse(raw);
      sessions = new Map(entries.map((e) => [e.chat_id, e]));
      log("info", `Loaded ${sessions.size} sessions`);
    } catch (err) {
      log("warn", "Failed to load sessions file, starting fresh", err);
      sessions = new Map();
    }
  }
}

/**
 * Persist session map to disk.
 */
function saveSessions(): void {
  const entries = Array.from(sessions.values());
  writeFileSync(SESSION_FILE, JSON.stringify(entries, null, 2));
}

/**
 * Get the session ID for a chat, or null if none exists.
 */
export function getSessionId(chatId: number): string | null {
  return sessions.get(chatId)?.session_id ?? null;
}

/**
 * Store or update a session mapping.
 */
export function setSessionId(chatId: number, sessionId: string): void {
  const existing = sessions.get(chatId);
  sessions.set(chatId, {
    chat_id: chatId,
    session_id: sessionId,
    last_active: Date.now(),
    message_count: (existing?.message_count ?? 0) + 1,
  });
  saveSessions();
}

/**
 * Clear session for a chat (start fresh conversation).
 */
export function clearSession(chatId: number): void {
  sessions.delete(chatId);
  saveSessions();
  log("info", `Cleared session for chat ${chatId}`);
}

/**
 * Get all active sessions.
 */
export function getAllSessions(): SessionEntry[] {
  return Array.from(sessions.values());
}

// ── Background job registry ──

interface BackgroundJob {
  taskId: string;
  sessionId: string;
  generation: number;
}

const chatGenerations = new Map<number, number>();
const backgroundJobs = new Map<number, Map<string, BackgroundJob>>();

export function hasBackgroundJobs(chatId: number): boolean {
  const jobs = backgroundJobs.get(chatId);
  return jobs !== undefined && jobs.size > 0;
}

export function addBackgroundJob(chatId: number, taskId: string, sessionId: string): void {
  if (!backgroundJobs.has(chatId)) {
    backgroundJobs.set(chatId, new Map());
  }
  backgroundJobs.get(chatId)!.set(taskId, {
    taskId,
    sessionId,
    generation: chatGenerations.get(chatId) ?? 0,
  });
}

export function removeBackgroundJob(chatId: number, taskId: string): void {
  const jobs = backgroundJobs.get(chatId);
  if (jobs) {
    jobs.delete(taskId);
    if (jobs.size === 0) backgroundJobs.delete(chatId);
  }
}

export function isJobRelevant(chatId: number, taskId: string): boolean {
  const job = backgroundJobs.get(chatId)?.get(taskId);
  if (!job) return false;
  return job.generation === (chatGenerations.get(chatId) ?? 0);
}

export function incrementGeneration(chatId: number): void {
  chatGenerations.set(chatId, (chatGenerations.get(chatId) ?? 0) + 1);
}

export function getGeneration(chatId: number): number {
  return chatGenerations.get(chatId) ?? 0;
}

export function clearAllBackgroundJobs(chatId: number): void {
  backgroundJobs.delete(chatId);
}
