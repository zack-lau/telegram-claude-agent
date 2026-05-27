# Prostec Labs — Brand Reference

Version 1.0 · Singapore · 2025

This document is the canonical reference for Prostec Labs visual identity in
code. It mirrors the brand sheet. When in doubt, this file wins.

---

## 1. Voice

> Prostec Labs builds quiet instruments for the body — software that listens
> before it acts, and acts with the precision of a surgeon's hand.

| Attribute | Meaning |
| --- | --- |
| **Calm** | Never raises voice. The work speaks louder than the marketing. |
| **Direct** | Plain words for difficult ideas. No jargon, no hedging. |
| **Crafted** | Every detail considered. Quiet evidence of care. |
| **Humanist** | Built around bodies and lives, not metrics. |

Sounds like: Aesop · Calm · Headspace
Never sounds like: Cyberpunk · Web3 · SaaS

---

## 2. Colour

Three colours. Use them in roughly **60 / 30 / 10**.

| Token | Hex | OKLCH | Role |
| --- | --- | --- | --- |
| `--ink` | `#15140F` | `oklch(0.18 0.005 90)` | Primary · 60% |
| `--bone` | `#F2EDE4` | `oklch(0.94 0.012 80)` | Ground · 30% |
| `--sage` | `#6B8772` | `oklch(0.55 0.04 145)` | Accent · 10% |
| `--paper` | `#EFECE5` | `oklch(0.93 0.011 80)` | Subtly darker bone, used between cards |

**Sage rules.** Sage appears in moments of warmth — a single dot in a chart, a
section header, a piece of stationery. Never as a button colour. Never as a
large surface fill. Never with Ink directly behind it (contrast is too low).

**Allowed pairings.** Ink on Bone (default), Bone on Ink (inverted), Sage on
Bone, Bone on Sage. Avoid Sage on Ink.

---

## 3. Typography

Three families.

| Family | Weights | Role |
| --- | --- | --- |
| **Instrument Sans** | 400, 500, 600 | UI, lockups, signage — almost everything |
| **Instrument Serif** | Italic only | Editorial accents, pull-quotes, page numerals |
| **JetBrains Mono** | 400, 500 | Captions, metadata, timestamps, technical labels |

Load via Google Fonts:

```html
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Instrument+Sans:wght@400;500;600&family=Instrument+Serif:ital@0;1&family=JetBrains+Mono:wght@400;500&display=swap">
```

### Type scale

| Role | Size / Leading | Family | Tracking |
| --- | --- | --- | --- |
| Display | 96 / 115 | Instrument Sans 500 | −0.03em |
| Headline | 56 / 67 | Instrument Sans 500 | −0.02em |
| Title | 32 / 38 | Instrument Sans 500 | −0.01em |
| Editorial | 28 / 36 | Instrument Serif Italic | 0 |
| Body | 16 / 26 | Instrument Sans 400 | 0 |
| Caption | 10 / 14 | JetBrains Mono 500 | +0.22em, UPPERCASE |

**Rules.** Never `text-transform: uppercase` on Instrument Sans or Serif —
only on JetBrains Mono captions. Never italic on Instrument Sans. Never roman
on Instrument Serif. Body line-length max 68 characters.

---

## 4. The wordmark

Two syllables stacked on left-aligned baselines. Letter-spacing −2%. Caption
"labs" sits below at 16% of cap height, JetBrains Mono, tracked +24%.

```
pro
stec
LABS
```

**Use the SVG.** Always import from `assets/brand/wordmark.svg` or the
`<Wordmark />` component. Never re-typeset.

| Rule | Value |
| --- | --- |
| Clear space | 1 × cap height on all sides |
| Minimum size — full lockup | 32 px on screen, 12 mm in print |
| Minimum size — mark only ("labs" dropped) | 16 px (favicon use) |
| Allowed colours | Ink, Bone, Sage |

---

## 5. App icon

Tight crop of the wordmark inside an Ink tile. Type fills 40% of tile width.
Inner padding 10% top/bottom, 12% sides.

Corner radius follows platform:

| Platform | Radius |
| --- | --- |
| iOS | 23% of tile |
| macOS | continuous (squircle) |
| Web / Android | 12–16% |

Variants:
- **Primary** — Bone type on Ink tile (default)
- **Light** — Ink type on Bone tile (light surfaces only)
- **Sage** — Bone type on Sage tile (seasonal / stationery only)

---

## 6. Layout, spacing, motion

| Token | Value |
| --- | --- |
| `--space-1` | 4 px |
| `--space-2` | 8 px |
| `--space-3` | 12 px |
| `--space-4` | 16 px |
| `--space-5` | 24 px |
| `--space-6` | 32 px |
| `--space-7` | 48 px |
| `--space-8` | 64 px |
| `--space-9` | 96 px |
| `--radius-sm` | 4 px (buttons, inputs) |
| `--radius-md` | 8 px (cards) |
| `--radius-lg` | 16 px (modal, sheet) |
| `--radius-tile` | 23% (app icon — relative) |
| `--shadow-1` | `0 1px 3px rgba(21,20,15,0.06), 0 1px 2px rgba(21,20,15,0.04)` |
| `--shadow-2` | `0 8px 24px -8px rgba(21,20,15,0.18), 0 2px 4px rgba(21,20,15,0.06)` |
| `--shadow-3` | `0 24px 60px -28px rgba(21,20,15,0.35), 0 2px 4px rgba(21,20,15,0.06)` |
| Motion easing | `cubic-bezier(0.22, 0.61, 0.36, 1)` — quiet, like a closing drawer |
| Motion duration | 180 ms (micro), 320 ms (panel), 600 ms (page) |

Borders: always `1px solid rgba(21,20,15,0.12)` on bone surfaces, or
`1px solid rgba(242,237,228,0.12)` on ink surfaces.

---

## 7. Component patterns

### Buttons

- **Primary** — Ink fill, Bone text, `--radius-sm`, 12 px / 20 px padding.
  Hover: 92% lightness on Ink. Active: 88%.
- **Secondary** — Ink 1 px outline, Ink text, transparent fill.
- **Tertiary** — text only, Ink, underline on hover.
- Label is Instrument Sans 500, 14 px, tracking 0.
- Never use Sage as button background.

### Inputs

- Bone fill, Ink 1 px bottom border. No full-box border.
- Label is Mono caption above the field.
- Focus: bottom border thickens to 2 px and shifts to Sage.

### Cards

- Bone fill, `1px solid rgba(21,20,15,0.12)`, `--radius-md`, `--shadow-1`.
- Use `--paper` background behind groups of cards for separation.

### Captions, metadata, badges

- All Mono, all 10 px, all `letter-spacing: 0.22em`, all UPPERCASE.

---

## 8. Don'ts

- ✗ No gradient backgrounds.
- ✗ No drop shadows beyond the three defined shadow tokens.
- ✗ No emoji in product copy.
- ✗ No new accent colours, ever.
- ✗ No re-typeset wordmark (use the SVG).
- ✗ No marketing tropes ("supercharge", "unleash", "next-gen", "AI-powered").
- ✗ No uppercase on Instrument Sans or Serif.
- ✗ No italic on Instrument Sans.
- ✗ No Sage-on-Ink, no thin Sage type at body sizes.

---

## 9. Colophon

Typefaces: Instrument Sans, Instrument Serif, JetBrains Mono.
Contact: design@prostec.ai · prostec.ai/brand
Version: v 1.0 — 26 May 2025 — Singapore
