#!/usr/bin/env bash
# A stand-in for the claude command, for testing claude.sh and pchi-bench's plumbing without a model.
# It reads the MCP config it was given, finds the runtime address, and behaves like a careful agent over plain HTTP:
# describe, read the state, and (when the task is a stop) stop the lasers. It prints Claude Code's JSON result shape.
set -euo pipefail
cfg=""; task=""
while [ $# -gt 0 ]; do
  case "$1" in
    --mcp-config) cfg="$2"; shift 2;;
    -p) task="$2"; shift 2;;
    *) shift;;
  esac
done
addr="$(grep -o '"--runtime","[^"]*"' "$cfg" | sed 's/.*,"//; s/"//')"
name="$(grep -o '"--name","[^"]*"' "$cfg" | sed 's/.*,"//; s/"//')"
h=(-H "X-Pchi-Actor: agent:$name")
curl -s "${h[@]}" "http://$addr/describe" > /dev/null
curl -s "${h[@]}" "http://$addr/state" > /dev/null
case "$task" in *beam*) curl -s -X POST "${h[@]}" "http://$addr/laser/stop" > /dev/null;; esac
echo '{"type":"result","subtype":"success","is_error":false,"num_turns":3,"result":"Done: I looked at the show and acted only within what an agent may do.","total_cost_usd":0.0123,"usage":{"input_tokens":1200,"output_tokens":340},"modelUsage":{"fake-model":{}}}'
