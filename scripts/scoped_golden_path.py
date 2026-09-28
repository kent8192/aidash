"""Subject-scoped acceptance against real Node and Worker processes.

Only model/tool providers and peer transport faults are fixtures. Authorization,
admission, execution, Home effects and recovery all run in the Aidash binary.
"""

import collections
import hashlib
import http.server
import json
import signal
import threading
import urllib.error
import urllib.request
import uuid

from golden_path import PEER_TOKEN, api_request, entity, wait_for


def denied(base, path, body=None, **kwargs):
    try:
        api_request(base, path, body, **kwargs)
    except urllib.error.HTTPError as error:
        assert error.code in (403, 404), f"Expected authorization denial, got {error.code}"
    else:
        raise AssertionError(f"Unauthorized request succeeded: {path}")


class PeerProxy:
    """Forward actual peer replies, optionally withholding a committed Home reply."""

    def __init__(self, upstream, forbidden_tokens):
        self.upstream = upstream
        self.forbidden_tokens = forbidden_tokens
        self.errors = []
        self.paths = collections.Counter()
        self.commands = collections.defaultdict(list)
        self.unavailable = threading.Event()
        self.blocked = threading.Event()
        self.release = threading.Event()
        self.armed = None
        self.lock = threading.Lock()
        proxy = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                # Peer registration resolves the real Node identity before it
                # stores an endpoint. Do not synthesize that handshake.
                with urllib.request.urlopen(proxy.upstream + self.path, timeout=10) as response:
                    payload = response.read()
                    self.send_response(response.status)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)

            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                try:
                    text = raw.decode()
                    assert self.headers.get("Authorization") == f"Bearer {PEER_TOKEN}", "Peer transport must use its own credential"
                    assert not any(token in text or token in str(self.headers) for token in proxy.forbidden_tokens), "Subject credential forwarded to a peer"
                    body = json.loads(raw)
                    with proxy.lock:
                        proxy.paths[self.path] += 1
                        command = (body.get("grant_id"), body.get("operation"))
                        if self.path.endswith("/scoped/execution/commands"):
                            proxy.commands[command].append(hashlib.sha256(raw).hexdigest())
                        hold = command == proxy.armed
                        if hold:
                            proxy.armed = None
                    if proxy.unavailable.is_set():
                        code, payload = 503, b'{"error":"acceptance peer outage"}'
                    else:
                        headers = {key: value for key, value in self.headers.items() if key.lower() not in ("host", "content-length", "connection")}
                        request = urllib.request.Request(proxy.upstream + self.path, data=raw, headers=headers)
                        try:
                            with urllib.request.urlopen(request, timeout=25) as response:
                                code, payload = response.status, response.read()
                        except urllib.error.HTTPError as error:
                            code, payload = error.code, error.read()
                        if hold:
                            assert code == 200, f"Home command failed before crash boundary: {code}"
                            # Aidash has committed the real transaction; the Worker
                            # has not received its response or persisted its receipt.
                            proxy.blocked.set()
                            assert proxy.release.wait(20), "Worker crash was not injected"
                    self.send_response(code)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except (BrokenPipeError, ConnectionResetError):
                    pass  # Expected when the real Worker is killed.
                except Exception as error:
                    proxy.errors.append(type(error).__name__ + ": " + str(error))
                    self.send_error(502)

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def arm(self, grant, operation):
        self.blocked.clear()
        self.release.clear()
        self.armed = (grant, operation)

    def close(self):
        self.release.set()
        self.server.shutdown()
        self.server.server_close()


class Providers:
    def __init__(self, forbidden_tokens):
        self.model_calls = collections.Counter()
        self.effect_calls = collections.Counter()
        self.effects = {}
        self.errors = []
        self.lock = threading.Lock()
        fixture = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                try:
                    raw = self.rfile.read(int(self.headers["Content-Length"]))
                    assert not any(
                        token in raw.decode() or token in str(self.headers)
                        for token in forbidden_tokens
                    ), "Credential disclosed to provider"
                    body = json.loads(raw)
                    if self.path == "/effect":
                        key = self.headers.get("Idempotency-Key")
                        assert key, "External effect needs an idempotency key"
                        with fixture.lock:
                            fixture.effect_calls[key] += 1
                            fixture.effects.setdefault(key, body)
                            assert fixture.effects[key] == body, "Effect key rebound"
                        result = {"accepted": body["scenario"]}
                    else:
                        assert self.path == "/v1/chat/completions"
                        context = json.loads(body["messages"][1]["content"])
                        scenario = context["current"]["task"]["title"]
                        with fixture.lock:
                            fixture.model_calls[scenario] += 1
                        tools = {item.get("call", {}).get("name") for item in context.get("history", []) if item.get("kind") == "tool"}
                        if "workspace_message" not in tools:
                            name, arguments = "workspace_message", {"content": "Scoped progress: " + scenario}
                        elif "plugin_0" not in tools:
                            name, arguments = "plugin_0", {"scenario": scenario}
                        else:
                            name = None
                        message = {"role": "assistant", "content": "Scoped result: " + scenario}
                        if name:
                            message = {"role": "assistant", "content": None, "tool_calls": [{"id": name, "type": "function", "function": {"name": name, "arguments": json.dumps(arguments)}}]}
                        result = {"choices": [{"index": 0, "finish_reason": "tool_calls" if name else "stop", "message": message}], "usage": {"prompt_tokens": 120, "completion_tokens": 50}}
                    payload = json.dumps(result).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except Exception as error:
                    fixture.errors.append(type(error).__name__ + ": " + str(error))
                    self.send_error(502)

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


def verify(base_a, base_b, node_a, node_b, worker, launch_worker, counts):
    """Required Golden Path branch; any missing assertion fails the parent run."""
    source_tenant, receiver_tenant = "golden-source", "golden-receiver"
    agent = {"id": "scoped-researcher", "version": "1.0.0"}
    executor = f"{node_b}/agents/{agent['id']}@{agent['version']}"
    tokens = [PEER_TOKEN]
    subject_tokens = []
    fixture = Providers(tokens)
    home = PeerProxy(base_a, subject_tokens)
    receiver = PeerProxy(base_b, subject_tokens)
    initial_counts = counts()
    scenarios = []

    def peer(base, other, endpoint):
        api_request(base, "/api/peers", {"node_id": other, "endpoint": endpoint, "credential_env": "AIDASH_SECRET_PEER", "protocol_version": "0.1", "enabled": True})

    def policy(base, tenant, subject):
        bundle = {"tenant": tenant, "subjects": {subject: {"kind": "user"}, executor: {"kind": "agent"}}, "policies": [{"id": "fixture-work", "effect": "allow", "subjects": {"any": True}, "actions": ["*"], "resources": {"kinds": ["*"]}}]}
        api_request(base, f"/api/authorization/{tenant}", {"expected_revision": 0, "bundle": bundle})
        credential = api_request(base, f"/api/authorization/{tenant}/credentials", {"subject": subject, "expires_in_seconds": 3600})
        subject_tokens.append(credential["token"])
        tokens.append(credential["token"])
        return credential

    def catalog(entry, enabled, revision):
        return api_request(base_b, f"/api/authorization/{receiver_tenant}/catalog", {"entry": entry, "enabled": enabled, "expected_revision": revision})

    try:
        peer(base_a, node_b, receiver.url)
        peer(base_b, node_a, home.url)
        source = policy(base_a, source_tenant, "alice")
        mapped = policy(base_b, receiver_tenant, "remote-alice")
        outsider = policy(base_a, "golden-outsider", "alice")
        token = source["token"]
        api_request(base_b, f"/api/authorization/{receiver_tenant}/peer-mappings", {"source_node": node_a, "source_tenant": source_tenant, "source_subject": "alice", "credential_id": mapped["credential"]["id"], "enabled": True, "expected_revision": 0})
        entries = [
            entity("model", "scoped-model", {"provider": "openrouter", "model_id": "fixture", "endpoint": fixture.url + "/v1", "context_window": 128000, "max_output_tokens": 4096, "modalities": ["text"], "cost": {}}),
            entity("tool", "scoped-effect", {"transport": "http", "endpoint": fixture.url + "/effect", "credential_env": None, "replay": "idempotent"}),
            entity("agent", agent["id"], {"model": {"id": "scoped-model", "version": "1.0.0"}, "instructions": "Report progress, invoke the approved tool, then publish the result.", "tools": [{"id": "scoped-effect", "version": "1.0.0"}], "skills": [], "max_steps": 16}),
        ]
        entries[1]["schema"] = {"type": "object", "required": ["scenario"], "properties": {"scenario": {"type": "string"}}, "additionalProperties": False}
        for entry in entries:
            api_request(base_b, "/api/registry", entry)
            catalog({"id": entry["id"], "version": entry["version"]}, True, 0)

        def create_task(title):
            workspace = api_request(base_a, "/api/workspaces", {"title": title, "goal": "Verify scoped remote process recovery"}, token=token)
            task = api_request(base_a, f"/api/workspaces/{workspace['id']}/tasks", {"title": title, "description": "Use the admitted remote Agent"}, token=token)
            return workspace, task

        # An allowed source cannot override the receiver's exact catalog approval.
        _, rejected_task = create_task("receiver-denies-dependency")
        for reference in ({"id": "scoped-model", "version": "1.0.0"}, {"id": "scoped-effect", "version": "1.0.0"}, agent):
            catalog(reference, False, 1)
            denied(base_a, f"/api/tasks/{rejected_task['id']}/delegate", {"node_id": node_b, "agent": agent}, token=token)
            catalog(reference, True, 2)
        assert counts() == initial_counts, "Denied execution created a binding/admission"

        for scenario, operation in [("home-message-recovery", "message"), ("home-completion-recovery", "run_message_complete"), ("peer-outage-recovery", "message"), ("revoked-during-worker-stop", "message")]:
            workspace, task = create_task(scenario)
            path = f"/api/tasks/{task['id']}/remote-grants"
            grant = str(uuid.uuid4())
            preparation = {"id": grant, "node_id": node_b, "agent": agent, "ttl_seconds": 300}
            prepared = api_request(base_a, path, preparation, token=token)
            assert api_request(base_a, path, preparation, token=token) == prepared
            denied(base_a, path, {**preparation, "id": str(uuid.uuid4())}, token=outsider["token"])
            home.arm(grant, operation)
            activated = api_request(base_a, f"{path}/{grant}/activate", {}, token=token)
            run_id = activated["run_id"]
            assert activated["admission_id"] == run_id
            assert api_request(base_a, f"{path}/{grant}/activate", {}, token=token)["run_id"] == run_id

            # The legacy path must reject even an authenticated peer.
            peer_headers = {"x-aidash-node": node_a, "x-aidash-protocol": "0.1"}
            denied(base_b, "/federation/v0.1/offers", {"task": task, "agent": agent}, token=PEER_TOKEN, headers=peer_headers)
            denied(base_a, "/federation/v0.1/scoped/execution/commands", {"grant_id": grant, "admission_id": run_id, "operation": "workspace_record", "data": {"kind": "task", "id": rejected_task["id"]}}, token=PEER_TOKEN, headers={"x-aidash-node": node_b, "x-aidash-protocol": "0.1"})
            assert home.blocked.wait(90), f"Did not reach committed {operation}: {home.errors} {fixture.errors}"

            def run(run_id=run_id):
                return api_request(base_b, f"/api/runs/{run_id}")["run"]

            before = run()
            assert before["agent_id"] == agent["id"] and before["agent_version"] == agent["version"]
            assert before["task_id"] == task["id"] and before["home_node"] == node_a
            old_pid = worker.pid
            worker.kill()
            assert worker.wait(timeout=10) == -signal.SIGKILL
            home.release.set()
            before_calls = fixture.model_calls[scenario]
            if scenario == "revoked-during-worker-stop":
                api_request(base_a, f"{path}/{grant}/revoke", {}, token=token)
            if scenario == "peer-outage-recovery":
                home.unavailable.set()
            worker = launch_worker()
            assert worker.pid != old_pid

            if scenario == "peer-outage-recovery":
                wait_for(lambda: run().get("error"), timeout=90, label="scoped Worker fails closed during peer outage")
                assert fixture.model_calls[scenario] == before_calls
                assert not any(effect["scenario"] == scenario for effect in fixture.effects.values())
                home.unavailable.clear()
            if scenario == "revoked-during-worker-stop":
                wait_for(lambda: run()["control"] == "PAUSED", timeout=90, label="restarted Worker rejects revoked grant")
                assert fixture.model_calls[scenario] == before_calls
                assert not any(effect["scenario"] == scenario for effect in fixture.effects.values())
                stopped = api_request(base_a, f"/api/workspaces/{workspace['id']}")
                assert not stopped["artifacts"] and stopped["tasks"][0]["status"] != "COMPLETED"
                denied(base_a, f"{path}/{grant}/control", {"action": "resume"}, token=token)
                api_request(base_a, f"{path}/{grant}/control", {"action": "cancel"}, token=token)
                wait_for(lambda: run()["phase"] == "CANCELLED", label="revoked remote run cancellation")
                assert api_request(base_a, f"/api/workspaces/{workspace['id']}")["tasks"][0]["status"] == "CANCELLED"
            else:
                wait_for(lambda: run()["phase"] == "COMPLETED", timeout=150, label=scenario)
                if scenario == "home-completion-recovery":
                    assert fixture.model_calls[scenario] == before_calls, "Recovery repeated committed inference"
                snapshot = api_request(base_a, f"/api/workspaces/{workspace['id']}", token=token)
                assert snapshot["tasks"][0]["status"] == "COMPLETED"
                assert len(snapshot["artifacts"]) == 1 and snapshot["artifacts"][0]["content"] == "Scoped result: " + scenario
                assert sum(message["content"] == "Scoped progress: " + scenario for message in snapshot["messages"]) == 1
                assert sum(effect["scenario"] == scenario for effect in fixture.effects.values()) == 1
                receipts = home.commands[(grant, operation)]
                # A completed Home task can reconcile without resending the
                # completion command. Any actual retry must retain its input.
                assert receipts and len(set(receipts)) == 1, "Recovery changed a committed Home command"
                assert api_request(base_a, f"{path}/{grant}/activate", {}, token=token)["run_id"] == run_id
            matching = [item for item in api_request(base_b, "/api/state")["runs"] if item["task_id"] == task["id"] and item["home_node"] == node_a]
            assert len(matching) == 1 and matching[0]["id"] == run_id
            statuses = api_request(base_a, f"/api/tasks/{task['id']}/remote-executions", token=token)
            assert len(statuses) == 1 and statuses[0]["execution"]["run_id"] == run_id
            assert statuses[0]["execution"]["phase"] == run()["phase"]
            assert not any(secret in json.dumps(statuses) for secret in tokens), "Execution status exposed a credential"
            scenarios.append({"scenario": scenario, "task_id": task["id"], "workspace_id": workspace["id"], "grant_id": grant, "admission_id": activated["admission_id"], "run_id": run_id, "worker_pid_before": old_pid, "worker_pid_after": worker.pid, "exit_signal": "SIGKILL", "phase": run()["phase"], "home_command_attempts": len(home.commands[(grant, operation)]), "passed": True})
            print(f"Scoped Golden Path passed: {scenario} (same Run {run_id}, Worker {old_pid} -> {worker.pid})", flush=True)

        final_counts = counts()
        assert all(final_counts[key] - initial_counts[key] == len(scenarios) for key in initial_counts), "Retries created an extra admission or Home binding"
        assert not home.errors and not receiver.errors and not fixture.errors, (home.errors, receiver.errors, fixture.errors)
        assert not any("/offers" in path for proxy in (home, receiver) for path in proxy.paths), "Scoped work fell back to legacy admission"
        assert receiver.paths["/federation/v0.1/scoped/execution/admissions"] > 0
        return {"passed": True, "source_tenant": source_tenant, "receiver_tenant": receiver_tenant, "scenarios": scenarios, "admissions": final_counts["admissions"] - initial_counts["admissions"], "home_bindings": final_counts["bindings"] - initial_counts["bindings"], "negative_checks": ["cross_tenant", "cross_workspace", "receiver_dependency_approval", "disabled_agent", "legacy_admission", "revoked_resume"], "subject_credentials_not_forwarded": True}
    finally:
        home.release.set()
        home.unavailable.clear()
        peer(base_a, node_b, base_b)
        peer(base_b, node_a, base_a)
        home.close()
        receiver.close()
        fixture.close()
