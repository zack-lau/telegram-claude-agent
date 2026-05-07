import { readFileSync, writeFileSync, existsSync, renameSync } from "fs";
import { resolve } from "path";
import type { Bot } from "grammy";
import { log } from "./config.js";

const REMINDERS_PATH = resolve("./data/reminders.json");
const REMINDERS_TMP = REMINDERS_PATH + ".tmp";
const SCAN_INTERVAL_MS = 60_000; // re-scan file every minute for reminders Mira adds mid-session

export interface Reminder {
  id: string;
  chatId: number;
  fireAt: string; // ISO 8601
  message: string;
  createdAt: string; // ISO 8601
}

function loadReminders(): Reminder[] {
  if (!existsSync(REMINDERS_PATH)) return [];
  try {
    return JSON.parse(readFileSync(REMINDERS_PATH, "utf8")) as Reminder[];
  } catch (err) {
    log("error", "scheduler: failed to parse reminders.json", err);
    return [];
  }
}

function saveReminders(reminders: Reminder[]): void {
  // Atomic write: write to .tmp then rename so a crash mid-write never corrupts the live file.
  writeFileSync(REMINDERS_TMP, JSON.stringify(reminders, null, 2), "utf8");
  renameSync(REMINDERS_TMP, REMINDERS_PATH);
}

function removeReminder(id: string): void {
  const reminders = loadReminders().filter((r) => r.id !== id);
  saveReminders(reminders);
}

// Tracks which reminder IDs already have a live timeout so we don't double-schedule.
const scheduled = new Set<string>();
let scanIntervalId: ReturnType<typeof setInterval> | null = null;

function scheduleOne(reminder: Reminder, bot: Bot): void {
  if (scheduled.has(reminder.id)) return;

  const fireAt = new Date(reminder.fireAt).getTime();
  if (!Number.isFinite(fireAt)) {
    log("error", `scheduler: reminder ${reminder.id} has invalid fireAt "${reminder.fireAt}", skipping`);
    removeReminder(reminder.id);
    return;
  }

  const delay = fireAt - Date.now();
  scheduled.add(reminder.id);

  const fire = async (late: boolean) => {
    scheduled.delete(reminder.id);
    removeReminder(reminder.id);
    const text = late ? `⏰ (late) ${reminder.message}` : `⏰ ${reminder.message}`;
    try {
      await bot.api.sendMessage(reminder.chatId, text);
      log("info", `scheduler: fired reminder ${reminder.id} for chat ${reminder.chatId}`);
    } catch (err) {
      log("error", `scheduler: failed to send reminder ${reminder.id}`, err);
    }
  };

  if (delay <= 0) {
    fire(true).catch(() => {});
  } else {
    setTimeout(() => fire(false).catch(() => {}), delay);
    log("info", `scheduler: scheduled reminder ${reminder.id} in ${Math.round(delay / 1000)}s`);
  }
}

export function stopScheduler(): void {
  if (scanIntervalId !== null) {
    clearInterval(scanIntervalId);
    scanIntervalId = null;
  }
}

export function startScheduler(bot: Bot): void {
  stopScheduler(); // clear any previous interval if called more than once

  const scan = () => {
    for (const r of loadReminders()) {
      scheduleOne(r, bot);
    }
  };

  scan();
  scanIntervalId = setInterval(scan, SCAN_INTERVAL_MS);
  log("info", `scheduler: started (${loadReminders().length} pending reminders)`);
}
