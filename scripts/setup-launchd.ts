#!/usr/bin/env bun

/**
 * Generate a macOS LaunchAgent plist to keep the bot running.
 *
 * Usage: bun run scripts/setup-launchd.ts
 *
 * This creates a plist in ~/Library/LaunchAgents/ that:
 * - Starts the bot on login
 * - Restarts on crash (max 3 retries per 60s)
 * - Logs stdout/stderr to ~/Library/Logs/
 */

import { writeFileSync, mkdirSync, existsSync } from "fs";
import { resolve } from "path";
import { homedir } from "os";

const LABEL = "com.user.telegram-claude-agent";
const projectDir = resolve(import.meta.dir, "..");
const bunPath = Bun.which("bun") ?? "/usr/local/bin/bun";
const home = homedir();
const plistDir = `${home}/Library/LaunchAgents`;
const logDir = `${home}/Library/Logs`;
const plistPath = `${plistDir}/${LABEL}.plist`;

// Ensure directories exist
for (const dir of [plistDir, logDir]) {
  if (!existsSync(dir)) {
    mkdirSync(dir, { recursive: true });
  }
}

const plist = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LABEL}</string>

  <key>ProgramArguments</key>
  <array>
    <string>${bunPath}</string>
    <string>run</string>
    <string>src/index.ts</string>
  </array>

  <key>WorkingDirectory</key>
  <string>${projectDir}</string>

  <key>RunAtLoad</key>
  <true/>

  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>

  <key>ThrottleInterval</key>
  <integer>20</integer>

  <key>StandardOutPath</key>
  <string>${logDir}/${LABEL}.log</string>

  <key>StandardErrorPath</key>
  <string>${logDir}/${LABEL}.error.log</string>

  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/usr/local/bin:/usr/bin:/bin:${home}/.bun/bin</string>
  </dict>

  <!-- launchd starts processes with a minimal environment, but Bun
       automatically loads .env from the WorkingDirectory above, so
       TELEGRAM_BOT_TOKEN, ALLOWED_USERS, and all other config vars
       are available to the process at runtime without extra setup. -->
</dict>
</plist>
`;

writeFileSync(plistPath, plist);
console.log(`✅ LaunchAgent plist written to: ${plistPath}`);
console.log();
console.log("To load (start now + auto-start on login):");
console.log(`  launchctl load ${plistPath}`);
console.log();
console.log("To unload (stop + disable auto-start):");
console.log(`  launchctl unload ${plistPath}`);
console.log();
console.log("To check status:");
console.log(`  launchctl list | grep ${LABEL}`);
console.log();
console.log("Logs:");
console.log(`  tail -f ${logDir}/${LABEL}.log`);
console.log(`  tail -f ${logDir}/${LABEL}.error.log`);
