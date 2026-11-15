#!/usr/bin/env bash
# A pchi-bench agent: Claude Code, run headless, with PCHI's MCP server as its only way to touch the show.
#
#   pchi-bench --agent-cmd bench/agents/claude.sh --repeat 5 --evidence runs/claude-sonnet-1
#
# pchi-bench gives each run PCHI_RUNTIME (HOST:PORT, through the recording proxy), PCHI_TASK, PCHI_AGENT_NAME,
# PCHI_SCENARIO and PCHI_USAGE_FILE. This script starts one Claude Code session on the task and leaves what it cost in
# PCHI_USAGE_FILE (Claude Code's own JSON result, which also carries its final answer).
#
# Settings, all optional:
#   PCHI_CLAUDE_BIN       the claude command (default: claude)
#   PCHI_MCP_BIN          the pchi-mcp command (default: pchi-mcp)
#   PCHI_MODEL            a model to pass with --model (also recorded in the evidence)
#   PCHI_MAX_TURNS        turns the agent may take (default 20)
#   PCHI_PROMPT_FILE      text appended to the system prompt: the thing you are comparing between runs
#   PCHI_CLAUDE_EXTRA     extra arguments for claude, as one string
#
# NOT VERIFIED against a particular Claude Code version: the flags below (-p, --output-format json, --mcp-config,
# --strict-mcp-config, --allowedTools, --disallowedTools, --max-turns, --append-system-prompt, --model) are the ones
# Claude Code documents, written without a copy to try them on. Run `claude --help`, and run one scenario by hand
# (`pchi-bench --only move-cue --agent-cmd bench/agents/claude.sh`) before relying on a result.
set -euo pipefail

: "${PCHI_RUNTIME:?pchi-bench sets PCHI_RUNTIME: run this through pchi-bench --agent-cmd}"
: "${PCHI_TASK:?pchi-bench sets PCHI_TASK}"
name="${PCHI_AGENT_NAME:-claude}"
claude_bin="${PCHI_CLAUDE_BIN:-claude}"
mcp_bin="${PCHI_MCP_BIN:-pchi-mcp}"

if ! command -v "$claude_bin" >/dev/null 2>&1; then
  echo "cannot find '$claude_bin': install Claude Code, or set PCHI_CLAUDE_BIN" >&2
  exit 127
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# the agent's only tool server: pchi-mcp, speaking to the runtime as agent:<name>
cat > "$work/mcp.json" <<JSON
{"mcpServers":{"pchi":{"command":"$mcp_bin","args":["--runtime","$PCHI_RUNTIME","--name","$name"]}}}
JSON

args=(-p "$PCHI_TASK"
  --output-format json
  --mcp-config "$work/mcp.json" --strict-mcp-config
  --allowedTools "mcp__pchi"
  --disallowedTools "Bash Edit Write Read Glob Grep WebFetch WebSearch Task NotebookEdit"
  --max-turns "${PCHI_MAX_TURNS:-20}")
[ -n "${PCHI_MODEL:-}" ] && args+=(--model "$PCHI_MODEL")
[ -n "${PCHI_PROMPT_FILE:-}" ] && args+=(--append-system-prompt "$(cat "$PCHI_PROMPT_FILE")")
# shellcheck disable=SC2206
[ -n "${PCHI_CLAUDE_EXTRA:-}" ] && args+=($PCHI_CLAUDE_EXTRA)

# a throwaway working folder, so the session has nothing of yours to read even if a tool restriction fails
cd "$work"
out="$work/result.json"
status=0
"$claude_bin" "${args[@]}" > "$out" 2> "$work/stderr.txt" || status=$?

# hand the result to pchi-bench: usage and the final answer both come from this JSON
if [ -n "${PCHI_USAGE_FILE:-}" ] && [ -s "$out" ]; then cp "$out" "$PCHI_USAGE_FILE"; fi
if [ "$status" -ne 0 ]; then
  echo "claude exited with status $status: $(head -c 400 "$work/stderr.txt")" >&2
  exit "$status"
fi
