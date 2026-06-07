---
name: sync-mcp
description: "Sync MCP servers from ~/.claude/mcp.json into Claude Desktop's claude_desktop_config.json. Adds missing entries, reports conflicts, never overwrites existing entries without confirmation."
---

# sync-mcp

Sync MCP server definitions from Claude Code's `~/.claude/mcp.json` into the Claude Desktop app config at `~/Library/Application Support/Claude/claude_desktop_config.json`.

## How Claude Code and Claude Desktop share MCP config (macOS)

On macOS, Claude Code reads MCP servers from **two** sources:
1. `~/Library/Application Support/Claude/claude_desktop_config.json` — shared with Desktop
2. `~/.claude/mcp.json` — Claude Code-only additions

Claude Desktop only reads source (1). So any server added to `mcp.json` is invisible to Cowork/Desktop until synced.

## Steps

1. **Read source**: `~/.claude/mcp.json` → `.mcpServers`
2. **Read target**: `~/Library/Application Support/Claude/claude_desktop_config.json` → `.mcpServers`
3. **Diff**:
   - **Missing**: in `mcp.json` but not in Desktop config → add
   - **Conflict**: same key exists in both with different config → report, do NOT overwrite without user confirmation
   - **Already present**: same key, identical config → skip silently
4. **Write** the updated Desktop config (pretty-printed, 2-space indent)
5. **Report** a summary table: added | conflict | skipped

## Implementation

```python
import json, pathlib, copy

CC_MCP   = pathlib.Path.home() / ".claude/mcp.json"
DESKTOP  = pathlib.Path.home() / "Library/Application Support/Claude/claude_desktop_config.json"

cc      = json.loads(CC_MCP.read_text())
desktop = json.loads(DESKTOP.read_text())

src = cc.get("mcpServers", {})
dst = desktop.setdefault("mcpServers", {})

added, conflicts, skipped = [], [], []

for name, cfg in src.items():
    if name not in dst:
        dst[name] = copy.deepcopy(cfg)
        added.append(name)
    elif dst[name] == cfg:
        skipped.append(name)
    else:
        conflicts.append((name, dst[name], cfg))   # existing, incoming

if added:
    DESKTOP.write_text(json.dumps(desktop, indent=2))

print(f"Added:    {added}")
print(f"Skipped:  {skipped}")
print(f"Conflicts:{[c[0] for c in conflicts]}")
for name, existing, incoming in conflicts:
    print(f"\n  [{name}] existing:\n    {existing}\n  [{name}] incoming:\n    {incoming}")
```

## Conflict resolution

When a conflict is found, show both configs and ask the user:
- **Keep existing** (Desktop wins) — default, do nothing
- **Use incoming** (mcp.json wins) — replace in Desktop config
- **Skip** — leave as-is

Never silently overwrite a conflicting entry.

## After syncing

Remind the user to **restart Claude Desktop** so the new servers are loaded. Config changes are only picked up at startup.

## Notes

- Plugin-provided MCPs (figma, memory, computer-use, qmd, trading, etc.) come from Claude Code plugins and are never in `mcp.json` — they don't need syncing.
- `~/.claude/mcp-servers/` dirs store venv installs only; their launch configs live in `claude_desktop_config.json` (the `.venv/bin/...` command entries).
- The openclaw entry may differ between the two files: `mcp.json` uses `--url` and `--token-file` flags; Desktop uses bare SSH. Treat these as a conflict and ask the user which config to use.
