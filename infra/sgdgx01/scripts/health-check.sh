#!/usr/bin/env bash
# health-check.sh — Full DGX service health check
# Checks all active services, sends Telegram alert on any failure
# Exit: 0=healthy, 1=degraded, 2=critical

set -uo pipefail
# PIPESTATUS workaround: SSE+head pipelines always produce SIGPIPE; ignore curl exit when head succeeds
sse_first_line() { curl -sf --max-time 5 "$1" 2>/dev/null | head -1 || true; }

source /home/agents/.secrets/telegram_notify.sh 2>/dev/null || true

MCP_TOKEN="${MCP_TOKEN:-}"
OVERALL=0
RESULTS=()

ok()       { RESULTS+=("✅  ${1}  ${2:-}"); }
warn()     { RESULTS+=("⚠️   ${1}  ${2:-}"); [[ $OVERALL -lt 1 ]] && OVERALL=1; }
critical() { RESULTS+=("🔴  ${1}  ${2:-}"); [[ $OVERALL -lt 2 ]] && OVERALL=2; }

http_ok() {
    local code
    code=$(curl -sf --connect-timeout 5 --max-time 10 -o /dev/null -w "%{http_code}" "$1" 2>/dev/null) || code="000"
    [[ "$code" =~ ^2 ]]
}

# ── 1. embed (8001) ──────────────────────────────────────────────────────────
dims=$(curl -sf --max-time 10 http://localhost:8001/v1/embeddings     -H 'Content-Type: application/json'     -d '{"model":"bge-m3","input":"health probe"}' 2>/dev/null     | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["data"][0]["embedding"]))' 2>/dev/null) || dims=0
[[ "${dims:-0}" -gt 0 ]] && ok "embed" "(${dims} dims)" || critical "embed" "DOWN"

# ── 2. rerank (8002) ─────────────────────────────────────────────────────────
rerank_code=$(curl -sf --max-time 10 -o /dev/null -w "%{http_code}"     -X POST http://localhost:8002/v1/rerank     -H 'Content-Type: application/json'     -d '{"model":"bge-reranker-v2-m3","query":"test","documents":["hello"]}' 2>/dev/null) || rerank_code="000"
[[ "$rerank_code" == "200" ]] && ok "rerank" || critical "rerank" "DOWN (HTTP ${rerank_code})"

# ── 2b. nemotron (8000) ──────────────────────────────────────────────────────
nem_code=$(curl -sf --connect-timeout 5 --max-time 10 -o /dev/null -w "%{http_code}" http://localhost:8000/health 2>/dev/null) || nem_code="000"
[[ "$nem_code" == "200" ]] && ok "nemotron" "(256k ctx)" || critical "nemotron" "DOWN (HTTP ${nem_code})"

# ── 3. mcp-memory / lancedb (5282) ───────────────────────────────────────────
mem_resp=$(sse_first_line http://localhost:5282/sse)
[[ "$mem_resp" == *"endpoint"* ]] && ok "mcp-memory" "(SSE ready)" || critical "mcp-memory" "DOWN"

# ── 4. qmd-mcp (8181) ────────────────────────────────────────────────────────
qmd_resp=$(curl -sf --max-time 5 http://localhost:8181/mcp     -H 'Content-Type: application/json'     -H 'Accept: application/json, text/event-stream'     -d '{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"health","version":"1"}},"id":1}' 2>/dev/null) || qmd_resp=""
[[ "$qmd_resp" == *"serverInfo"* ]] && ok "qmd-mcp" || critical "qmd-mcp" "DOWN"

# ── 6. whisper-server (8007) ─────────────────────────────────────────────────
whisper_st=$(systemctl is-active whisper-server 2>/dev/null) || whisper_st="unknown"
[[ "$whisper_st" == "active" ]] && ok "whisper" || warn "whisper" "(${whisper_st})"

# ── 11. minio (9000) ─────────────────────────────────────────────────────────
http_ok "http://localhost:9000/minio/health/live" && ok "minio" || critical "minio" "DOWN"

# ── 11b. searxng (8080) ──────────────────────────────────────────────────────
http_ok "http://192.168.22.3:8080/" && ok "searxng" || warn "searxng" "DOWN"

# ── 12. storage ──────────────────────────────────────────────────────────────
used_pct=$(df /home/agents 2>/dev/null | awk 'NR==2{gsub(/%/,"",$5);print $5}') || used_pct=0
free=$(df -h /home/agents 2>/dev/null | awk 'NR==2{print $4}') || free="?"
if [[ "${used_pct:-0}" -ge 90 ]]; then critical "storage" "${free} free (${used_pct}% used)"
elif [[ "${used_pct:-0}" -ge 80 ]]; then warn "storage" "${free} free (${used_pct}% used)"
else ok "storage" "${free} free (${used_pct}% used)"
fi

# ── 13. swap ─────────────────────────────────────────────────────────────────
swap_used_mb=$(free -m | awk '/Swap/{print $3}') || swap_used_mb=0
swap_total_mb=$(free -m | awk '/Swap/{print $2}') || swap_total_mb=1
swap_pct=$(( swap_used_mb * 100 / (swap_total_mb + 1) ))
swap_h=$(free -h | awk '/Swap/{print $3}')
if [[ $swap_pct -ge 80 ]]; then critical "swap" "${swap_h} used (${swap_pct}%)"
elif [[ $swap_pct -ge 60 ]]; then warn "swap" "${swap_h} used (${swap_pct}%)"
else ok "swap" "${swap_h} used (${swap_pct}%)"
fi

# ── Output ────────────────────────────────────────────────────────────────────
case $OVERALL in
    0) header="✅ DGX HEALTHY" ;;
    1) header="⚠️  DGX DEGRADED" ;;
    2) header="🔴 DGX CRITICAL" ;;
esac

echo "$header  [$(date '+%Y-%m-%d %H:%M')]"
echo "─────────────────────────────────────"
for r in "${RESULTS[@]}"; do echo "$r"; done

if [[ $OVERALL -gt 0 ]] && declare -f send_telegram &>/dev/null; then
    failures=$(printf '%s\n' "${RESULTS[@]}" | grep -E '^(⚠️|🔴)' | sed 's/^[^ ]* //' | tr '\n' ', ' | sed 's/, $//')
    send_telegram "${header}: ${failures}" || true
fi

exit $OVERALL
