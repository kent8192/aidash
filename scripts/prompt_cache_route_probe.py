#!/usr/bin/env python3
"""Probe OpenRouter append-only tool conversations, without Aidash.

Dry run (default): python3 scripts/prompt_cache_route_probe.py --route MODEL@PROVIDER --max-spend-usd 1
Live: use --execute --output private/probe.json and OPENROUTER_API_KEY. The key
must have a non-resetting OpenRouter spending limit no greater than the cap and
include BYOK usage. Use a dedicated key: OpenRouter enforces the limit, including
concurrent or uncertain requests. No live call is made by default. --echo-upstream-body
requests and saves OpenRouter's debug echo, which may contain the whole conversation.
Repeat --route to compare routes. See https://openrouter.ai/docs/api/api-reference/api-keys/get-current-key
"""
import argparse
import copy
import json
import math
import os
from pathlib import Path
import sys
import urllib.error
import urllib.request

URL = "https://openrouter.ai/api/v1/"
SYSTEM = "You are a research assistant. Call read_page once per turn for pages 1 through 4."
TOOLS = [{"type": "function", "function": {"name": "read_page", "description": "Read an archive page",
          "parameters": {"type": "object", "properties": {"page": {"type": "integer"}}, "required": ["page"]}}}]


def request_body(model, provider, messages, echo):
    body = {"model": model, "messages": copy.deepcopy(messages), "tools": TOOLS, "tool_choice": "required",
            "max_tokens": 512, "reasoning": {"effort": "medium"}, "usage": {"include": True},
            "provider": {"only": [provider], "allow_fallbacks": False}}
    if echo:
        body["debug"] = {"echo_upstream_body": True}
    return body


def append_turn(messages, assistant, step):
    # Retain the returned reasoning_details value unchanged, including signatures and ordering.
    message = {key: copy.deepcopy(assistant[key]) for key in ("role", "content", "tool_calls", "reasoning_details") if key in assistant}
    if message.get("role") != "assistant" or not message.get("tool_calls"):
        raise ValueError("provider returned no assistant tool call")
    messages.append(message)
    for call in message["tool_calls"]:
        messages.append({"role": "tool", "tool_call_id": call["id"], "content":
                         (f"Archive page {step}: fixed historical records and findings. " * 80)})


def metrics(usage, previous):
    prompt = usage.get("prompt_tokens")
    output = usage.get("completion_tokens")
    detail = usage.get("prompt_tokens_details") or {}
    read = detail.get("cached_tokens")
    denominator = None if previous is None or previous[0] is None or previous[1] is None else sum(previous)
    return {"prompt_tokens": prompt, "cache_read_tokens": read,
            "cache_write_tokens": detail.get("cache_write_tokens", usage.get("cache_write_tokens")), "completion_tokens": output,
            "reasoning_tokens": (usage.get("completion_tokens_details") or {}).get("reasoning_tokens"),
            "reuse": read / denominator if read is not None and denominator else None}


def fetch(path, key, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(URL + path, data=data, headers={"Authorization": "Bearer " + key,
                                  "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=120) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        raise RuntimeError(f"OpenRouter HTTP {error.code}") from None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--route", action="append", required=True, help="MODEL@PROVIDER; repeat for each pinned route")
    parser.add_argument("--max-spend-usd", type=float, required=True, help="maximum allowed OpenRouter key limit")
    parser.add_argument("--execute", action="store_true", help="enable paid requests; default prints sample requests")
    parser.add_argument("--output", type=Path, help="new JSON file for live output")
    parser.add_argument("--echo-upstream-body", action="store_true")
    args = parser.parse_args(argv)
    if not math.isfinite(args.max_spend_usd) or args.max_spend_usd <= 0:
        parser.error("--max-spend-usd must be positive and finite")
    routes = []
    for route in args.route:
        model, sep, provider = route.rpartition("@")
        if not sep or not model or not provider:
            parser.error("each route must be MODEL@PROVIDER")
        routes.append((model, provider))
    key = os.environ.get("OPENROUTER_API_KEY")
    if args.execute:
        if not key or not args.output:
            parser.error("--execute requires OPENROUTER_API_KEY and --output")
        info = fetch("key", key)["data"]
        limit = info.get("limit")
        if (not isinstance(limit, (int, float)) or not math.isfinite(limit) or limit <= 0 or
                limit > args.max_spend_usd or info.get("limit_reset") is not None or
                info.get("include_byok_in_limit") is not True):
            parser.error("key needs a non-resetting limit <= --max-spend-usd including BYOK")
    results = {"mode": "live" if args.execute else "dry-run", "requests": [], "calls": []}
    output = None
    if args.execute:
        output = args.output.open("x", encoding="utf-8")
    try:
        for model, provider in routes:
            messages = [{"role": "system", "content": SYSTEM}, {"role": "user", "content": "Inspect the archive in order."}]
            previous = None
            for step in range(1, 5):
                request = request_body(model, provider, messages, args.echo_upstream_body)
                results["requests"].append(request)
                if args.execute:
                    response = fetch("chat/completions", key, request)
                    usage = response.get("usage") or {}
                    results["calls"].append({"route": f"{model}@{provider}", "call": step,
                                             **metrics(usage, previous), "response": response})
                    output.seek(0)
                    json.dump(results, output, indent=2)
                    output.truncate()
                    output.flush()
                    previous = (usage.get("prompt_tokens"), usage.get("completion_tokens"))
                    assistant = response["choices"][0]["message"]
                else:
                    assistant = {"role": "assistant", "content": None,
                                 "reasoning_details": [{"type": "reasoning.text", "text": "dry-run reasoning"}],
                                 "tool_calls": [{"id": f"dry-{step}", "type": "function", "function":
                                                 {"name": "read_page", "arguments": json.dumps({"page": step})}}]}
                append_turn(messages, assistant, step)
                if output:
                    output.seek(0)
                    json.dump(results, output, indent=2)
                    output.truncate()
                    output.flush()
        if not output:
            print(json.dumps(results, indent=2))
    finally:
        if output:
            output.seek(0)
            json.dump(results, output, indent=2)
            output.truncate()
            output.close()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError, KeyError) as error:
        print(f"probe: {error}", file=sys.stderr)
        sys.exit(1)
