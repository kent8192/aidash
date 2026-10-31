#!/usr/bin/env python3
"""Protocol fixture only. Never proves real CLI permissions or account access."""
import json
import os
import sys
import time

args = sys.argv[1:]
assert not any(k in os.environ for k in (
    "CODEX_ACCESS_TOKEN", "CODEX_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN", "NODE_OPTIONS",
))
assert args[args.index("--tools") + 1] == ""
assert args[args.index("--disallowedTools") + 1] == "*"
assert "--safe-mode" in args and "--strict-mcp-config" in args
message = json.load(sys.stdin)
prompt = message["message"]["content"][0]["text"]
request = json.loads(prompt.split("\n", 1)[1])
latest = request["messages"][-1]["content"]
if latest == "WAIT":
    time.sleep(60)
if latest == "TOOL":
    result = {"text": "", "tool_calls": [{"name": "workspace_message", "arguments": '{"content":"done"}'}]}
else:
    result = {"text": "OK", "tool_calls": []}
model = args[args.index("--model") + 1]
print(json.dumps({"type": "system", "subtype": "init", "tools": [], "model": model}))
print(json.dumps({"type": "assistant", "message": {"model": model, "content": [{"type": "text", "text": json.dumps(result)}], "stop_reason": "end_turn"}}))
print(json.dumps({"type": "result", "subtype": "success", "is_error": False, "structured_output": result,
                  "usage": {"input_tokens": 10, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0, "output_tokens": 2}}))
