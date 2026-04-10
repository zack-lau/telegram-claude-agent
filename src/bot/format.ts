/**
 * Convert GitHub-flavored Markdown (from Claude) to Telegram HTML.
 *
 * Telegram HTML supports:
 *   <b>, <i>, <u>, <s>, <code>, <pre>, <a href="">,
 *   <blockquote>, <tg-spoiler>, <tg-emoji>
 *
 * Not supported: tables, images, headings (we fake them with bold).
 */

/** Escape HTML special chars in text that isn't already inside a tag. */
function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

export function markdownToTelegramHtml(md: string): string {
  // Work line-by-line for block elements, then apply inline formatting
  const lines = md.split("\n");
  const out: string[] = [];
  let inCodeBlock = false;
  let codeLang = "";
  let codeLines: string[] = [];
  let inBlockquote = false;
  let bqLines: string[] = [];
  let inTable = false;
  let tableRows: string[][] = [];

  function flushTable() {
    if (tableRows.length === 0) { inTable = false; return; }
    // Calculate column widths
    const colCount = Math.max(...tableRows.map((r) => r.length));
    const widths: number[] = [];
    for (let i = 0; i < colCount; i++) {
      widths.push(Math.max(...tableRows.map((r) => (r[i] ?? "").length)));
    }
    // Build aligned rows — first row is header (bold not possible inside pre, so use CAPS-ish as-is)
    const formatted = tableRows.map((row, ri) => {
      const cells = [];
      for (let i = 0; i < colCount; i++) {
        cells.push((row[i] ?? "").padEnd(widths[i]));
      }
      const line = cells.join("  │  ");
      // Add separator after header row
      if (ri === 0) {
        const sep = widths.map((w) => "─".repeat(w)).join("──┼──");
        return `${escapeHtml(line)}\n${sep}`;
      }
      return escapeHtml(line);
    });
    out.push(`<pre>${formatted.join("\n")}</pre>`);
    tableRows = [];
    inTable = false;
  }

  function flushBlockquote() {
    if (bqLines.length > 0) {
      out.push(`<blockquote>${applyInline(bqLines.join("\n"))}</blockquote>`);
      bqLines = [];
      inBlockquote = false;
    }
  }

  for (const rawLine of lines) {
    // ── Fenced code blocks ──
    if (rawLine.trimStart().startsWith("```")) {
      if (!inCodeBlock) {
        flushBlockquote();
        inCodeBlock = true;
        codeLang = rawLine.trimStart().slice(3).trim();
        codeLines = [];
      } else {
        // Close code block
        const escaped = escapeHtml(codeLines.join("\n"));
        if (codeLang) {
          out.push(`<pre><code class="language-${escapeHtml(codeLang)}">${escaped}</code></pre>`);
        } else {
          out.push(`<pre>${escaped}</pre>`);
        }
        inCodeBlock = false;
        codeLang = "";
      }
      continue;
    }

    if (inCodeBlock) {
      codeLines.push(rawLine);
      continue;
    }

    // ── Blockquotes ──
    if (rawLine.startsWith("> ") || rawLine === ">") {
      inBlockquote = true;
      bqLines.push(rawLine.replace(/^>\s?/, ""));
      continue;
    } else if (inBlockquote) {
      flushBlockquote();
    }

    // ── Table rows — collect and flush as a block ──
    if (rawLine.trimStart().startsWith("|") && rawLine.trimEnd().endsWith("|")) {
      if (!inTable) {
        flushBlockquote();
        inTable = true;
        tableRows = [];
      }
      // Skip separator rows (|---|---|)
      if (/^\|[\s:|-]+\|$/.test(rawLine.trim())) continue;
      // Parse cells
      const cells = rawLine.split("|").slice(1, -1).map((c) => c.trim());
      tableRows.push(cells);
      continue;
    } else if (inTable) {
      flushTable();
    }

    // ── Headings → bold ──
    const headingMatch = rawLine.match(/^(#{1,6})\s+(.*)/);
    if (headingMatch) {
      out.push(`\n<b>${applyInline(escapeHtml(headingMatch[2]))}</b>`);
      continue;
    }

    // ── Horizontal rules ──
    if (/^[-*_]{3,}\s*$/.test(rawLine)) {
      out.push("—————");
      continue;
    }

    // ── Unordered list items ──
    const ulMatch = rawLine.match(/^(\s*)[-*+]\s+(.*)/);
    if (ulMatch) {
      const indent = Math.floor(ulMatch[1].length / 2);
      const prefix = indent > 0 ? "  ".repeat(indent) + "◦ " : "• ";
      out.push(`${prefix}${applyInline(escapeHtml(ulMatch[2]))}`);
      continue;
    }

    // ── Ordered list items ──
    const olMatch = rawLine.match(/^(\s*)(\d+)[.)]\s+(.*)/);
    if (olMatch) {
      const indent = Math.floor(olMatch[1].length / 2);
      const prefix = "  ".repeat(indent);
      out.push(`${prefix}${olMatch[2]}. ${applyInline(escapeHtml(olMatch[3]))}`);
      continue;
    }

    // ── Regular line ──
    out.push(applyInline(escapeHtml(rawLine)));
  }

  // Close unclosed code block
  if (inCodeBlock && codeLines.length > 0) {
    const escaped = escapeHtml(codeLines.join("\n"));
    out.push(`<pre>${escaped}</pre>`);
  }

  flushTable();
  flushBlockquote();

  return out
    .join("\n")
    .replace(/\n{3,}/g, "\n\n") // collapse excessive newlines
    .trim();
}

/**
 * Apply inline markdown formatting to already-escaped HTML text.
 * Order matters — process code spans first to avoid formatting inside them.
 */
function applyInline(text: string): string {
  // Inline code (must be first — don't format inside code)
  text = text.replace(/`([^`]+)`/g, "<code>$1</code>");

  // Bold+italic (***text*** or ___text___)
  text = text.replace(/\*{3}(.+?)\*{3}/g, "<b><i>$1</i></b>");

  // Bold (**text** or __text__)
  text = text.replace(/\*{2}(.+?)\*{2}/g, "<b>$1</b>");
  text = text.replace(/__(.+?)__/g, "<b>$1</b>");

  // Italic (*text* or _text_) — avoid matching mid-word underscores
  text = text.replace(/\*(.+?)\*/g, "<i>$1</i>");
  text = text.replace(/(?<!\w)_(.+?)_(?!\w)/g, "<i>$1</i>");

  // Strikethrough (~~text~~)
  text = text.replace(/~~(.+?)~~/g, "<s>$1</s>");

  // Links [text](url)
  text = text.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2">$1</a>');

  return text;
}
