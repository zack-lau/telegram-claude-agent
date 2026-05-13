#!/usr/bin/env bash
# Usage: bash scripts/new-invention.sh <slug>
set -euo pipefail
SLUG="${1:?Usage: new-invention.sh <slug>}"
BASE="inventions/$SLUG"
mkdir -p "$BASE"/{01_source_refs,03_prior_art,04_draft,05_qc_reports,06_filing_package}
mkdir -p "$BASE/02_disclosure"
cat > "$BASE/02_disclosure/disclosure.md" << EOF
# Invention Disclosure — <TITLE>

**Inventor(s):**
**Date:** $(date +%Y-%m-%d)
**Slug:** $SLUG

## 1. Executive Summary

## 2. Problem Solved

## 3. What It Does and How

## 4. Novelty Over Prior Art

## 5. Pseudocode / Algorithm

## 6. Data Structures

## 7. Alternatives Considered

## 8. Potential Prior Art Concerns

## 9. Claims Sketch (independent claims)

## 10. Figures Needed
EOF
echo "Scaffolded $BASE/"
ls "$BASE/"
