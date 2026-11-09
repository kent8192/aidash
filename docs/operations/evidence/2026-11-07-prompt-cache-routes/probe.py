#!/usr/bin/env python3
"""Live check of prompt caching per route through OpenRouter (#776).

Sends synthetic requests shaped like Aidash's Native projection: a salted system
block, tools, an attachment message, append-only history, then one volatile user
message. Records usage only, never prompts or the key. Stops at SPEND_CEILING.

Run with: python3 -I probe.py <results.jsonl> [arm-prefix ...]
"""
import concurrent.futures as futures
import json
import os
import random
import secrets
import sys
import threading
import time
import urllib.error
import urllib.request

URL = "https://openrouter.ai/api/v1/chat/completions"
SPEND_CEILING = 2.0
MARK = {"type": "ephemeral"}
WORDS = (
    "invoice ledger route tenant harness projection marker prefix window budget "
    "attachment history journal policy approval worker runner sandbox gateway "
    "contract digest snapshot binding revision catalog workspace thread message"
).split()

lock = threading.Lock()
spent = 0.0
stopped = False


def text(seed, words):
    rng = random.Random(seed)
    return " ".join(f"{rng.choice(WORDS)}{rng.randint(0, 9999)}" for _ in range(words))


TOOLS = [
    {
        "type": "function",
        "function": {
            "name": f"tool_{i:02d}",
            "description": text(f"tool-{i}", 40),
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}, "limit": {"type": "integer"}},
                "required": ["path"],
                "additionalProperties": False,
            },
        },
    }
    for i in range(8)
]


def build(nonce, step, arm):
    """The Native request of one step. History holds `step` Model Turns."""
    words = arm.get("prefix_words", 1000)
    system = f"aidash-cache-scope:v1:{nonce}\n" + text(f"{nonce}-system", words)
    marker = arm.get("markers", 0)
    messages = [
        {
            "role": "system",
            "content": [{"type": "text", "text": system, "cache_control": MARK}] if marker else system,
        },
        {
            "role": "user",
            "content": [
                {"type": "text", "text": text(f"{nonce}-identity", 60)},
                {"type": "text", "text": text(f"{nonce}-task", 200)},
            ],
        },
    ]
    parallel = arm.get("parallel", 1)
    ends = []
    for turn in range(step):
        calls = [
            {
                "id": f"call_{turn}_{i}",
                "type": "function",
                "function": {"name": f"tool_{i % 8:02d}", "arguments": json.dumps({"path": f"f{turn}{i}"})},
            }
            for i in range(parallel)
        ]
        messages.append({"role": "assistant", "content": None, "tool_calls": calls})
        for i in range(parallel):
            messages.append(
                {
                    "role": "tool",
                    "tool_call_id": f"call_{turn}_{i}",
                    "content": text(f"{nonce}-result-{turn}-{i}", arm.get("result_words", 120) // parallel),
                }
            )
        ends.append(len(messages) - 1)
    # Mark the last history message, and with three markers the previous end too.
    for index in ends[-(marker - 1) :] if marker > 1 else []:
        content = messages[index]["content"]
        messages[index]["content"] = [{"type": "text", "text": content, "cache_control": MARK}]
    volatile = {"source": "step", "trust": "data", "step": step, "state": text(f"{nonce}-volatile-{step}", 60)}
    messages.append({"role": "user", "content": json.dumps(volatile)})
    body = {
        "model": arm["model"],
        "messages": messages,
        "tools": TOOLS,
        "max_tokens": arm.get("max_tokens", 32),
        "provider": {"zdr": True, "require_parameters": True, **arm.get("provider", {})},
    }
    if arm.get("session"):
        body["session_id"] = f"aidash-{nonce}"
    if arm.get("options"):
        body["prompt_cache_options"] = arm["options"]
    if arm.get("reasoning"):
        body["reasoning"] = {"effort": arm["reasoning"]}
    return body


def call(body):
    request = urllib.request.Request(
        URL,
        data=json.dumps(body).encode(),
        headers={
            "Authorization": "Bearer " + os.environ["OPENROUTER_API_KEY"],
            "Content-Type": "application/json",
        },
    )
    started = time.time()
    try:
        with urllib.request.urlopen(request, timeout=180) as response:
            return json.load(response), None, time.time() - started
    except urllib.error.HTTPError as error:
        detail = error.read()[:600].decode(errors="replace")
        return None, f"{error.code} {detail}", time.time() - started
    except Exception as error:  # A probe records a failed call and moves on.
        return None, f"{type(error).__name__} {error}", time.time() - started


def run(name, arm, out):
    global spent, stopped
    nonce = secrets.token_hex(16)
    for step in range(arm.get("steps", 6)):
        with lock:
            if stopped or spent >= SPEND_CEILING:
                stopped = True
                return
        history = 0 if arm.get("static") else step
        reply, error, seconds = call(build(nonce, history, arm))
        record = {"arm": name, "model": arm["model"], "step": step, "seconds": round(seconds, 2), "error": error}
        if reply:
            usage = reply.get("usage") or {}
            details = usage.get("prompt_tokens_details") or {}
            record.update(
                provider=reply.get("provider"),
                prompt=usage.get("prompt_tokens"),
                read=details.get("cached_tokens"),
                write=details.get("cache_write_tokens"),
                output=usage.get("completion_tokens"),
                cost=usage.get("cost"),
            )
            with lock:
                spent += usage.get("cost") or 0
        with lock:
            out.write(json.dumps(record) + "\n")
            out.flush()
            if error and error.startswith("402"):
                stopped = True
        if error and not arm.get("keep_going"):
            return
        time.sleep(arm.get("delay", 0))


TERRA = "openai/gpt-6.1-terra"
HAIKU = "anthropic/claude-haiku-5.5"
SONNET = "anthropic/claude-sonnet-5.5"
GEMINI = "google/gemini-3.8-flash"
DEEPSEEK = "deepseek/deepseek-v4.1-flash-1017"

ARMS = {
    # GPT-5.6+: no markers, plan A, plan B, and the rolling explicit-only trap.
    "terra-implicit": {"model": TERRA},
    "terra-A-implicit+2": {"model": TERRA, "markers": 2},
    "terra-B-explicit+3": {"model": TERRA, "markers": 3, "options": {"mode": "explicit"}},
    "terra-explicit+2-rolling": {"model": TERRA, "markers": 2, "options": {"mode": "explicit"}},
    # Claude: markers on a tool message, session_id, Auto Exacto, parallel steps, reasoning.
    "haiku-none": {"model": HAIKU, "session": True, "steps": 4},
    "haiku-2": {"model": HAIKU, "markers": 2, "steps": 12},
    "haiku-2+session": {"model": HAIKU, "markers": 2, "session": True, "steps": 12},
    "haiku-2+session+price": {"model": HAIKU, "markers": 2, "session": True, "provider": {"sort": "price"}, "steps": 12},
    "haiku-2+session+parallel16": {"model": HAIKU, "markers": 2, "session": True, "parallel": 16, "result_words": 320, "steps": 8},
    "haiku-2+session+reasoning": {"model": HAIKU, "markers": 2, "session": True, "reasoning": "medium", "max_tokens": 400, "steps": 6},
    "sonnet-2+session": {"model": SONNET, "markers": 2, "session": True, "steps": 5},
    # Gemini: is the zero the minimum size, the shape, or the route?
    "gemini-static-8k": {"model": GEMINI, "static": True, "prefix_words": 1400, "steps": 4, "delay": 6},
    "gemini-static-20k": {"model": GEMINI, "static": True, "prefix_words": 3400, "steps": 4, "delay": 20},
    "gemini-static-20k-us": {"model": GEMINI, "static": True, "prefix_words": 3400, "steps": 4, "delay": 20, "provider": {"only": ["google-vertex/us"]}},
    "gemini-static-8k-long": {"model": GEMINI, "static": True, "prefix_words": 1400, "steps": 12, "delay": 6, "session": True},
    "gemini-native": {"model": GEMINI, "prefix_words": 1400, "steps": 6, "delay": 6},
    "gemini-native-session": {"model": GEMINI, "prefix_words": 1400, "steps": 12, "delay": 6, "session": True},
    "gemini-system-marker": {"model": GEMINI, "markers": 1, "prefix_words": 1400, "steps": 8, "delay": 6, "session": True},
    "gemini-2-markers": {"model": GEMINI, "markers": 2, "prefix_words": 1400, "steps": 6, "delay": 6},
    # The same shapes without ZDR, to see what ZDR costs per family. Synthetic text only.
    "nozdr-gemini-studio-static": {"model": GEMINI, "static": True, "prefix_words": 1400, "steps": 10, "delay": 6, "session": True, "provider": {"zdr": False, "only": ["google-ai-studio"]}},
    "nozdr-gemini-studio-native": {"model": GEMINI, "prefix_words": 1400, "steps": 12, "delay": 6, "session": True, "provider": {"zdr": False, "only": ["google-ai-studio"]}},
    "nozdr-haiku-anthropic": {"model": HAIKU, "markers": 2, "session": True, "steps": 6, "provider": {"zdr": False, "only": ["anthropic"]}},
    "nozdr-terra-openai": {"model": TERRA, "markers": 3, "options": {"mode": "explicit"}, "session": True, "steps": 6, "provider": {"zdr": False, "only": ["openai"]}},
    "nozdr-deepseek-official": {"model": DEEPSEEK, "session": True, "steps": 8, "provider": {"zdr": False, "only": ["deepseek"]}},
    # Mixed hosts: does session_id hold with tools, and does price sorting help?
    "deepseek-none": {"model": DEEPSEEK, "steps": 10},
    "deepseek-session": {"model": DEEPSEEK, "session": True, "steps": 10},
    "deepseek-session+price": {"model": DEEPSEEK, "session": True, "provider": {"sort": "price"}, "steps": 10},
}


def main():
    path = sys.argv[1]
    wanted = sys.argv[2:]
    arms = {k: v for k, v in ARMS.items() if not wanted or any(k.startswith(w) for w in wanted)}
    with open(path, "a") as out, futures.ThreadPoolExecutor(6) as pool:
        for job in [pool.submit(run, name, arm, out) for name, arm in arms.items()]:
            job.result()
    print(f"done, spent {spent:.4f}, stopped={stopped}")


if __name__ == "__main__":
    main()
