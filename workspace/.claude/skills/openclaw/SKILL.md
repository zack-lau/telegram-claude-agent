---
name: openclaw
description: "Read, send, and monitor messages through OpenClaw conversation routes (Telegram gateway)."
metadata:
  openclaw:
    emoji: "🦞"
---

# OpenClaw MCP

OpenClaw is a messaging gateway that bridges Claude agents with external channels (Telegram). The MCP server exposes 9 tools under `mcp__openclaw__*`.

## Tools

| Tool | Purpose |
|------|---------|
| `conversations_list` | List available session routes |
| `conversation_get` | Get a specific conversation by session key |
| `messages_read` | Read recent messages (persistent log) |
| `messages_send` | Send a message to a conversation |
| `attachments_fetch` | Fetch attachments from a conversation |
| `events_poll` | Poll the event queue since a cursor |
| `events_wait` | Long-poll for the next event (blocks until arrival or timeout) |
| `permissions_list_open` | List pending exec/plugin approval requests |
| `permissions_respond` | Respond to an approval request |

## Session keys

The default Telegram session is `agent:main:main`. Use `conversations_list` to discover others.

## Sending a message

```
messages_send(session_key: "agent:main:main", text: "Hello")
```

## Reading messages

`messages_read` reads from the **persistent message log** — it always has the full history regardless of whether events were consumed.

```
messages_read(session_key: "agent:main:main", limit: 20)
```

## Live event polling (correct cursor pattern)

`events_poll` and `events_wait` use an **event queue cursor** — an independent integer starting at 1 that increments per event. It is NOT the message `seq` field and NOT a timestamp.

**Wrong:** `events_wait(after_cursor: 31)` — this is a message seq, not a queue position. If the queue has fewer than 31 events, it returns immediately with `null`.

**Correct pattern:**

```
# Step 1: get the current queue tail
result = events_poll(session_key: "agent:main:main", after_cursor: 0, limit: 200)
cursor = result.next_cursor   # e.g. 4

# Step 2: block until the next new event
event = events_wait(session_key: "agent:main:main", after_cursor: cursor, timeout_ms: 60000)
# event.cursor is the next position to use for continued polling
```

`next_cursor` in the `events_poll` response equals the cursor of the last returned event. Always use it — never derive the cursor from `messageSeq`, `seq`, or timestamps.

## Differences: events vs messages

| | `events_poll` / `events_wait` | `messages_read` |
|---|---|---|
| Storage | Transient queue | Persistent log |
| Consumed by agent? | Yes — agent on sgdgx01 dequeues events | No |
| Use for | Real-time detection of new activity | Reading history |
| Cursor type | Queue position (1, 2, 3…) | N/A |
