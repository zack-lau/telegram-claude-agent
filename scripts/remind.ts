#!/usr/bin/env bun
/**
 * remind.ts — CLI for managing Mira reminders
 *
 * Usage:
 *   bun run scripts/remind.ts add <chatId> <ISO8601> <message>
 *   bun run scripts/remind.ts list
 *   bun run scripts/remind.ts remove <id>
 */
import { readFileSync, writeFileSync, renameSync, existsSync } from "fs";
import { resolve } from "path";
import { randomUUID } from "crypto";

const REMINDERS_PATH = resolve("./data/reminders.json");
const REMINDERS_TMP = REMINDERS_PATH + ".tmp";

interface Reminder {
  id: string;
  chatId: number;
  fireAt: string;
  message: string;
  createdAt: string;
}

function load(): Reminder[] {
  if (!existsSync(REMINDERS_PATH)) return [];
  try {
    return JSON.parse(readFileSync(REMINDERS_PATH, "utf8")) as Reminder[];
  } catch {
    console.error("error: reminders.json is corrupted — cannot read");
    process.exit(1);
  }
}

function save(reminders: Reminder[]): void {
  writeFileSync(REMINDERS_TMP, JSON.stringify(reminders, null, 2), "utf8");
  renameSync(REMINDERS_TMP, REMINDERS_PATH);
}

const [, , cmd, ...args] = process.argv;

if (cmd === "add") {
  const [chatIdStr, fireAt, ...msgParts] = args;
  const chatId = parseInt(chatIdStr ?? "", 10);
  const message = msgParts.join(" ");

  if (!Number.isInteger(chatId) || chatId <= 0 || !fireAt || !message) {
    console.error("usage: remind.ts add <chatId> <ISO8601> <message>");
    process.exit(1);
  }

  const fireMs = new Date(fireAt).getTime();
  if (!Number.isFinite(fireMs)) {
    console.error(`invalid date: ${fireAt} — use ISO 8601 (e.g. 2026-05-07T15:00:00+08:00)`);
    process.exit(1);
  }

  const reminder: Reminder = {
    id: randomUUID(),
    chatId,
    fireAt: new Date(fireAt).toISOString(),
    message,
    createdAt: new Date().toISOString(),
  };

  const reminders = load();
  reminders.push(reminder);
  save(reminders);

  const diff = Math.round((fireMs - Date.now()) / 1000);
  console.log(`added reminder ${reminder.id} — fires in ${diff}s (${reminder.fireAt})`);

} else if (cmd === "list") {
  const reminders = load();
  if (reminders.length === 0) {
    console.log("no pending reminders");
  } else {
    for (const r of reminders) {
      const diff = Math.round((new Date(r.fireAt).getTime() - Date.now()) / 1000);
      const when = diff > 0 ? `in ${diff}s` : `${Math.abs(diff)}s ago (overdue)`;
      console.log(`${r.id} | chat:${r.chatId} | ${when} | ${r.message}`);
    }
  }

} else if (cmd === "remove") {
  const [id] = args;
  if (!id) { console.error("usage: remind.ts remove <id>"); process.exit(1); }
  const reminders = load().filter((r) => r.id !== id);
  save(reminders);
  console.log(`removed ${id}`);

} else {
  console.error("usage: remind.ts <add|list|remove> ...");
  process.exit(1);
}
