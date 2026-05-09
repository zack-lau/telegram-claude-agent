---
name: openclaw-update
description: Update OpenClaw on SGDGX01 and restart the gateway to match. Use this skill whenever the user asks to update, upgrade, or patch OpenClaw, or when you notice a CLI/gateway version mismatch in `openclaw status` output. Also use when agents on DGX report "Something went wrong" errors that could stem from a stale gateway.
---

# OpenClaw Update

Updates the OpenClaw installation on SGDGX01 and ensures the gateway process is restarted to match the new CLI version. This prevents the common failure mode where `npm update -g openclaw` updates files on disk but the running gateway keeps serving stale code.

## Why this matters

OpenClaw's gateway runs as a systemd service. When npm updates the package, the service keeps running the old loaded code. The systemd unit also bakes the version into its `Description` and `OPENCLAW_SERVICE_VERSION` env var. If you only run `npm update` without restarting, agents will hit errors from path mismatches (e.g., missing skill files that moved between versions).

## Steps

### 1. Run the update wrapper

```bash
ssh sgdgx01 'export PATH="/home/agents/.npm-global/bin:$PATH" && /home/agents/.local/bin/openclaw-update'
```

This script handles the full sequence: npm update, systemd unit regeneration, gateway restart, and version verification.

Use a 120-second timeout — the npm update can take a while.

### 2. Verify versions match

After the script completes, confirm both CLI and gateway are on the same version:

```bash
ssh sgdgx01 'export PATH="/home/agents/.npm-global/bin:$PATH" && openclaw status 2>&1 | grep -E "Update|Gateway "'
```

Look for `app <version>` in the Gateway line matching the npm version. If they don't match, the update wrapper will have flagged it already.

### 3. Check for post-restart errors

Tail the logs briefly to make sure nothing is broken:

```bash
ssh sgdgx01 'export PATH="/home/agents/.npm-global/bin:$PATH" && openclaw logs 2>&1 | tail -20'
```

Look for `error` lines, especially ENOENT or missing skill/plugin paths — these indicate the gateway is still referencing old paths.

### 4. Report results

Tell the user:
- Previous version and new version
- Whether gateway and CLI versions match
- Any errors in the post-restart logs
- If the changelog is interesting, offer to show highlights (the changelog lives at the npm package path, but you can also check the openclaw docs site)
