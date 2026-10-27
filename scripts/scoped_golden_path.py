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
import time
import urllib.error
import urllib.request
import uuid

from golden_path import tool_descriptor, tool_binding, api_request, entity, tenant_subject, default_node, register_node, wait_for

# Budget for a scoped Run to reach its crash boundary, and again to complete
# after the SIGKILL. Each durable phase performs bounded Home RPCs before its
# next lease, and one describe or command call costs about 1 s in the debug
# binary CI runs. On develop 7580b08c8 a passing home-message-recovery took
# 110-130 s after the restart and a failing one about 160 s, so 150 s held
# only on fast runners (#321).
RECOVERY_BUDGET_SECONDS = 300


def denied(base, path, body=None, codes=(403, 404), **kwargs):
    try:
        api_request(base, path, body, **kwargs)
    except urllib.error.HTTPError as error:
        assert error.code in codes, f"Expected authorization denial, got {error.code}"
    else:
        raise AssertionError(f"Unauthorized request succeeded: {path}")


def serve(handler, listener=None):
    """A threaded HTTP server for `handler`, on a socket the caller bound earlier when it passes one (keeping its port)."""
    if listener is None:
        return http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    server = http.server.ThreadingHTTPServer(listener.getsockname(), handler, bind_and_activate=False)
    server.socket.close()
    server.socket = listener
    server.server_address = listener.getsockname()
    server.server_name, server.server_port = server.server_address[:2]
    server.server_activate()
    return server


class PeerProxy:
    """Forward actual peer replies, optionally withholding a committed Home reply."""

    def __init__(self, upstream, forbidden_tokens, listener=None):
        self.upstream = upstream
        self.forbidden_tokens = forbidden_tokens
        self.errors = []
        self.paths = collections.Counter()
        self.commands = collections.defaultdict(list)
        self.timings = collections.defaultdict(list)
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
                # Registering a Node pins the key its Deployment publishes. Do
                # not synthesize that handshake.
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
                    assert self.headers.get("Authorization", "").startswith("Bearer "), "Peer transport must send a Federation token"
                    assert self.headers.get("x-aidash-protocol") == "0.4", "Peer transport must speak the current protocol"
                    assert not any(token in text or token in str(self.headers) for token in proxy.forbidden_tokens), "Subject credential forwarded to a peer"
                    body = json.loads(raw)
                    started = time.monotonic()
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
                        with proxy.lock:
                            proxy.timings[command].append(round(time.monotonic() - started, 3))
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

        self.server = serve(Handler, listener)
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
    def __init__(self, forbidden_tokens, listener=None):
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
                        elif "scoped_effect" not in tools:
                            name, arguments = "scoped_effect", {"scenario": scenario}
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

        self.server = serve(Handler, listener)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


def verify(base_a, base_b, node_a, node_b, worker, provider_socket, launch_worker, counts, proxy_sockets, issue_a, issue_b):
    """Required Golden Path branch; any missing assertion fails the parent run.

    The model provider listens on the already bound `provider_socket`, whose
    origin the nodes permit for Keyless endpoints. `issue_a` and `issue_b` mint
    a Tenant's first Subject Credential on each Node from the host (#484).
    """
    source_tenant, receiver_tenant = "golden-source", "golden-receiver"
    agent = {"id": "scoped-researcher", "version": "1.0.0"}
    agent_ref = f"{agent['id']}@{agent['version']}"
    tokens = []
    subject_tokens = []
    fixture = Providers(tokens, provider_socket)
    # Each Node reaches the other through a proxy on a port its Deployment permits.
    home = PeerProxy(base_a, subject_tokens, proxy_sockets[0])
    receiver = PeerProxy(base_b, subject_tokens, proxy_sockets[1])
    initial_counts = counts()
    scenarios = []

    approval_stop = threading.Event()
    owned_grants = []

    def approve_fixture_calls():
        while not approval_stop.wait(1):
            for task_id, grant_id in list(owned_grants):
                try:
                    statuses = api_request(base_a, f"/api/tasks/{task_id}/remote-executions", token=token)
                    for status in statuses:
                        for request in status.get("human_requests", []):
                            if request.get("response") is None and request["kind"] == "APPROVAL_REQUIRED" and request["prompt"].startswith("Approve this exact external tool action once? Tool: scoped-effect@"):
                                api_request(base_a, f"/api/tasks/{task_id}/remote-grants/{grant_id}/human-requests/answer", {"id": request["id"], "response": {"approved": True}}, token=token)
                except OSError:
                    pass  # Outage or revoked authority must not create an effect.
                except Exception as error:
                    fixture.errors.append(str(error))

    def policy(base, label, subject, agents):
        # Aidash assigns Tenant names; the fixture's names become labels.
        tenant, credential = tenant_subject(base, label, subject, issue_a if base == base_a else issue_b, agents=agents)
        subject_tokens.append(credential["token"])
        tokens.append(credential["token"])
        return tenant, credential

    def catalog(entry, enabled, revision):
        return api_request(base_b, f"/api/authorization/{receiver_tenant}/catalog", {"node": receiver_node, "entry": entry, "enabled": enabled, "expected_revision": revision}, token=mapped["token"])

    registered = False
    try:
        # The receiver's Agent executes on its Tenant's default Node, which the
        # source Tenant's policy names as a subject too.
        receiver_tenant, mapped = policy(base_b, receiver_tenant, "remote-alice", agents=(agent_ref,))
        receiver_node, address_b = default_node(base_b, receiver_tenant, mapped["token"])
        source_tenant, source = policy(base_a, source_tenant, "alice", agents=(f"{address_b}/agents/{agent_ref}",))
        _, outsider = policy(base_a, "golden-outsider", "alice", agents=())
        source_node, address_a = default_node(base_a, source_tenant, source["token"])
        token = source["token"]
        # Each Tenant's default Node accepts the other's, reaching it through the observing proxies.
        register_node(base_a, source_tenant, address_b, receiver.url, source["token"])
        register_node(base_b, receiver_tenant, address_a, home.url, mapped["token"])
        registered = True
        api_request(base_b, f"/api/authorization/{receiver_tenant}/peer-mappings", {"node": receiver_node, "source_node": address_a, "source_subject": "alice", "credential_id": mapped["credential"]["id"], "enabled": True, "expected_revision": 0}, token=mapped["token"])
        entries = [
            entity("model", "scoped-model", {"provider": "openrouter", "model_id": "fixture", "endpoint": fixture.url + "/v1", "context_window": 128000, "max_output_tokens": 4096, "modalities": ["text"], "cost": {}}),
            entity("tool", "scoped-effect", tool_descriptor(node_b, {"transport": "http", "endpoint": fixture.url + "/effect", "replay": "unsafe"}, "scoped_effect")),
            entity("agent", agent["id"], {"model": {"id": "scoped-model", "version": "1.0.0"}, "instructions": "Report progress, invoke the approved tool, then publish the result.", "schema_version": 1, "bindings": [tool_binding(node_b, "scoped-effect", "scoped_effect")], "remove_default": [], "max_steps": 16}),
        ]
        entries[1]["schema"] = {"type": "object", "required": ["scenario"], "properties": {"scenario": {"type": "string"}}, "additionalProperties": False}
        for entry in entries:
            api_request(base_b, "/api/registry", entry, token=mapped["token"])
            catalog({"id": entry["id"], "version": entry["version"]}, True, 0)

        def create_task(title):
            workspace = api_request(base_a, "/api/workspaces", {"title": title, "goal": "Verify scoped remote process recovery"}, token=token)
            task = api_request(base_a, f"/api/workspaces/{workspace['id']}/tasks", {"title": title, "description": "Use the admitted remote Agent"}, token=token)
            return workspace, task

        threading.Thread(target=approve_fixture_calls, daemon=True).start()

        # An allowed source cannot override the receiver's exact catalog approval.
        _, rejected_task = create_task("receiver-denies-dependency")
        for reference in ({"id": "scoped-model", "version": "1.0.0"}, {"id": "scoped-effect", "version": "1.0.0"}, agent):
            catalog(reference, False, 1)
            denied(base_a, f"/api/tasks/{rejected_task['id']}/delegate", {"node_id": address_b, "agent": agent}, token=token)
            catalog(reference, True, 2)
        assert counts() == initial_counts, "Denied execution created a binding/admission"

        for scenario, operation in [("home-message-recovery", "message"), ("home-completion-recovery", "run_message_complete"), ("peer-outage-recovery", "message"), ("revoked-during-worker-stop", "message")]:
            workspace, task = create_task(scenario)
            path = f"/api/tasks/{task['id']}/remote-grants"
            grant = str(uuid.uuid4())
            # The grant must outlive both recovery waits and the setup between them.
            preparation = {"id": grant, "node_id": address_b, "agent": agent, "ttl_seconds": 2 * RECOVERY_BUDGET_SECONDS + 120}
            prepared = api_request(base_a, path, preparation, token=token)
            assert api_request(base_a, path, preparation, token=token) == prepared
            denied(base_a, path, {**preparation, "id": str(uuid.uuid4())}, token=outsider["token"])
            home.arm(grant, operation)
            activated = api_request(base_a, f"{path}/{grant}/activate", {}, token=token)
            run_id = activated["run_id"]
            owned_grants.append((task["id"], grant))
            assert activated["admission_id"] == run_id
            assert api_request(base_a, f"{path}/{grant}/activate", {}, token=token)["run_id"] == run_id

            # The legacy path is not served, and a request without a Federation
            # token that a registered Node signed is refused. #371 removed the
            # legacy route, so the dashboard's GET-only asset route answers the
            # POST with 405.
            peer_headers = {"x-aidash-protocol": "0.4"}
            denied(base_b, "/federation/v0.1/offers", {"task": task, "agent": agent}, codes=(401, 403, 404, 405), token="not-a-federation-token", headers=peer_headers)
            denied(base_a, "/federation/v0.1/scoped/execution/commands", {"grant_id": grant, "admission_id": run_id, "operation": "workspace_record", "data": {"kind": "task", "id": rejected_task["id"]}}, codes=(401, 403, 404), token="not-a-federation-token", headers=peer_headers)
            def run(run_id=run_id):
                return api_request(base_b, f"/api/runs/{run_id}", token=mapped["token"])["run"]

            def progress(grant=grant, scenario=scenario):
                stalled = run()
                commands = {name: len(attempts) for (owner, name), attempts in home.commands.items() if owner == grant}
                timings = {name: {"total": round(sum(values), 3), "max": max(values)} for (owner, name), values in home.timings.items() if owner == grant}
                return f"phase={stalled['phase']}, step={stalled['step']}, error={stalled.get('error')}, model_calls={fixture.model_calls[scenario]}, Home commands={commands}, RPC seconds={timings}, peer_errors={home.errors}, provider_errors={fixture.errors}"

            # Completion also includes the external-action approval continuation.
            if not home.blocked.wait(RECOVERY_BUDGET_SECONDS):
                raise AssertionError(f"Did not reach committed {operation}: {progress()}")

            before = run()
            assert before["agent_id"] == agent["id"] and before["agent_version"] == agent["version"]
            assert before["task_id"] == task["id"] and before["home_node"] == address_a
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
                stopped = api_request(base_a, f"/api/workspaces/{workspace['id']}", token=token)
                assert not stopped["artifacts"] and stopped["tasks"][0]["status"] != "COMPLETED"
                denied(base_a, f"{path}/{grant}/control", {"action": "resume"}, token=token)
                api_request(base_a, f"{path}/{grant}/control", {"action": "cancel"}, token=token)
                wait_for(lambda: run()["phase"] == "CANCELLED", label="revoked remote run cancellation")
                assert api_request(base_a, f"/api/workspaces/{workspace['id']}", token=token)["tasks"][0]["status"] == "CANCELLED"
            else:
                try:
                    wait_for(lambda: run()["phase"] == "COMPLETED", timeout=RECOVERY_BUDGET_SECONDS, label=scenario)
                except AssertionError as error:
                    raise AssertionError(f"{error}; {progress()}") from None
                if scenario == "home-completion-recovery":
                    assert fixture.model_calls[scenario] == before_calls, "Recovery repeated committed inference"
                else:
                    # A committed message leaves only the tool request and final
                    # response, including when recovery first meets a peer outage.
                    assert fixture.model_calls[scenario] == before_calls + 2, "Message recovery must perform exactly two remaining inferences"
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
            # The receiver holds no row for the source-owned workspace, so a
            # receiver subject's state never lists this Run, and a Deployment
            # Administrator holds no Tenant content (#484): read the admitted
            # Run by id as the receiver's mapped subject. The admission and Home
            # binding counts below catch a duplicate.
            admitted = run()
            assert admitted["id"] == run_id and admitted.get("home_node", address_a) == address_a
            statuses = api_request(base_a, f"/api/tasks/{task['id']}/remote-executions", token=token)
            assert len(statuses) == 1 and statuses[0]["execution"]["run_id"] == run_id
            assert statuses[0]["execution"]["phase"] == run()["phase"]
            assert not any(secret in json.dumps(statuses) for secret in tokens), "Execution status exposed a credential"
            # Terminal scenarios no longer need an approval poller. Keep the
            # fixture workload bounded to the one execution under observation.
            owned_grants.remove((task["id"], grant))
            scenarios.append({"scenario": scenario, "task_id": task["id"], "workspace_id": workspace["id"], "grant_id": grant, "admission_id": activated["admission_id"], "run_id": run_id, "worker_pid_before": old_pid, "worker_pid_after": worker.pid, "exit_signal": "SIGKILL", "phase": run()["phase"], "home_command_attempts": len(home.commands[(grant, operation)]), "passed": True})
            print(f"Scoped Golden Path passed: {scenario} (same Run {run_id}, Worker {old_pid} -> {worker.pid})", flush=True)

        final_counts = counts()
        assert all(final_counts[key] - initial_counts[key] == len(scenarios) for key in initial_counts), "Retries created an extra admission or Home binding"
        assert not home.errors and not receiver.errors and not fixture.errors, (home.errors, receiver.errors, fixture.errors)
        assert not any("/offers" in path for proxy in (home, receiver) for path in proxy.paths), "Scoped work fell back to legacy admission"
        assert receiver.paths["/federation/v0.1/scoped/execution/admissions"] > 0
        return {"passed": True, "source_tenant": source_tenant, "receiver_tenant": receiver_tenant, "scenarios": scenarios, "admissions": final_counts["admissions"] - initial_counts["admissions"], "home_bindings": final_counts["bindings"] - initial_counts["bindings"], "negative_checks": ["cross_tenant", "cross_workspace", "receiver_dependency_approval", "disabled_agent", "legacy_admission", "revoked_resume"], "subject_credentials_not_forwarded": True}
    finally:
        approval_stop.set()
        home.release.set()
        home.unavailable.clear()
        if registered:
            # Leave the Nodes reaching one another directly, as other branches expect.
            register_node(base_a, source_tenant, address_b, base_b, source["token"])
            register_node(base_b, receiver_tenant, address_a, base_a, mapped["token"])
        home.close()
        receiver.close()
        fixture.close()
