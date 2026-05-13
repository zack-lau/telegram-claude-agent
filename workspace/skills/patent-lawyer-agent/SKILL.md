---
name: patent-lawyer-agent
description: Senior patent attorney review agent. Triggers on "attorney review", "lawyer review", "final review", "patent-lawyer-agent".
---

# Patent Lawyer Agent (stub)

`mcpmarket.com/tools/skills/patent-lawyer-agent` returned HTTP 429 on all URL
variants during setup (2026-05-13). This stub wires attorney-review tasks
using Claude-Patent-Creator's full-review workflow.

## Fallback Attorney Review Workflow

For attorney-handoff-ready review, run the following in order:

1. **Full patent review**
   Use Claude-Patent-Creator slash command: `/full-review`
   Provide the path to `inventions/<slug>/04_draft/<latest-draft>.md`

2. **Claims analysis**
   Use `/review-claims` — checks independent claims for:
   - Antecedent basis errors
   - 35 USC 112(b) definiteness
   - Functional claim language (§ 112(f) mean-plus-function traps)
   - Overly broad vs. prior art

3. **Specification review**
   Use `/review-specification` — checks:
   - 35 USC 112(a) enablement
   - Written description adequacy
   - Best mode disclosure

4. **35 USC 101 eligibility**
   Apply Alice/Mayo two-step analysis manually:
   - Step 1: Is the claim directed to an abstract idea / natural phenomenon / law of nature?
   - Step 2A prong 2: Does the claim integrate into a practical application?
   - Step 2B: Does the claim add significantly more than the exception?
   Flag all software claims that lack a concrete technical improvement anchor.

5. **Output handoff package**
   Save to `inventions/<slug>/06_filing_package/attorney_handoff_<date>.md`
   Include: draft, QC report, prior art summary, § 101 analysis, open issues list.

## To Install Real Skill

```bash
# Visit mcpmarket.com/tools/skills/patent-lawyer-agent in a browser
# Download the skill manifest and save as manifest.json here
# Then register per mcpmarket instructions
curl -fsSL "https://mcpmarket.com/tools/skills/patent-lawyer-agent" \
  -o /Users/zack/claude-agent/workspace/skills/patent-lawyer-agent/manifest.json
```
