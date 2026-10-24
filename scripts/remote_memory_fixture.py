#!/usr/bin/env python3
"""Deterministic, in-cluster providers. Only synthetic acceptance data is captured."""
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOCK = threading.Condition()
STATE = {"hold_embedding": False, "hold_inference": False, "requests": [], "errors": []}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, value):
        body = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass  # Expected after killing a provider's caller.

    def do_GET(self):
        with LOCK:
            value = json.loads(json.dumps(STATE))
        self.reply(200, value)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/control":
            with LOCK:
                for key in ("hold_embedding", "hold_inference"):
                    if key in body:
                        STATE[key] = bool(body[key])
                LOCK.notify_all()
            return self.reply(200, {"ok": True})
        try:
            if self.path == "/v1/embeddings":
                with LOCK:
                    # A keyless embedding endpoint (ADR 0006): no environment
                    # key exists, so no caller credential may arrive here.
                    if self.headers.get("Authorization"):
                        STATE["requests"].append({"kind": "embedding_rejected"})
                        return self.reply(401, {"error": "unexpected credential for a keyless endpoint"})
                    STATE["requests"].append({"kind": "embedding", "keyless": True, "body": body})
                    LOCK.wait_for(lambda: not STATE["hold_embedding"], timeout=300)
                return self.reply(200, {"model": body["model"], "data": [{"index": 0, "embedding": [1.0, 0.0, 0.0]}],
                                        "usage": {"prompt_tokens": 1, "total_tokens": 1}})
            assert self.path == "/v1/chat/completions", self.path
            assert not self.headers.get("Authorization"), "subject/peer/provider credentials must not be forwarded to the unsigned model"
            context = json.loads(body["messages"][1]["content"])
            current = context["current"]
            task = current["task"]
            parent = task["title"] == "Generated origin"
            with LOCK:
                previous = sum(r["kind"] == "parent" and r["task"] == task["id"] for r in STATE["requests"])
                STATE["requests"].append({"kind": "parent" if parent else "inference", "task": task["id"], "body": body})
                if not parent:
                    semantic = current["semantic_memory"]
                    encoded = json.dumps(semantic)
                    assert semantic["home_node"] == self.server.home_node, semantic
                    assert "ochre falcon" in encoded and "DO NOT DISCLOSE" not in encoded, semantic
                    assert semantic["executor"].startswith(self.server.executor_node + "/agents/"), semantic
                    assert semantic["sources"] and semantic["result"]["matches"], semantic
                    assert all(item["revision"] > 0 for item in semantic["sources"]), semantic
                    LOCK.wait_for(lambda: not STATE["hold_inference"], timeout=300)
            if parent:
                if previous == 0:
                    name, arguments = "task_create", {"title": "Generated remote research", "description": "Find the relevant non-keyword archive marker", "requirements": {}}
                else:
                    name, arguments = "human_request", {"kind": "INFORMATION_REQUEST", "prompt": "Wait while the foreign child completes"}
                message = {"role": "assistant", "content": None, "tool_calls": [{"id": f"parent-{previous}", "type": "function", "function": {"name": name, "arguments": json.dumps(arguments)}}]}
            else:
                message = {"role": "assistant", "content": "Remote result: ochre falcon from the approved Home receipt."}
            self.reply(200, {"choices": [{"index": 0, "finish_reason": "tool_calls" if parent else "stop", "message": message}],
                             "usage": {"prompt_tokens": 1, "completion_tokens": 1}})
        except Exception as error:
            with LOCK:
                STATE["errors"].append(f"{type(error).__name__}: {error}")
            self.reply(400, {"error": "acceptance assertion failed"})


def serve(handler=Handler):
    home = os.environ["AIDASH_FIXTURE_HOME_NODE"]
    executor = os.environ["AIDASH_FIXTURE_EXECUTOR_NODE"]
    server = ThreadingHTTPServer(("0.0.0.0", 8080), handler)
    server.home_node, server.executor_node = home, executor
    server.serve_forever()


if __name__ == "__main__":
    serve()
