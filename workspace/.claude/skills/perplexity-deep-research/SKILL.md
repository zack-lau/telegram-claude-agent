---
name: perplexity-deep-research
description: Run Perplexity Pro Deep Research via Chrome CDP browser automation. Use this skill whenever the user asks for deep research, thorough investigation, comprehensive analysis, or in-depth research on any topic — especially when Perplexity Sonar (the API) wouldn't be sufficient. Also use when the user explicitly mentions "deep research" or "perplexity deep research".
---

# Perplexity Deep Research

Run Perplexity Pro's Deep Research feature via Playwright MCP browser automation (Chrome CDP). This produces much more thorough, multi-source research reports than the Perplexity Sonar API — typically 3-8 minutes per query with cited sources.

**Quota is scarce (20 queries/day).** Always confirm with the user before starting a Deep Research query.

## When to Use

- User asks for **deep**, **thorough**, or **comprehensive** research on a topic
- Perplexity Sonar API (`mcp__perplexity__search`) wouldn't provide enough depth
- Need multi-source synthesis with citations (investment DD, technical reviews, competitive analysis)
- User explicitly asks for "deep research"

**Don't use when:** A quick factual lookup is sufficient — use `mcp__perplexity__search` with `detail: "detailed"` instead.

## Prerequisites

- Chrome running with remote debugging enabled (CDP)
- Playwright MCP tools available (`mcp__plugin_playwright_playwright__browser_*`)
- Perplexity Pro subscription (20 Deep Research queries/day)

## Preflight Checks

Before starting, verify the environment is ready:

1. **CDP connection** — Run `browser_navigate` to `https://perplexity.ai`. If it fails, Chrome CDP is not available. Stop and tell the user to launch Chrome with `--remote-debugging-port`.
2. **Login state** — After navigation, take a `browser_snapshot`. Look for signs of being logged out: "Sign in", "Log in", "Create account" buttons. If not logged in, stop and tell the user: "Perplexity needs to be logged in to use Deep Research. Please log in to perplexity.ai in Chrome and try again."
3. **User confirmation** — Before submitting the query, confirm: "This will use 1 of your 20 daily Deep Research queries. Proceed?"

If any preflight check fails, stop. Do not proceed.

## Execution Flow

Use Playwright MCP tools (`browser_*`) to drive the Perplexity web UI:

### Step 1: Navigate and verify

```
browser_navigate → https://perplexity.ai
browser_snapshot → verify logged in (no "Sign in" / "Log in" buttons)
```

### Step 2: Activate Deep Research mode

```
browser_snapshot → find the search textbox (look for textbox with "Ask anything" or similar placeholder)
browser_click → click the textbox
browser_type → type "/" into the textbox
browser_wait_for → wait 1-2s for the slash menu to appear
browser_snapshot → find "Deep Research" or "Deep research" menuitem/option
browser_click → click the Deep Research option
browser_wait_for → wait 1-2s for mode switch
```

If the `/` menu doesn't appear or "Deep Research" isn't listed, take a fresh snapshot and look for alternative UI paths (a mode picker, toggle, or dropdown near the input). If nothing works, stop and report the UI may have changed.

### Step 3: Submit the query

```
browser_snapshot → find the textbox again (element refs change after mode switch)
browser_fill_form → fill in the research query
browser_press_key → Enter
```

### Step 4: Poll for completion

Deep Research takes 3-8 minutes. Poll every 15-20 seconds:

```
browser_snapshot → check page state
```

**Still researching if** any of these appear in the snapshot: "Gathering", "Retrieving", "Synthesizing", "Analyzing", "Searching", "Compiling", "Evaluating", progress bars, step indicators.

**Quota exceeded if** any of these appear: "limit", "quota", "daily limit", "upgrade", "try again later", "unavailable".

**Done when ALL of these are true:**
- No progress/loading indicators visible
- Report content is present (look for a large text block with headings, paragraphs, citations)
- Confirmed stable across 2 consecutive polls (content didn't change between polls)

**Hard timeout:** 10 minutes. If not done, stop and report partial results or timeout.

### Step 5: Extract the report

Once done:

```
browser_snapshot → identify the report container element
browser_click → click the report element (to select/focus it)
```

Read the report text from the snapshot. If it appears truncated (no conclusion or summary at end):

```
browser_press_key → End (scroll to bottom)
browser_snapshot → get remaining content
```

Also extract source/citation URLs visible in the report.

### Step 6: Return results

Present to the user:
- **Query** — what was asked
- **Report** — full text of the research report
- **Sources** — list of cited URLs
- **Status** — ok, quota_exceeded, timeout, or error
- **Elapsed time**

## Biotech DD Shortcut

For biotech/pharma ticker research specifically, a dedicated script exists:

```bash
cd /Users/zack/claude-agent/projects/biotech-dd
npx tsx server/perplexity.ts TICKER [ASSET] [INDICATION] [PDUFA_DATE]
```

This uses `agent-browser` CLI directly and saves output to `{DD_BASE}/{TICKER}/raw/perplexity_research.json`. See `/Users/zack/claude-agent/projects/biotech-dd/server/perplexity.ts` for the reference implementation.

## Crafting Good Research Queries

Deep Research works best with structured, specific queries:

- **Subject** — what you're researching
- **Scope** — specific aspects to cover
- **Format hints** — "provide specific data points, dates, and citations"
- **Purpose** — helps Perplexity calibrate depth ("for investment due diligence", "for technical architecture decision")

Example:
```
Deep research on [TOPIC].

Cover:
1) [Aspect 1 with specifics]
2) [Aspect 2 with specifics]
3) [Aspect 3 with specifics]

Provide specific data points, dates, and citations. This is for [PURPOSE].
```

Batch related sub-questions into one query to conserve quota.

## Troubleshooting

| Issue | Fix |
|-------|-----|
| CDP connection fails | Chrome needs `--remote-debugging-port` flag |
| Sign-in wall | Log in to perplexity.ai in Chrome manually |
| `/` menu doesn't appear | Take snapshot, look for alternative mode picker UI |
| "Deep Research" not in menu | UI may have changed — look for toggles/dropdowns near input |
| Timeout after 10 min | Retry once; some queries legitimately take long |
| Quota exceeded | Wait until next day, or fall back to `mcp__perplexity__search` |
| Report appears truncated | Scroll to bottom and re-extract |
