import { readFileSync, writeFileSync, existsSync } from "fs";
import { resolve } from "path";
import type { Bot } from "grammy";
import { log } from "./config.js";

const REMINDERS_PATH = resolve("./data/reminders.json");
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
  writeFileSync(REMINDERS_PATH, JSON.stringify(reminders, null, 2), "utf8");
}

function removeReminder(id: string): void {
  const reminders = loadReminders().filter((r) => r.id !== id);
  saveReminders(reminders);
}

// Tracks which reminder IDs already have a live timeout so we don't double-schedule.
const scheduled = new Set<string>();

function scheduleOne(reminder: Reminder, bot: Bot): void {
  if (scheduled.has(reminder.id)) return;

  const now = Date.now();
  const fireAt = new Date(reminder.fireAt).getTime();
  const delay = fireAt - now;

  scheduled.add(reminder.id);

  const fire = async (late: boolean) => {
    scheduled.delete(reminder.id);
    removeReminder(reminder.id);
    const text = late
      ? `⏰ (late) ${reminder.message}`
      : `⏰ ${reminder.message}`;
    try {
      await bot.api.sendMessage(reminder.chatId, text);
      log("info", `scheduler: fired reminder ${reminder.id} for chat ${reminder.chatId}`);
    } catch (err) {
      log("error", `scheduler: failed to send reminder ${reminder.id}`, err);
    }
  };

  if (delay <= 0) {
    // Already past — fire immediately as late
    fire(true).catch(() => {});
  } else {
    setTimeout(() => fire(false).catch(() => {}), delay);
    log("info", `scheduler: scheduled reminder ${reminder.id} in ${Math.round(delay / 1000)}s`);
  }
}

export function startScheduler(bot: Bot): void {
  const scan = () => {
    const reminders = loadReminders();
    for (const r of reminders) {
      scheduleOne(r, bot);
    }
  };

  scan();
  setInterval(scan, SCAN_INTERVAL_MS);
  log("info", `scheduler: started (${loadReminders().length} pending reminders)`);
}
