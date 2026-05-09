---
name: timestamp
description: Blockchain-timestamp documents using OpenTimestamps (Bitcoin). Use when the user says "timestamp", "anchor", "proof of existence", "notarize", or wants to prove a document existed at a specific time. Also use after completing major document milestones (patent drafts, architecture docs, contracts).
---

# Blockchain Timestamping via OpenTimestamps

Anchor SHA-256 hashes on Bitcoin for tamper-proof proof of existence. Free, no API key, no vendor lock-in.

## Prerequisites

- `opentimestamps-client` installed: `pip3 install --break-system-packages opentimestamps-client`
- Verification script: `~/claude-agent/scripts/ots-verify.sh`
- No Bitcoin node needed (uses free public calendar servers + Blockstream API)
- No wallet or funds needed

## Commands

### Stamp (create proof)

```bash
ots stamp <file1> [file2] [file3...]
```

Creates `.ots` proof files alongside originals. Submits to 4 calendar servers automatically.

### Upgrade (complete proof after Bitcoin confirmation)

```bash
ots upgrade <file>.ots
```

Run a few hours after stamping. Completes the Merkle path to the Bitcoin block. After upgrade, the `.ots` file is self-contained and verifiable forever without any server.

### Verify (no Bitcoin node needed)

```bash
# Preferred — uses Blockstream public API, no node required
~/claude-agent/scripts/ots-verify.sh <file>.ots [original_file]
```

This checks:
1. File SHA-256 matches the hash in the .ots proof
2. Extracts Bitcoin block attestations from the proof
3. Queries Blockstream public API for each block's merkle root
4. Confirms merkle roots match — fully trustless verification

If a Bitcoin node is available, `ots verify <file>.ots` also works.

### Info (inspect proof structure)

```bash
ots info <file>.ots
```

Shows the full Merkle path from file hash to Bitcoin transaction.

## Workflow

When asked to timestamp documents:

1. **Compute hash first** — `shasum -a 256 <file>` — record it for the user
2. **Stamp** — `ots stamp <file>` — creates `<file>.ots`
3. **Confirm submission** — report which calendar servers accepted
4. **Record** — note the hash and file path in a timestamp-records.md or memory
5. **Schedule upgrade** — remind user to run `ots upgrade` in ~4-12 hours, or set a cron
6. **Package** — after upgrade, ZIP original + .ots together for archival
7. **Upload** — `gws drive +upload <zip> --parent <folder_id>` to Google Drive `Timestamp-Packages/`

## Batch stamping

```bash
ots stamp file1.md file2.md file3.pdf
```

All files submitted in one call. Each gets its own `.ots` file.

## Packaging verified timestamps

After stamping and upgrading, create a ZIP package:

```bash
# 1. Verify hash hasn't changed
HASH=$(shasum -a 256 document.md | awk '{print $1}')
EXPECTED="<hash_from_stamp_time>"
[ "$HASH" = "$EXPECTED" ] && echo "MATCH" || echo "MISMATCH — DO NOT ZIP"

# 2. ZIP original + proof
zip Document_${HASH:0:12}.zip document.md document.md.ots

# 3. Upload to Drive
gws drive +upload Document_${HASH:0:12}.zip --parent <Timestamp-Packages-folder-id>
```

Google Drive `Timestamp-Packages/` folder ID: `1-E3U_qQrbWV9t0cxquxKIFlPGPIuz1SU`

## Important notes

- `.ots` files are small (~700 bytes) — commit them to git or store alongside documents
- The original file must remain byte-for-byte identical for verification to work
- Proofs are pending until `ots upgrade` completes (requires Bitcoin block confirmation)
- Public calendars: `a.pool.opentimestamps.org`, `b.pool.opentimestamps.org`, `a.pool.eternitywall.com`, `ots.btc.catallaxy.com`
- No rate limits for normal usage (5-20 docs/month is fine)
- Proof is legally defensible — verifiable by anyone with a Bitcoin node or the Blockstream API
- **NEVER modify the original file after stamping** — any edit invalidates the proof

## When to use

- After finalizing patent drafts or architecture documents
- Before any public disclosure of IP
- When user explicitly asks to timestamp/notarize/anchor
- After completing major project milestones
- When establishing prior art dates

## Cost

$0. Forever. Calendar servers are community-operated and free.
