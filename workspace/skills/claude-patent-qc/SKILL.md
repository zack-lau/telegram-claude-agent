---
name: claude-patent-qc
description: Patent QC sweep — check claims, specification, and formalities compliance. Triggers on "qc check", "quality check", "patent qc", "/qc".
---

# Patent QC Skill (stub)

`leonardhope/claude-patent-qc` does not exist on GitHub (checked 2026-05-13).
This stub wires the `/qc` command using Claude-Patent-Creator's built-in compliance tools.

## Fallback QC Workflow

Run these steps in order for `inventions/<slug>/`:

1. **Claims — 35 USC 112(b) definiteness**
   Call MCP tool `review_patent_claims` with the claims text from `04_draft/`.

2. **Specification — 35 USC 112(a) enablement + written description**
   Call MCP tool `review_specification` with the detailed description.

3. **Formalities — MPEP 608**
   Call MCP tool `check_formalities` with the full draft.

4. **Manual checks**
   - Abstract ≤ 150 words
   - Title ≤ 500 characters
   - Every figure referenced by number in the detailed description
   - No new matter introduced relative to the disclosure

5. **Output report**
   Save to `inventions/<slug>/05_qc_reports/qc_v1.0_<YYYY-MM-DD>.md`

   Format:
   ```
   # QC Report — <slug> — <date>

   ## BLOCKING issues (must fix before filing)
   - [list]

   ## ADVISORY issues (nice-to-have)
   - [list]

   ## Status: PASS / FAIL
   ```

## To Install Real Skill

When `leonardhope/claude-patent-qc` becomes available:
```bash
rm -rf /Users/zack/claude-agent/workspace/skills/claude-patent-qc
git clone https://github.com/leonardhope/claude-patent-qc \
  /Users/zack/claude-agent/workspace/skills/claude-patent-qc
# then re-register via Claude Code plugin settings
```
