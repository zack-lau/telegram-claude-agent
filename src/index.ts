import { mkdirSync, existsSync } from "fs";
import { getConfig, log } from "./config.js";
import { createBot } from "./bot/bot.js";
import { loadSessions } from "./agent/sessions.js";
import { startContextServer } from "./context-server.js";

async function main() {
  const cfg = getConfig();
  log("info", "Starting Telegram Claude Agent");

  // Ensure directories exist
  for (const dir of ["./data", cfg.AGENT_CWD, cfg.PROJECTS_ROOT]) {
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true });
      log("info", `Created directory: ${dir}`);
    }
  }

  // Load session mappings
  loadSessions();

  // Start context server for get_project_context MCP tool
  const contextServer = await startContextServer(cfg);

  // Create and start the bot
  const bot = createBot();

  await bot.api.setMyCommands([
    { command: "new", description: "Start a fresh conversation" },
    { command: "status", description: "Check system status" },
    { command: "memory", description: "View memory stats" },
  ]);

  // Graceful shutdown
  const shutdown = async (signal: string) => {
    log("info", `Received ${signal}, shutting down...`);
    contextServer.stop(true);
    bot.stop();
    process.exit(0);
  };
  process.on("SIGINT", () => shutdown("SIGINT"));
  process.on("SIGTERM", () => shutdown("SIGTERM"));

  // Drop stale getUpdates connections to avoid 409 conflicts on restart
  await bot.api.deleteWebhook({ drop_pending_updates: false });

  log("info", "Bot is running. Waiting for messages...");
  await bot.start({
    onStart: () => log("info", "Telegram bot connected and polling"),
  });
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
