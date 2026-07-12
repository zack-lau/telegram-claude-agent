---
name: project-docs
description: Manage Prostec Labs project documentation versioning, timestamping, and Google Drive archival. Use when creating new projects, promoting doc versions, or when the user says "promote", "new version", "archive docs", or "timestamp and upload".
---

# Prostec Labs Project Documentation Management

Standard folder structure and versioning workflow for all Prostec Labs projects (Aegis, Praxis, and future projects).

## Project Folder Structure

All projects follow this layout:

```
prostec-<name>/
├── CLAUDE.md                    ← project conventions + product spec
├── .gitignore
├── app/                         ← client application (Flutter, etc.)
├── api/                         ← backend API (Lambda, Rust, etc.)
├── infra/                       ← Terraform infrastructure
│   ├── main.tf
│   └── ...
├── docs/
│   └── v1/                      ← current working version
│       ├── architecture.md
│       ├── adr/                 ← architecture decision records
│       │   ├── 0001-*.md
│       │   └── ...
│       ├── pricing.md
│       └── ...
└── reviews/                     ← external reviews (Qwen, Codex, etc.)
    └── YYYY-MM-DD/
```

## Doc Versioning Rules

- All docs live under `docs/vN/` (e.g., `docs/v1/`)
- `docs/` root contains ONLY version folders, never loose files
- Edit in place within the current version folder
- No version suffixes on filenames (e.g., `architecture.md`, not `architecture-v2.md`)

## Version Promotion Workflow

When the user says "promote to vN" or "new version":

### Step 1 — Timestamp all docs

```bash
cd <project>/docs/vN/
ots stamp *.md adr/*.md
# Also stamp any other .md files in subdirectories
```

Record SHA-256 hashes:
```bash
shasum -a 256 *.md adr/*.md
```

### Step 2 — Package for archival

```bash
# Create ZIP with all .md + .ots files
cd <project>/docs/
zip -r <ProjectName>-vN_<short_hash>.zip vN/
```

### Step 3 — Upload to Google Drive

Upload the ZIP to `Timestamp-Packages/<ProjectName>/`:

```bash
cd /Users/zack/claude-agent

# Create project subfolder if first promotion
gws drive files create --json '{"name":"<ProjectName>","mimeType":"application/vnd.google-apps.folder","parents":["1-E3U_qQrbWV9t0cxquxKIFlPGPIuz1SU"]}'

# Upload the ZIP
gws drive +upload <ProjectName>-vN_<short_hash>.zip --parent <project_folder_id>
```

Google Drive folder IDs:
- `Timestamp-Packages/`: `1-E3U_qQrbWV9t0cxquxKIFlPGPIuz1SU`
- `Timestamp-Packages/Aegis/`: (create on first use)
- `Timestamp-Packages/Praxis/`: (create on first use)

### Step 4 — Create next version

```bash
cp -r <project>/docs/vN/ <project>/docs/vN+1/
# Remove .ots files from the new working version
find <project>/docs/vN+1/ -name "*.ots" -delete
```

The old `docs/vN/` is now a frozen archive (with .ots proofs). The new `docs/vN+1/` is the working head.

## Creating a New Project

When the user asks to create a new Prostec Labs project:

1. Use `mcp__projects__project_create` with name `prostec-<name>`
2. Write a proper CLAUDE.md with product vision, tech stack, architecture, and conventions
3. Create the folder structure:
   ```bash
   mkdir -p <project>/docs/v1/adr
   mkdir -p <project>/app
   mkdir -p <project>/api
   mkdir -p <project>/infra
   mkdir -p <project>/reviews
   ```
4. Add versioning convention to CLAUDE.md:
   ```
   - Use doc versioning: `docs/vN/` folders, timestamp on promotion, upload to Drive
   ```

## Important Notes

- NEVER modify files in a frozen version folder (contains .ots proofs)
- Always verify `shasum -a 256` before packaging
- .ots proofs need `ots upgrade` after ~4-12 hours to complete Bitcoin confirmation
- The original file must remain byte-for-byte identical for .ots verification
- Schedule `ots upgrade` reminder after stamping
