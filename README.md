# telegram-claude-agent

A personal Telegram bot powered by the [Anthropic Claude Agent SDK](https://github.com/anthropics/claude-agent-sdk). Send it a message and it runs a full Claude Code agent session — with tool use, file access, web search, memory, and more — all from Telegram.

Built for a single-user (allowlist-gated) setup on a personal server.

---

## Features

- **Text, voice, photo, and document** input from Telegram
- **Streaming responses** — replies arrive in real time as Claude thinks
- **Background agents** — long-running tasks run in the background and report back when done
- **Tool approval** — destructive or sensitive tool calls get an inline Telegram prompt before executing
- **Per-chat message queue** — concurrent messages are queued and processed in order
- **Optional modules** — memory, voice transcription, and doc search are plug-in; the bot works without them

---

## Quick start

**Requirements:** [Bun](https://bun.sh) ≥ 1.0, Node.js ≥ 18 (for the agent SDK)

```bash
git clone https://github.com/yourname/telegram-claude-agent
cd telegram-claude-agent
bun install
cp .env.example .env
# edit .env with your token and user ID
bun run start
```

The bot starts with only three required environment variables.

---

## Configuration

Copy `.env.example` and fill in the required fields. Everything else is optional.

### Required

| Variable | Description |
|---|---|
| `TELEGRAM_BOT_TOKEN` | Your bot token from [@BotFather](https://t.me/BotFather) |
| `ALLOWED_USERS` | Comma-separated Telegram user IDs allowed to use the bot (e.g. `123456789`) |

The bot silently drops messages from anyone not in `ALLOWED_USERS`.

### Agent SDK

| Variable | Default | Description |
|---|---|---|
| `AGENT_CWD` | `./workspace` | Working directory for the agent |
| `AGENT_MAX_TURNS` | `15` | Max tool-use turns per message |
| `AGENT_PERMISSION_MODE` | `bypassPermissions` | SDK permission mode (`bypassPermissions`, `acceptEdits`, `default`) |

### Optional modules

See the [Modules](#modules) section below for setup instructions for each.

| Variable | Module | Description |
|---|---|---|
| `MEMORY_MCP_COMMAND` | Memory | Python executable for the LanceDB MCP server |
| `MEMORY_MCP_SCRIPT` | Memory | Path to the LanceDB MCP server script |
| `SPARK_QMD_MCP_URL` | Doc search | URL of a running QMD MCP server |
| `SPARK_WHISPER_URL` | Voice | URL of a Whisper-compatible transcription endpoint |
| `SPARK_EMBED_URL` | Memory | BGE-M3 embeddings server (used by LanceDB MCP) |
| `SPARK_RERANK_URL` | Memory | BGE Reranker server (used by LanceDB MCP) |
| `SPARK_TTS_URL` | TTS | Text-to-speech server (Kokoro-compatible) |
| `SPARK_SEARCH_URL` | Web search | SearXNG instance URL |
| `SPARK_CRAWL_URL` | Web crawl | Crawl4AI instance URL |

---

## Modules

Optional modules extend what the agent can do. Each is independently configurable — the bot starts and runs normally without any of them.

### Memory

The agent can store and recall facts, goals, and preferences across conversations using a [LanceDB](https://lancedb.github.io/lancedb/)-backed vector memory store.

**What it enables:** `memory_store`, `memory_recall`, `memory_forget`, `memory_list`, `memory_stats` tools in the agent. The `/memory` command shows current memory stats.

**Requires:**
- A running LanceDB MCP server (see `workspace/mcp-memory-lancedb/`)
- BGE-M3 embeddings server (`SPARK_EMBED_URL`) for semantic search
- BGE Reranker server (`SPARK_RERANK_URL`) for result reranking

**Setup:**
```bash
cd workspace/mcp-memory-lancedb
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
```

Then set in `.env`:
```
MEMORY_MCP_COMMAND=./workspace/mcp-memory-lancedb/.venv/bin/python
MEMORY_MCP_SCRIPT=./workspace/mcp-memory-lancedb/server.py
SPARK_EMBED_URL=http://your-embed-server:8001
SPARK_RERANK_URL=http://your-rerank-server:8002
```

---

### Voice transcription

Voice messages sent to the bot are transcribed and passed to the agent as text.

**What it enables:** Send voice notes directly — they're converted to text before the agent sees them.

**Requires:** Any [OpenAI-compatible](https://platform.openai.com/docs/api-reference/audio/createTranscription) `/v1/audio/transcriptions` endpoint. Works with:
- [faster-whisper-server](https://github.com/fedirz/faster-whisper-server) (self-hosted)
- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) server mode
- OpenAI Whisper API (set `SPARK_WHISPER_URL=https://api.openai.com`)

**Setup:**
```
SPARK_WHISPER_URL=http://your-whisper-server:8007
```

---

### Doc search

The agent can search a corpus of indexed markdown documents — notes, reports, research, documentation.

**What it enables:** `mcp__qmd__query`, `mcp__qmd__get`, `mcp__qmd__multi_get` tools. The agent can find and read documents by keyword or semantic query.

**Requires:** A running [QMD](https://github.com/yourusername/qmd) MCP server with your documents indexed.

**Setup:**
```
SPARK_QMD_MCP_URL=http://your-qmd-server:8181/mcp
```

---

### Web search

Enables the agent to search the web via a self-hosted [SearXNG](https://searxng.github.io/searxng/) instance.

**Requires:** A SearXNG instance. Configure it in `workspace/.mcp.json` under `mcpServers.searxng`, using `SPARK_SEARCH_URL`.

---

### Web crawl

Enables the agent to fetch and parse web pages via [Crawl4AI](https://github.com/unclecode/crawl4ai).

**Requires:** A running Crawl4AI server. Configure it in `workspace/.mcp.json`.

---

## Architecture

```
Telegram
  │
  ▼
grammy bot (src/bot/)
  ├── auth + rate limit middleware
  ├── message queue (per chat)
  └── handlers: text, voice, photo, document
           │
           ▼
    agent/agent.ts
    (Claude Agent SDK)
           │
           ├── MCP servers (memory, qmd, projects)
           ├── Tool hooks (approval gate for destructive ops)
           └── Session management (resume across messages)
```

Sessions are persisted to `data/sessions.json` so conversations survive restarts.

---

## Tool approval

Certain tool calls require explicit approval via a Telegram inline keyboard before the agent proceeds:

- `rm`, `git push/reset/rebase`, `docker rm`, `kill`, `pkill`
- `systemctl stop/disable/restart`
- Gmail draft creation

All other tools run automatically. Add patterns to `GATED_PATTERNS` in `src/bot/approvals.ts` to gate additional tools.

---

## Running as a service

A launchd plist for macOS is included:

```bash
bun run setup:launchd
```

For Linux, a systemd unit:

```ini
[Unit]
Description=Telegram Claude Agent
After=network-online.target

[Service]
Type=simple
WorkingDirectory=/path/to/telegram-claude-agent
EnvironmentFile=/path/to/.env
ExecStart=/path/to/bun run src/index.ts
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

---

## License

MIT
