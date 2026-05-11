#!/usr/bin/env python3
"""
Adversarial code review via direct vLLM API (Qwen3.6 on sgdgx01).
Runs ON sgdgx01 — invoke via SSH from local machine.

Usage (from local machine):
  cat file.py        | ssh sgdgx01 "python3 ~/trading/scripts/qwen_review.py --stdin --name file.py"
  git diff           | ssh sgdgx01 "python3 ~/trading/scripts/qwen_review.py --stdin --diff"
  git diff HEAD~1    | ssh sgdgx01 "python3 ~/trading/scripts/qwen_review.py --stdin --diff"

Usage (on sgdgx01 directly):
  python3 ~/trading/scripts/qwen_review.py <path/to/file.py>
  python3 ~/trading/scripts/qwen_review.py --reasoning <path/to/file.py>
"""

import argparse
import os
import re
import signal
import subprocess
import sys
from pathlib import Path

signal.signal(signal.SIGPIPE, signal.SIG_DFL)  # clean exit on broken SSH pipe

try:
    import openai
except ImportError:
    print("Error: openai SDK not found. Run: /home/agents/perplexity-mcp/.venv/bin/pip install openai", file=sys.stderr)
    sys.exit(1)

BASE_URL = os.environ.get("OPENAI_BASE_URL", "http://localhost:8000/v1")
MODEL = "qwen3.6"
MAX_FILE_BYTES = 512 * 1024
MAX_TOKENS = 32000  # thinking tokens count against budget; 32K gives room for both reasoning + review
# 1800s (30 min) — dense bundles (e.g. 60 KB of crypto code) make Qwen3.6-thinking
# generate ~13K reasoning tokens at ~33 tok/s (≈ 7 min solo). Under concurrent batching
# per-sequence throughput drops; 600s used to time out the last-in-batch request
# while vLLM was still actively generating. 1800s gives ~5× headroom over solo-time.
TIMEOUT = 1800

SYSTEM_PROMPT = """\
You are conducting a fresh adversarial review. Play TWO roles simultaneously and be ruthlessly honest:

**Role 1 — Attacker / Security Auditor**
Hunt for: injection (SQL/cmd/path), authentication bypasses, authorization gaps, secrets in code, \
insecure deserialization, SSRF, prototype pollution, race conditions, timing attacks, data exposure.

**Role 2 — Reliability Engineer**
Hunt for: uncaught exceptions, resource leaks (file handles, connections, memory), \
undefined/null dereferences, off-by-one errors, missing error propagation, \
silent failures, incorrect timeout/retry logic, concurrent mutation without locks.

---

**Output format (use exactly this structure):**

## Findings

For each issue:
- **[CRITICAL/HIGH/MEDIUM/LOW]** `file:line` — one-line description
  - *How it manifests*: ...
  - *Fix*: ...

## Overall Assessment
2 sentences max. Is this safe to ship?

## Top 3 Priorities
Ordered list of the most important fixes.

---

Begin the adversarial review now. Be specific — cite exact line numbers where possible.
Do NOT use any tools. Do NOT read files. Do NOT run shell commands.
Write your complete structured review based solely on the code provided.
Respond with the ## Findings section immediately.\
"""

RETRY_SUFFIX = "\n\nRespond concisely — keep your analysis brief and focus on the structured output format."


def escape_fences(text: str) -> str:
    return text.replace("```", r"\`\`\`")


def get_git_diff() -> tuple[str, str]:
    def run(cmd) -> str | None:
        """Return diff text, "" if not applicable, None on oversized output."""
        try:
            result = subprocess.run(cmd, capture_output=True, timeout=10, check=True)
            raw = result.stdout
            if len(raw) > MAX_FILE_BYTES:
                return None  # caller will exit with an error
            return raw.decode(errors="replace").strip()
        except subprocess.CalledProcessError as e:
            err = e.stderr.decode(errors="replace").strip()
            if e.returncode != 128:  # 128 = not a git repo
                print(f"  warning: {' '.join(cmd)}: {err}", file=sys.stderr)
            return ""
        except subprocess.TimeoutExpired:
            print(f"  warning: {' '.join(cmd)}: timed out after 10s", file=sys.stderr)
            return ""
        except OSError as e:
            print(f"  warning: {' '.join(cmd)}: {e}", file=sys.stderr)
            return ""

    staged = run(["git", "diff", "--cached"])
    unstaged = run(["git", "diff"])
    if staged is None or unstaged is None:
        print(f"Error: git diff output too large (>{MAX_FILE_BYTES // 1024} KB)", file=sys.stderr)
        sys.exit(1)
    diff = "\n\n".join(filter(None, [staged, unstaged]))
    if diff:
        if len(diff.encode()) > MAX_FILE_BYTES:
            print(f"Error: combined git diff too large (>{MAX_FILE_BYTES // 1024} KB)", file=sys.stderr)
            sys.exit(1)
        return diff, "working tree"

    last_commit = run(["git", "diff", "HEAD~1", "HEAD"])
    if last_commit is None:
        print(f"Error: git diff output too large (>{MAX_FILE_BYTES // 1024} KB)", file=sys.stderr)
        sys.exit(1)
    if last_commit:
        return last_commit, "last commit (HEAD~1..HEAD)"

    return "", "none"


def build_prompt(args) -> str:
    if args.stdin:
        if sys.stdin.isatty():
            print("Error: --stdin requires piped input, not an interactive terminal", file=sys.stderr)
            sys.exit(1)
        raw = sys.stdin.buffer.read(MAX_FILE_BYTES + 1)
        if len(raw) > MAX_FILE_BYTES:
            print(f"Error: stdin too large (>{MAX_FILE_BYTES // 1024} KB)", file=sys.stderr)
            sys.exit(1)
        content = raw.decode(errors="replace")
        if not content.strip():
            print("Error: stdin is empty", file=sys.stderr)
            sys.exit(1)
        if args.diff:
            return f"**Changes to review:**\n```diff\n{escape_fences(content)}\n```\n"
        label = args.name or "stdin"
        return f"**File: {label}**\n```\n{escape_fences(content)}\n```\n"

    if args.file:
        path = Path(args.file)
        try:
            size = path.stat().st_size
            if size > MAX_FILE_BYTES:
                print(f"Error: file too large ({size // 1024} KB, max {MAX_FILE_BYTES // 1024} KB)", file=sys.stderr)
                sys.exit(1)
            content = path.read_text(errors="replace")
        except OSError as e:
            print(f"Error: cannot read {path}: {e}", file=sys.stderr)
            sys.exit(1)
        return f"**File: {path.name}**\n```\n{escape_fences(content)}\n```\n"

    # fallback: git diff
    diff, source = get_git_diff()
    if diff:
        return f"**Changes to review ({source}):**\n```diff\n{escape_fences(diff)}\n```\n"

    print("Error: no file or stdin content. Pass a file path, use --stdin, or stage changes.", file=sys.stderr)
    sys.exit(1)


def extract_review_from_reasoning(reasoning: str) -> str | None:
    """If the model put the structured review inside its reasoning, extract it."""
    match = re.search(r"(## Findings.*)", reasoning, re.DOTALL)
    if match:
        return match.group(1).strip()
    return None


def call_vllm(client, messages, attempt: int = 1, no_think: bool = False) -> tuple[str, str]:
    """Call vLLM and return (content, reasoning). Retries once on empty content."""
    print(f"Waiting for vLLM response (attempt {attempt})...", file=sys.stderr, flush=True)

    kwargs: dict = dict(
        model=MODEL,
        messages=messages,
        max_tokens=MAX_TOKENS,
        temperature=0.2,
        timeout=TIMEOUT,
    )
    if no_think:
        kwargs["extra_body"] = {"chat_template_kwargs": {"enable_thinking": False}}

    try:
        response = client.chat.completions.create(**kwargs)
    except Exception as e:
        print(f"Error calling vLLM: {e}", file=sys.stderr)
        sys.exit(1)

    if not response.choices:
        print("Error: empty choices in response from vLLM", file=sys.stderr)
        sys.exit(1)
    msg = response.choices[0].message
    if msg is None:
        print("Error: null message in response from vLLM", file=sys.stderr)
        sys.exit(1)

    content = msg.content or ""
    reasoning = (getattr(msg, "reasoning_content", None)
                 or getattr(msg, "reasoning", None)
                 or "")

    # Retry once if content is empty (model spent all tokens on thinking)
    if not content.strip() and attempt == 1:
        print("WARNING: empty content on first attempt, retrying with concise prompt...", file=sys.stderr, flush=True)
        # Append conciseness instruction to the last user message
        retry_messages = list(messages)
        retry_messages[-1] = {
            "role": retry_messages[-1]["role"],
            "content": retry_messages[-1]["content"] + RETRY_SUFFIX,
        }
        return call_vllm(client, retry_messages, attempt=2, no_think=no_think)

    return content, reasoning


def main():
    parser = argparse.ArgumentParser(description="Adversarial code review via Qwen3.6")
    parser.add_argument("file", nargs="?", help="File to review (runs on sgdgx01 directly)")
    parser.add_argument("--stdin", action="store_true", help="Read code/diff from stdin (for SSH pipe)")
    parser.add_argument("--diff", action="store_true", help="Treat stdin as a git diff (use with --stdin)")
    parser.add_argument("--name", help="Filename label when reading from stdin")
    parser.add_argument("--reasoning", action="store_true", help="Print thinking trace to stderr")
    parser.add_argument("--no-think", dest="no_think", action="store_true", help="Disable extended thinking (faster, avoids token-budget loops)")
    args = parser.parse_args()

    if args.stdin and args.file:
        parser.error("--stdin and a file argument are mutually exclusive")
    if args.diff and not args.stdin:
        parser.error("--diff requires --stdin")

    context = build_prompt(args)

    client = openai.OpenAI(base_url=BASE_URL, api_key="dummy")  # localhost vLLM — no auth needed

    messages = [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": context},
    ]

    content, reasoning = call_vllm(client, messages, no_think=args.no_think)

    if args.reasoning and reasoning:
        print("=== THINKING ===", file=sys.stderr)
        print(reasoning, file=sys.stderr)
        print("=== END THINKING ===\n", file=sys.stderr)

    if content.strip():
        print(content)
    elif reasoning:
        # Try to extract structured review from reasoning
        extracted = extract_review_from_reasoning(reasoning)
        if extracted:
            print(extracted)
            print("\nWARNING: review was extracted from model reasoning (content was empty)", file=sys.stderr, flush=True)
        else:
            # Fallback: dump reasoning as output
            print(reasoning)
            print("\nWARNING: response was in reasoning not content — thinking consumed token budget", file=sys.stderr, flush=True)
    else:
        print("Error: empty response from model after retry", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
