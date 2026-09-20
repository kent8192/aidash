#!/usr/bin/env python3
"""Two real Aidash nodes with protocol fixtures, PostgreSQL, NATS and SIGKILL.

The deterministic model server is a test double, not a live model-quality test.
Only task-created databases and child processes are removed by this script.
"""

import argparse
import collections
import http.server
import json
import os
import pathlib
import signal
import subprocess
import threading
import time
import urllib.error
import urllib.request
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
TOKEN = "acceptance-access-token"
PEER_TOKEN = "acceptance-peer-token"


def api_request(base, path, body=None, token=TOKEN, headers=None):
    request = urllib.request.Request(
        base + path,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json", **(headers or {})},
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)


def wait_for(fn, timeout=120, label="condition"):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = fn()
            if value:
                return value
        except (OSError, urllib.error.URLError) as error:
            last = error
        time.sleep(0.2)
    raise AssertionError(f"Timed out waiting for {label}: {last}")


def entity(kind, identifier, config, capability="web.search"):
    return {
        "id": identifier, "version": "1.0.0", "kind": kind,
        "name": {"en": identifier.replace("-", " ").title(), "ja": {"coordinator": "調査コーディネーター", "researcher": "調査エージェント"}.get(identifier, identifier)},
        "description": {"en": "Golden path protocol fixture", "ja": "受入試験用のプロトコルフィクスチャ"},
        "capabilities": [capability], "tags": ["acceptance"], "languages": ["ja", "en"],
        "skills": [], "schema": {}, "config": config,
    }


class Fixture:
    def __init__(self, node_a, node_b):
        self.node_a = node_a
        self.node_b = node_b
        self.effects = {}
        self.requests = collections.Counter()
        self.lock = threading.Lock()
        self.remote_effect_started = threading.Event()
        self.provider_calls = collections.Counter()

    def infer(self, body):
        self.provider_calls["anthropic" if "system" in body else "openai"] += 1
        context = json.loads(body["messages"][0 if "system" in body else 1]["content"])
        current = context["current"]
        history = context.get("history", [])
        task = current["task"]
        workspace = current["workspace"]
        tools = [(event.get("call", {}).get("name"), event.get("result")) for event in history]
        calls = []
        text = ""

        def call(name, arguments):
            calls.append({"id": f"call-{len(calls)}", "name": name, "arguments": arguments})

        if current["identity"]["agent_id"] in ["native-runner", "mcp-runner", "agent-runner"]:
            if not any(name == "plugin_0" for name, _ in tools):
                arguments = {"message": "plugin roundtrip"}
                if current["identity"]["agent_id"] == "agent-runner":
                    arguments = {"title": "Nested native task", "description": "Verify Agent tool delegation", "requirements": {}, "dependencies": [], "parent_id": None}
                call("plugin_0", arguments)
            else:
                text = "Plugin completed: " + json.dumps(next(result for name, result in tools if name == "plugin_0"))
        elif task["title"] == "Human approval acceptance":
            answers = [event for event in history if event.get("kind") == "human"]
            if not answers:
                call("human_request", {"kind": "APPROVAL_REQUIRED", "prompt": "Approve publication?"})
            else:
                assert answers[-1]["response"] is False
                assert any(message["sender"].startswith("human") and message["content"] == "Do not publish." for message in workspace["messages"])
                text = "Approval was declined. No publication was performed."
        elif current["identity"]["agent_id"] == "coordinator":
            if not any(name == "agent_discover" for name, _ in tools):
                call("agent_discover", {"capability": "web.search", "language": "ja"})
            elif not any(name == "task_create" for name, _ in tools):
                for framework in ["Axum", "Actix", "Rocket"]:
                    call("task_create", {"title": framework, "description": f"Research {framework} and cite findings", "requirements": {"capability": "web.search", "language": "ja"}})
            elif not any(name == "task_delegate" for name, _ in tools):
                for name, result in tools:
                    if name == "task_create":
                        call("task_delegate", {"task_id": result["id"], "node_id": self.node_b if result["title"] == "Actix" else self.node_a, "agent": {"id": "researcher", "version": "1.0.0"}})
            elif any(child["status"] != "COMPLETED" for child in workspace["tasks"] if child["parent_id"] == task["id"]):
                call("workspace_wait", {"seconds": 2})
            else:
                text = "Axum・Actix・Rocketの調査が完了しました。各エージェントの成果物を統合しました。\n\n" + "\n".join(str(a["content"]) for a in workspace["artifacts"])
        elif not any(name == "plugin_0" for name, _ in tools):
            call("plugin_0", {"topic": task["title"]})
        else:
            text = f"{task['title']}: " + str(next(result for name, result in tools if name == "plugin_0"))
        if "system" in body:
            content = ([{"type": "text", "text": text}] if text else []) + [{"type": "tool_use", "id": c["id"], "name": c["name"], "input": c["arguments"]} for c in calls]
            return {"id": "fixture-message", "type": "message", "role": "assistant", "model": body["model"], "content": content, "stop_reason": "tool_use" if calls else "end_turn", "usage": {"input_tokens": 120, "output_tokens": 50}}
        message = {"role": "assistant", "content": text or None}
        if calls:
            message["tool_calls"] = [{"id": c["id"], "type": "function", "function": {"name": c["name"], "arguments": json.dumps(c["arguments"])}} for c in calls]
        return {"choices": [{"index": 0, "finish_reason": "tool_calls" if calls else "stop", "message": message}], "usage": {"prompt_tokens": 120, "completion_tokens": 50}}

    def handler(self):
        fixture = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                self.send_response(405)
                self.end_headers()

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                if self.path == "/mcp":
                    if "id" not in body:
                        self.send_response(202)
                        self.end_headers()
                        return
                    if body["method"] == "initialize":
                        result = {"protocolVersion": body["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "acceptance-mcp", "version": "1.0.0"}}
                    elif body["method"] == "tools/call":
                        assert body["params"]["name"] == "echo"
                        result = {"content": [{"type": "text", "text": json.dumps(body["params"]["arguments"])}], "isError": False}
                    else:
                        raise AssertionError(f"Unexpected MCP method: {body['method']}")
                    response = json.dumps({"jsonrpc": "2.0", "id": body["id"], "result": result}).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(response)))
                    self.end_headers()
                    self.wfile.write(response)
                    return
                if self.path == "/research":
                    key = self.headers.get("Idempotency-Key")
                    assert key, "Tool adapter must propagate an idempotency key"
                    with fixture.lock:
                        fixture.requests[key] += 1
                        first = key not in fixture.effects
                        fixture.effects.setdefault(key, {"topic": body["topic"], "finding": f"{body['topic']} protocol fixture result", "source": "https://docs.rs/"})
                        result = fixture.effects[key]
                    if body["topic"] == "Actix" and first:
                        fixture.remote_effect_started.set()
                        time.sleep(4)
                else:
                    result = fixture.infer(body)
                payload = json.dumps(result).encode()
                try:
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        return Handler


def psql(db, query):
    return subprocess.check_output(["docker", "compose", "exec", "-T", "postgres", "psql", "-U", "aidash", "-d", db, "-tA", "-v", "ON_ERROR_STOP=1", "-c", query], cwd=ROOT, text=True).strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default=os.environ.get("AIDASH_BINARY", "target/debug/aidash"))
    parser.add_argument("--keep", action="store_true", help="Keep the completed local demo running until interrupted")
    parser.add_argument("--dashboard", action="store_true", help="Submit the golden path goal through Chromium instead of the API")
    args = parser.parse_args()
    run_id = uuid.uuid4().hex[:12]
    db_a, db_b = f"aidash_e2e_{run_id}_a", f"aidash_e2e_{run_id}_b"
    node_a, node_b = f"aidash://acceptance-{run_id}-a", f"aidash://acceptance-{run_id}-b"
    base_a, base_b = "http://127.0.0.1:18080", "http://127.0.0.1:18081"
    logs = ROOT / ".ignore" / "acceptance"
    logs.mkdir(parents=True, exist_ok=True)
    (logs / "report.json").unlink(missing_ok=True)
    children = []
    files = []
    fixture = Fixture(node_a, node_b)
    fixture_server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), fixture.handler())
    threading.Thread(target=fixture_server.serve_forever, daemon=True).start()
    fixture_url = f"http://127.0.0.1:{fixture_server.server_port}"
    stream_events = []

    def launch(node, database, port, mode):
        env = {**os.environ, "DATABASE_URL": f"postgres://aidash:aidash-local@127.0.0.1:54370/{database}", "NATS_URL": "nats://127.0.0.1:42270", "AIDASH_NODE_ID": node, "AIDASH_ENDPOINT": f"http://127.0.0.1:{port}", "AIDASH_LISTEN": f"127.0.0.1:{port}", "AIDASH_API_TOKEN": TOKEN, "AIDASH_SECRET_PEER": PEER_TOKEN, "AIDASH_WEB_DIR": str(ROOT / "web/dist")}
        log = open(logs / f"{database}-{mode}-{len(children)}.log", "w")
        files.append(log)
        child = subprocess.Popen([args.binary, mode], cwd=ROOT, env=env, stdout=log, stderr=log)
        children.append(child)
        return child

    def sse():
        request = urllib.request.Request(base_a + "/api/events/stream", headers={"Authorization": f"Bearer {TOKEN}"})
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                for line in response:
                    if line.startswith(b"data:"):
                        stream_events.append(json.loads(line[5:]))
        except (OSError, ValueError):
            pass

    try:
        psql("aidash_a", f"CREATE DATABASE {db_a}")
        psql("aidash_a", f"CREATE DATABASE {db_b}")
        launch(node_a, db_a, 18080, "server")
        launch(node_b, db_b, 18081, "server")
        wait_for(lambda: api_request(base_a, "/health"), label="Node A")
        wait_for(lambda: api_request(base_b, "/health"), label="Node B")
        for base, other, endpoint in [(base_a, node_b, base_b), (base_b, node_a, base_a)]:
            api_request(base, "/api/peers", {"node_id": other, "endpoint": endpoint, "credential_env": "AIDASH_SECRET_PEER", "protocol_version": "0.1", "enabled": True})
            model = entity("model", "fixture-model", {"provider": "openai" if base == base_a else "anthropic", "model_id": "protocol-fixture", "endpoint": fixture_url + "/v1", "context_window": 256000, "modalities": ["text"], "cost": {"currency": "USD", "input_per_million": 0}, "credential_env": None})
            api_request(base, "/api/registry", model)
            tool = entity("tool", "research-http", {"transport": "http", "endpoint": fixture_url + "/research", "credential_env": None, "replay": "idempotent"})
            tool["schema"] = {"type": "object", "required": ["topic"], "properties": {"topic": {"type": "string"}}, "additionalProperties": False}
            api_request(base, "/api/registry", tool)
            api_request(base, "/api/registry", entity("cluster", "research-cluster", {"coordinator": {"id": "coordinator" if base == base_a else "researcher", "version": "1.0.0"}}))
            api_request(base, "/api/registry", entity("agent", "researcher", {"model": {"id": "fixture-model", "version": "1.0.0"}, "instructions": "Research one framework and publish your findings.", "tools": [{"id": "research-http", "version": "1.0.0"}], "skills": [], "cluster": {"id": "research-cluster", "version": "1.0.0"}, "max_steps": 64}))
        api_request(base_a, "/api/registry", entity("agent", "coordinator", {"model": {"id": "fixture-model", "version": "1.0.0"}, "instructions": "Discover researchers, decompose the goal, delegate across nodes, wait and synthesize.", "tools": [], "skills": [], "cluster": {"id": "research-cluster", "version": "1.0.0"}, "max_steps": 128}, capability="task.coordinate"))
        api_request(base_a, "/api/registry", entity("cluster", "research-cluster", {"coordinator": {"id": "coordinator", "version": "1.0.0"}}))
        discovered = api_request(base_a, "/api/discover", {"capability": "web.search", "language": "ja"})
        assert {a["node_id"] for a in discovered["agents"]} == {node_a, node_b}
        threading.Thread(target=sse, daemon=True).start()
        launch(node_a, db_a, 18080, "worker")
        remote_worker = launch(node_b, db_b, 18081, "worker")
        if args.dashboard:
            created = json.loads(subprocess.check_output(["node", "web/scripts/start-goal.mjs"], cwd=ROOT, text=True))
        else:
            created = api_request(base_a, "/api/conversations", {"title": "Rustフレームワークの競合調査", "goal": "Rust製Webフレームワークについて競合調査して", "target": {"id": "research-cluster", "version": "1.0.0"}, "target_kind": "cluster"})
        workspace = created["workspace"]["id"]
        assert fixture.remote_effect_started.wait(90), "Remote tool was never invoked"
        before = api_request(base_b, "/api/state")["runs"][0]
        remote_worker.kill()
        remote_worker.wait(timeout=10)
        print("Killed Node B worker after the remote tool effect, before result persistence", flush=True)
        launch(node_b, db_b, 18081, "worker")

        def complete():
            snapshot = api_request(base_a, f"/api/workspaces/{workspace}")
            failures = [t for t in snapshot["tasks"] if t["status"] in ["FAILED", "CANCELLED"]]
            if failures:
                print("Task failure:", json.dumps(failures), flush=True)
                return False
            return snapshot if len(snapshot["tasks"]) == 4 and all(t["status"] == "COMPLETED" for t in snapshot["tasks"]) else False

        snapshot = wait_for(complete, timeout=150, label="all four tasks and artifacts")
        recovered = wait_for(lambda: next((r for r in api_request(base_b, "/api/state")["runs"] if r["id"] == before["id"] and r["phase"] == "COMPLETED"), None), label="same durable remote run")
        assert recovered["id"] == before["id"]
        assert any(e["kind"] == "run.recovered" and e["data"].get("run_id") == recovered["id"] for e in api_request(base_b, "/api/events"))
        assert len(snapshot["artifacts"]) == 4
        assert len(fixture.effects) == 3
        assert max(fixture.requests.values()) >= 2, "A remote invocation should have been replayed with the same key"
        assert fixture.provider_calls["openai"] > 0 and fixture.provider_calls["anthropic"] > 0
        wait_for(lambda: any(e["type"] == "task.completed" for e in stream_events), label="SSE result delivery")
        wait_for(lambda: int(psql(db_a, "SELECT count(*) FROM events WHERE published_at IS NULL")) == 0, label="outbox flush")
        wait_for(lambda: int(psql(db_a, "SELECT count(*) FROM inbox")) > 0, label="JetStream durable consumption")
        assert len({e["id"] for e in stream_events}) == len(stream_events)
        last_sequence = stream_events[-1]["sequence"]
        replay = api_request(base_a, f"/api/events?after={last_sequence - 1}")
        assert replay[0]["sequence"] == last_sequence
        human_workspace = api_request(base_a, "/api/workspaces", {"title": "Human interaction", "goal": "Verify remote human controls"})
        human_task = api_request(base_a, f"/api/workspaces/{human_workspace['id']}/tasks", {"title": "Human approval acceptance", "description": "Ask before publishing", "requirements": {"language": "ja"}, "dependencies": [], "parent_id": None})
        api_request(base_a, f"/api/tasks/{human_task['id']}/delegate", {"node_id": node_b, "agent": {"id": "researcher", "version": "1.0.0"}})
        request = wait_for(lambda: next((h for node in api_request(base_a, "/api/mesh")["nodes"] for h in node["human_requests"] if h["workspace_id"] == human_workspace["id"]), None), label="remote human request")
        human_run = request["run_id"]
        def remote_control(action, **kwargs):
            return api_request(base_a, "/api/remote", {"node_id": node_b, "control": {"run_id": human_run, "action": action, **kwargs}})
        wait_for(lambda: api_request(base_b, f"/api/runs/{human_run}")["run"]["phase"] == "WAITING", label="human run waiting")
        remote_control("pause")
        remote_control("message", content="Do not publish.")
        remote_control("answer", request_id=request["id"], response=False)
        paused = api_request(base_b, f"/api/runs/{human_run}")["run"]
        assert paused["control"] == "PAUSED" and paused["phase"] == "WAITING"
        remote_control("resume")
        human_result = wait_for(lambda: next((t for t in api_request(base_a, f"/api/workspaces/{human_workspace['id']}")["tasks"] if t["status"] == "COMPLETED"), None), label="remote human response and resume")
        assert human_result["id"] == human_task["id"]
        assert len(fixture.effects) == 3, "Declined approval must not produce an extra effect"
        wait_for(lambda: any(e["type"] == "task.completed" and e["subject"] == human_workspace["id"] for e in stream_events), label="human-controlled result SSE")
        plugin_runs = []
        for base, agent_id, tool_config in [
            (base_b, "native-runner", {"transport": "native", "operation": "echo"}),
            (base_a, "mcp-runner", {"transport": "mcp", "endpoint": fixture_url + "/mcp", "tool_name": "echo", "replay": "read_only", "credential_env": None, "idempotency_argument": None}),
            (base_a, "agent-runner", {"transport": "agent", "node_id": node_b, "agent": {"id": "native-runner", "version": "1.0.0"}}),
        ]:
            tool_id = agent_id + "-tool"
            api_request(base, "/api/registry", entity("tool", tool_id, tool_config))
            api_request(base, "/api/registry", entity("agent", agent_id, {"model": {"id": "fixture-model", "version": "1.0.0"}, "instructions": "Verify the registered plugin, then return its result.", "tools": [{"id": tool_id, "version": "1.0.0"}], "skills": [], "max_steps": 32}))
            plugin_conversation = api_request(base, "/api/conversations", {"title": agent_id, "goal": "Execute the configured plugin", "target": {"id": agent_id, "version": "1.0.0"}, "target_kind": "agent"})
            plugin_workspace = plugin_conversation["workspace"]["id"]
            result = wait_for(lambda base=base, plugin_workspace=plugin_workspace, task_id=plugin_conversation["task"]["id"]: next((t for t in api_request(base, f"/api/workspaces/{plugin_workspace}")["tasks"] if t["id"] == task_id and t["status"] == "COMPLETED"), None), label=agent_id)
            assert result["status"] == "COMPLETED"
            plugin_runs.append(agent_id)
        report = {"node_a": base_a, "node_b": base_b, "node_ids": [node_a, node_b], "workspace_id": workspace, "tasks": len(snapshot["tasks"]), "artifacts": len(snapshot["artifacts"]), "external_effects": len(fixture.effects), "tool_requests": dict(fixture.requests), "provider_calls": dict(fixture.provider_calls), "recovered_run_id": recovered["id"], "sse_events": len(stream_events), "database_a": db_a, "database_b": db_b, "goal_entry": "dashboard" if args.dashboard else "api", "remote_human_controls": "passed", "additional_plugins": plugin_runs}
        if args.dashboard:
            subprocess.run(["npm", "test", "--prefix", "web"], cwd=ROOT, check=True)
            report["browser_scenarios"] = 3
        (logs / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
        print("Golden path passed:", json.dumps(report, ensure_ascii=False), flush=True)
        if args.keep:
            (ROOT / ".ignore/e2e-ready.json").write_text(json.dumps(report))
            while True:
                time.sleep(1)
    finally:
        for child in children:
            if child.poll() is None:
                child.send_signal(signal.SIGTERM)
        for child in children:
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
        fixture_server.shutdown()
        for file in files:
            file.close()
        for db in [db_a, db_b]:
            psql("aidash_a", f"DROP DATABASE IF EXISTS {db} WITH (FORCE)")
        (ROOT / ".ignore/e2e-ready.json").unlink(missing_ok=True)


if __name__ == "__main__":
    main()
