#!/usr/bin/env python3
"""Two real Aidash nodes with protocol fixtures, PostgreSQL, NATS and SIGKILL.

The deterministic model server is a test double, not a live model-quality test.
Only task-created databases and child processes are removed by this script.
"""

import argparse
import collections
import hashlib
import http.server
import json
import os
import pathlib
import runpy
import secrets
import signal
import select
import socket
import socketserver
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid

import tool_call_approvals

ROOT = pathlib.Path(__file__).resolve().parents[1]
TOKEN = "acceptance-access-token"
PEER_TOKEN = "acceptance-peer-token-for-local-tests"


def api_request(base, path, body=None, token=TOKEN, headers=None):
    request = urllib.request.Request(
        base + path,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json", **(headers or {})},
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)


def tenant_subject(base, label, subject, agents=()):
    """Create a Tenant whose policy lets its subjects do anything, and issue `subject` a credential.

    Only Tenant subjects register definitions and run work (#371); the
    Deployment Administrator creates the Tenant, writes its first policy and
    issues the credential.
    """
    tenant = api_request(base, "/api/tenants", {"label": label})["name"]
    subjects = {subject: {"kind": "user"}, **{agent: {"kind": "agent"} for agent in agents}}
    bundle = {"tenant": tenant, "subjects": subjects, "policies": [{"id": "fixture-work", "effect": "allow", "subjects": {"any": True}, "actions": ["*"], "resources": {"kinds": ["*"]}}]}
    api_request(base, f"/api/authorization/{tenant}", {"expected_revision": 1, "bundle": bundle})
    credential = api_request(base, f"/api/authorization/{tenant}/credentials", {"subject": subject, "expires_in_seconds": 3600})
    return tenant, credential


def approve(base, tenant, token, entries):
    """Approve definitions in the Tenant's catalog as one dependency closure, so its subjects may run them."""
    approvals = [{"entry": {"id": entry["id"], "version": entry["version"]}, "enabled": True, "expected_revision": 0} for entry in entries]
    api_request(base, f"/api/authorization/{tenant}/catalog/approvals", {"approvals": approvals}, token=token)


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


def reconcile_uncertain_http(base, run_id, effects, token=TOKEN):
    """Confirm a lost HTTP result from the fixture's recorded external receipt."""
    def uncertain():
        run = api_request(base, f"/api/runs/{run_id}", token=token)["run"]
        if run["phase"] == "WAITING" and run["state"]["data"]["reason"] == "reconciliation":
            return run["state"]["data"]
        return None

    waiting = wait_for(uncertain, label="uncertain HTTP invocation after Worker crash")
    key = waiting["key"]
    receipts = effects()
    assert key in receipts, "Reconciliation requires the exact external invocation receipt"
    api_request(base, f"/api/human-requests/{waiting['request_id']}/answer", {"result": receipts[key]}, token=token)
    return key


FIXTURE_TOOLS = ("research-http", "http-runner-tool", "mcp-runner-tool")


def approve_fixture_external_requests(base, token=TOKEN):
    """Approve only the owned fixture's exact external-call continuations of Runs executing for another Node.

    Such a Run asks this Node through a human request; a Run on this Node raises a Tool call
    approval, which approve_fixture_tool_calls answers.
    """
    for request in api_request(base, "/api/state", token=token)["human_requests"]:
        prompt = request["prompt"]
        if request.get("response") is None and request["kind"] == "APPROVAL_REQUIRED" and any(
            prompt.startswith(f"Approve this exact external tool action once? Tool: {tool}@")
            for tool in FIXTURE_TOOLS
        ):
            api_request(base, f"/api/human-requests/{request['id']}/answer", {"approved": True}, token=token)


def approve_fixture_tool_calls(base, token):
    """Allow once each exact call of the owned fixture's tools that a Run on this Node asks a Tool call approval for.

    `token` must be a Tenant subject's: a Deployment Administrator cannot list a Tenant's approvals.
    """
    for card in tool_call_approvals.pending(lambda path: api_request(base, path, token=token)):
        if card.get("tool") in FIXTURE_TOOLS:
            tool_call_approvals.decide(lambda path, body: api_request(base, path, body, token=token), card, True)


def diagnose(base, token, tenant, workspace):
    """Print a stalled workspace's tasks and Runs, and its Tenant's policy, catalog and every policy decision."""
    def show(label, value):
        print(f"--- {label}: {json.dumps(value, ensure_ascii=False)}", flush=True)

    try:
        snapshot = api_request(base, f"/api/workspaces/{workspace}", token=token)
        show("tasks", [{key: task.get(key) for key in ("id", "title", "status", "parent_id")} for task in snapshot["tasks"]])
        tasks = {task["id"] for task in snapshot["tasks"]}
        for run in api_request(base, "/api/state", token=token)["runs"]:
            if run["task_id"] in tasks:
                detail = api_request(base, f"/api/runs/{run['id']}", token=token)["run"]
                response = ((detail.get("state") or {}).get("data") or {}).get("response") or {}
                show(f"run {run['id']}", {**{key: detail.get(key) for key in ("agent_id", "phase", "control", "step", "error")}, "tool_calls": response.get("tool_calls")})
        show("policy", api_request(base, f"/api/authorization/{tenant}"))
        show("catalog", api_request(base, f"/api/authorization/{tenant}/catalog", token=token))
        decisions, after = [], 0
        while page := api_request(base, f"/api/authorization/{tenant}/decisions?after={after}&limit=100"):
            decisions += page
            after = page[-1]["sequence"]
        show("decisions", [[d["sequence"], d["created_at"][11:23], d["action"], d["decision"]["allowed"], d["subject"], d["resource_id"]] for d in decisions])
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"--- diagnosis failed: {error!r}", flush=True)


def entity(kind, identifier, config, capability="web.search"):
    return {
        "id": identifier, "version": "1.0.0", "kind": kind,
        "name": {"en": identifier.replace("-", " ").title(), "ja": {"coordinator": "調査コーディネーター", "researcher": "調査エージェント"}.get(identifier, identifier)},
        "description": {"en": "Golden path protocol fixture", "ja": "受入試験用のプロトコルフィクスチャ"},
        "capabilities": [capability], "tags": ["acceptance"], "languages": ["ja", "en"],
        "skills": [], "schema": {}, "config": config,
    }


def artifact_read_state(history, artifact_id):
    chunks = [
        (
            event["call"].get("arguments", {}),
            event.get("result", {}),
        )
        for event in history
        if event.get("kind") == "tool"
        and event.get("call", {}).get("name") == "workspace_read"
        and event.get("call", {}).get("arguments", {}).get("kind") == "artifact"
        and event.get("call", {}).get("arguments", {}).get("id") == artifact_id
        and isinstance(event.get("result", {}).get("content"), str)
    ]
    chunks.sort(key=lambda item: item[0].get("offset", 0))
    offset = 0
    content = []
    for arguments, result in chunks:
        if arguments.get("offset", 0) != offset:
            continue
        piece = result["content"]
        content.append(piece)
        offset += len(piece)
        if result.get("next_offset") is None:
            return json.loads("".join(content))["content"], None
    return None, offset


def tool_descriptor(node, transport, alias="research_lookup"):
    """Declare an integration without asserting provider behavior from its config."""
    return {"registry_node": node, "provider": "integration." + transport["transport"] + "@1",
            "operation": "invoke", "default_alias": alias, "tier": "integration", "transport": transport}


def tool_binding(node, name, alias="research_lookup"):
    return {"kind": "tool", "target": {"registry_node": node, "id": name, "version": "1.0.0"},
            "alias": alias, "narrow": {}}


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
        assert body["provider"]["zdr"] is True
        self.provider_calls["openrouter"] += 1
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

        if current["identity"]["agent_id"] in ["http-runner", "mcp-runner"]:
            if not any(name == "research_lookup" for name, _ in tools):
                call("research_lookup", {"message": "plugin roundtrip"})
            else:
                text = "Plugin completed: " + json.dumps(next(result for name, result in tools if name == "research_lookup"))
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
                contents = []
                waiting_for_reads = False
                for artifact in workspace["artifacts"]:
                    content, offset = artifact_read_state(history, artifact["id"])
                    if offset is not None:
                        waiting_for_reads = True
                        call("workspace_read", {"kind": "artifact", "id": artifact["id"], "offset": offset, "max_chars": 8000})
                    else:
                        contents.append(str(content))
                if not waiting_for_reads:
                    text = "Axum・Actix・Rocketの調査が完了しました。各エージェントの成果物を統合しました。\n\n" + "\n".join(contents)
        elif not any(name == "research_lookup" for name, _ in tools):
            call("research_lookup", {"topic": task["title"]})
        else:
            text = f"{task['title']}: " + str(next(result for name, result in tools if name == "research_lookup"))
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
                if self.path == "/echo":
                    result = body
                elif self.path == "/research":
                    key = self.headers.get("Idempotency-Key")
                    assert key, "Tool adapter must propagate an idempotency key"
                    if body["topic"] == "Actix" and key not in fixture.effects:
                        # Let the other two results return and persist first, so
                        # a Worker kill on a shared Node leaves exactly this
                        # invocation uncertain. The adapter times out after 10 s.
                        deadline = time.monotonic() + 3
                        while time.monotonic() < deadline:
                            with fixture.lock:
                                if {"Axum", "Rocket"} <= {effect["topic"] for effect in fixture.effects.values()}:
                                    break
                            time.sleep(0.1)
                        time.sleep(1)
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


# Compose's init script creates these logins (#262). Migrations run as the owner
# of every table; servers and workers as the role row-level security binds.
OWNER_LOGIN = "aidash_owner:aidash-owner-local"
SERVER_LOGIN = "aidash_server:aidash-server-local"


def database_url(login, database):
    return f"postgres://{login}@127.0.0.1:{os.environ.get('AIDASH_POSTGRES_PORT', '54370')}/{database}"


def provision(database):
    """Create a database as a deployment does: owned by aidash_owner, with the
    superuser-only extensions and the server role's default privileges."""
    psql("aidash_a", f"CREATE DATABASE {database} OWNER aidash_owner TEMPLATE template0")
    psql(database, "CREATE EXTENSION pg_jsonschema WITH SCHEMA public; CREATE EXTENSION vector; "
         "ALTER DEFAULT PRIVILEGES FOR ROLE aidash_owner GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO aidash_server; "
         "ALTER DEFAULT PRIVILEGES FOR ROLE aidash_owner GRANT USAGE, SELECT ON SEQUENCES TO aidash_server")
    # PGroonga's crash-safe worker starts asynchronously; each psql call is a
    # fresh backend, and a failed start is remembered only by its own backend.
    deadline = time.monotonic() + 30
    for statement in ("CREATE EXTENSION IF NOT EXISTS pgroonga", "SELECT pgroonga_command('io_flush')"):
        while True:
            try:
                psql(database, statement)
                break
            except subprocess.CalledProcessError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.1)


class NatsProxy(socketserver.ThreadingTCPServer):
    """Bind a private endpoint now; accept connections only after the outage test."""

    daemon_threads = True

    def __init__(self):
        super().__init__(("127.0.0.1", 0), NatsForwarder, bind_and_activate=False)
        self.server_bind()
        self.started = False

    def restore(self):
        self.server_activate()
        self.started = True
        threading.Thread(target=self.serve_forever, daemon=True).start()

    def close(self):
        if self.started:
            self.shutdown()
        self.server_close()


def forward(client, upstream_port):
    with socket.create_connection(("127.0.0.1", upstream_port), timeout=10) as upstream:
        upstream.settimeout(None)
        sockets = (client, upstream)
        try:
            while True:
                ready, _, _ = select.select(sockets, [], [], 30)
                for source in ready:
                    data = source.recv(65536)
                    if not data:
                        return
                    destination = upstream if source is client else client
                    destination.sendall(data)
        except OSError:
            pass


class NatsForwarder(socketserver.BaseRequestHandler):
    def handle(self):
        forward(self.request, int(os.environ.get("AIDASH_NATS_PORT", "42270")))


class KeylessFixtureProxy(socketserver.ThreadingTCPServer):
    """Own a reserved Keyless fixture port for the whole run.

    The nodes permit only the origins fixed at launch, and a dashboard spec's
    Node server cannot adopt a socket bound here. So this proxy takes over the
    held `listener` and forwards each connection to the port the spec publishes
    in `upstream_file` (web/tests/keyless-ports.ts); no other process can claim
    the permitted port between launch and the spec's turn.
    """

    daemon_threads = True

    def __init__(self, listener, upstream_file):
        super().__init__(listener.getsockname(), KeylessFixtureForwarder, bind_and_activate=False)
        self.socket.close()
        self.socket = listener
        self.server_address = listener.getsockname()
        self.upstream_file = upstream_file
        self.started = False

    def start(self):
        self.server_activate()
        self.started = True
        threading.Thread(target=self.serve_forever, daemon=True).start()

    def close(self):
        if self.started:
            self.shutdown()
        self.server_close()


class KeylessFixtureForwarder(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            upstream_port = int(self.server.upstream_file.read_text())
        except (OSError, ValueError):
            # The spec is not serving yet; closing fails the call instead of hanging it.
            return
        try:
            forward(self.request, upstream_port)
        except OSError:
            pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default=os.environ.get("AIDASH_BINARY", "target/debug/aidash"))
    parser.add_argument("--keep", action="store_true", help="Keep the completed local demo running until interrupted")
    parser.add_argument("--dashboard", action="store_true", help="Submit the golden path goal through Chromium instead of the API")
    args = parser.parse_args()
    revision = runpy.run_path(str(ROOT / "scripts/core-capability-evidence.py"))["revision"]
    source = revision()
    binary_sha256 = hashlib.sha256(pathlib.Path(args.binary).read_bytes()).hexdigest()
    query_helper = pathlib.Path(args.binary).resolve().parent / "manage"
    queries = json.loads(subprocess.check_output([str(query_helper), "diagnostics", "acceptance"], text=True))
    run_id = uuid.uuid4().hex[:12]
    db_a, db_b = f"aidash_e2e_{run_id}_a", f"aidash_e2e_{run_id}_b"
    node_a, node_b = f"aidash://acceptance-{run_id}-a", f"aidash://acceptance-{run_id}-b"
    port_a = int(os.environ.get("AIDASH_E2E_PORT_A", "18080"))
    port_b = int(os.environ.get("AIDASH_E2E_PORT_B", "18081"))
    base_a, base_b = f"http://127.0.0.1:{port_a}", f"http://127.0.0.1:{port_b}"
    logs = ROOT / "private" / "acceptance"
    logs.mkdir(parents=True, exist_ok=True)
    (logs / "report.json").unlink(missing_ok=True)
    (logs / "source.json").write_text(json.dumps({"source": source, "binary_sha256": binary_sha256}, indent=2) + "\n")
    children = []
    files = []
    # Binding defaults include File Tools. Enable only disposable fixture
    # storage; command runners remain disabled in the Node's execution profile.
    profile_directory = tempfile.TemporaryDirectory(prefix="aidash-golden-profile-")
    profile = pathlib.Path(profile_directory.name) / "profile.json"
    profile.write_text(json.dumps({"admission": True, "storage": str(profile.parent / "storage")}))
    # Compaction Deciders name the Tenant's OpenRouter Provider Credential
    # (ADR 0041), so each Node runs a disposable PostgreSQL Provider Credential
    # Store whose keys exist only for this run.
    provider_credentials = pathlib.Path(profile_directory.name) / "provider-credentials.json"
    provider_credentials.write_text(json.dumps({"provider_credentials": {
        "fingerprint_key": {"env": "AIDASH_PROVIDER_FINGERPRINT_KEY"},
        "store": {"kind": "postgres", "master_key": {"env": "AIDASH_PROVIDER_STORE_MASTER_KEY"}},
    }}))
    provider_credential_keys = {"AIDASH_PROVIDER_CREDENTIAL_SETTINGS": str(provider_credentials),
                                "AIDASH_PROVIDER_FINGERPRINT_KEY": secrets.token_hex(32),
                                "AIDASH_PROVIDER_STORE_MASTER_KEY": secrets.token_hex(32)}
    # The research cluster runs on Node A alone; scoped_golden_path covers
    # execution across Nodes.
    fixture = Fixture(node_a, node_a)
    fixture_server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), fixture.handler())
    threading.Thread(target=fixture_server.serve_forever, daemon=True).start()
    fixture_url = f"http://127.0.0.1:{fixture_server.server_port}"
    # Keyless model endpoints on loopback reach only origins the Deployment
    # permits, so bind the scoped branch's provider socket before launch and
    # hold it until the provider takes it over; no other process can claim
    # the permitted port in between.
    scoped_provider_socket = socket.socket()
    scoped_provider_socket.bind(("127.0.0.1", 0))
    scoped_provider_port = scoped_provider_socket.getsockname()[1]
    # The dashboard specs reach their Keyless model and embedding fixtures
    # through these ports, one per spec (web/tests/keyless-ports.ts). Each is
    # held from now on and forwarded to the port its spec publishes.
    keyless_upstreams = tempfile.TemporaryDirectory(prefix="aidash-keyless-upstreams-")
    browser_provider_proxies = []
    for index in range(3):
        held = socket.socket()
        held.bind(("127.0.0.1", 0))
        browser_provider_proxies.append(KeylessFixtureProxy(held, pathlib.Path(keyless_upstreams.name) / str(index)))
    browser_provider_ports = [proxy.server_address[1] for proxy in browser_provider_proxies]
    keyless_origins = ",".join([fixture_url, f"http://127.0.0.1:{scoped_provider_port}",
                                *(f"http://127.0.0.1:{port}" for port in browser_provider_ports)])
    stream_events = []
    nats_proxy = NatsProxy()
    # Each Node's Tenant subject credential, by base URL.
    tokens = {}

    approval_stop = threading.Event()
    local_approvals = threading.Event()
    local_approvals.set()
    approval_errors = []

    def approve_fixture_calls():
        while not approval_stop.wait(0.25):
            if not local_approvals.is_set():
                continue
            for base, token in list(tokens.items()):
                try:
                    approve_fixture_external_requests(base, token)
                    approve_fixture_tool_calls(base, token)
                except OSError:
                    # A deliberately unavailable/restarting Node has no approval authority.
                    pass
                except Exception as error:
                    approval_errors.append(str(error))

    def node_env(node, database, port, login):
        env = {**os.environ, "DATABASE_URL": database_url(login, database), "NATS_URL": f"nats://127.0.0.1:{nats_proxy.server_address[1]}", "AIDASH_NODE_ID": node, "AIDASH_ENDPOINT": f"http://127.0.0.1:{port}", "AIDASH_LISTEN": f"127.0.0.1:{port}", "AIDASH_API_TOKEN": TOKEN, "AIDASH_SECRET_PEER": PEER_TOKEN, "AIDASH_SECRET_TRANSACTION_FIXTURE": "local-transaction-fixture-key-0123456789", "AIDASH_WEB_DIR": str(ROOT / "web/dist"), "AIDASH_KEYLESS_PRIVATE_ORIGINS": keyless_origins}
        # Browser fixtures create audit/history pages in bursts alongside UI polling.
        env["AIDASH_API_RATE_BURST"] = os.environ.get("AIDASH_API_RATE_BURST", "1000")
        env["AIDASH_CAPABILITY_PROFILE"] = str(profile)
        env.update(provider_credential_keys)
        # These disposable Node IDs own their JetStream streams and consumers.
        # When the outage proxy opens, recover real activation delivery as well
        # as event publication instead of retaining fallback for the whole run.
        env["AIDASH_ACTIVATION_BOOTSTRAP"] = "true"
        return env

    def migrate(node, database, port):
        with open(logs / f"{database}-migrate.log", "w") as log:
            subprocess.run([args.binary, "migrate"], cwd=ROOT, env=node_env(node, database, port, OWNER_LOGIN),
                           stdout=log, stderr=log, check=True, timeout=600)

    def launch(node, database, port, mode):
        env = node_env(node, database, port, SERVER_LOGIN)
        log = open(logs / f"{database}-{mode}-{len(children)}.log", "w")
        files.append(log)
        child = subprocess.Popen([args.binary, mode], cwd=ROOT, env=env, stdout=log, stderr=log)
        children.append(child)
        return child

    def sse():
        request = urllib.request.Request(base_a + "/api/events/stream", headers={"Authorization": f"Bearer {tokens[base_a]}"})
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                for line in response:
                    if line.startswith(b"data:"):
                        stream_events.append(json.loads(line[5:]))
        except (OSError, ValueError):
            pass

    try:
        # Create isolated databases; application schema belongs to the native
        # history, which only the owner applies before the servers start.
        provision(db_a)
        provision(db_b)
        migrate(node_a, db_a, port_a)
        migrate(node_b, db_b, port_b)
        launch(node_a, db_a, port_a, "server")
        launch(node_b, db_b, port_b, "server")
        wait_for(lambda: api_request(base_a, "/health"), label="Node A")
        wait_for(lambda: api_request(base_b, "/health"), label="Node B")
        tenant_a, subject_a = tenant_subject(base_a, "golden-research", "alice")
        tenant_b, subject_b = tenant_subject(base_b, "golden-plugins", "bob")
        tokens.update({base_a: subject_a["token"], base_b: subject_b["token"]})
        tenants = {base_a: tenant_a, base_b: tenant_b}

        def register(base, entries):
            for entry in entries:
                api_request(base, "/api/registry", entry, token=tokens[base])
            approve(base, tenants[base], tokens[base], entries)

        model = entity("model", "fixture-model", {"provider": "openrouter", "model_id": "protocol-fixture", "endpoint": fixture_url + "/v1", "context_window": 256000, "max_output_tokens": 4096, "modalities": ["text"], "cost": {"currency": "USD", "input_per_million": 0}})
        tool = entity("tool", "research-http", tool_descriptor(node_a, {"transport": "http", "endpoint": fixture_url + "/research", "replay": "unsafe"}))
        tool["schema"] = {"type": "object", "required": ["topic"], "properties": {"topic": {"type": "string"}}, "additionalProperties": False}
        register(base_a, [
            model, tool,
            entity("agent", "coordinator", {"model": {"id": "fixture-model", "version": "1.0.0"}, "instructions": "Discover researchers, decompose the goal, delegate, wait and synthesize.", "schema_version": 1, "bindings": [], "remove_default": [], "cluster": None, "max_steps": 128}, capability="task.coordinate"),
            entity("cluster", "research-cluster", {"coordinator": {"id": "coordinator", "version": "1.0.0"}}),
            entity("agent", "researcher", {"model": {"id": "fixture-model", "version": "1.0.0"}, "instructions": "Research one framework and publish your findings.", "schema_version": 1, "bindings": [tool_binding(node_a, "research-http")], "remove_default": [], "cluster": {"id": "research-cluster", "version": "1.0.0"}, "max_steps": 64}),
        ])
        register(base_b, [model])
        discovered = api_request(base_a, "/api/discover", {"capability": "web.search", "language": "ja"}, token=tokens[base_a])
        assert {a["node_id"] for a in discovered["agents"]} == {node_a}
        threading.Thread(target=sse, daemon=True).start()
        worker_a = launch(node_a, db_a, port_a, "worker")
        remote_worker = launch(node_b, db_b, port_b, "worker")
        threading.Thread(target=approve_fixture_calls, daemon=True).start()
        if args.dashboard:
            created = json.loads(subprocess.check_output(["node", "web/scripts/start-goal.mjs"], cwd=ROOT, text=True, env={**os.environ, "AIDASH_E2E_URL": base_a, "AIDASH_E2E_TOKEN": tokens[base_a], "AIDASH_E2E_TENANT": tenant_a, "AIDASH_E2E_SUBJECT": "alice"}))
        else:
            created = api_request(base_a, "/api/conversations", {"title": "Rustフレームワークの競合調査", "goal": "Rust製Webフレームワークについて競合調査して", "target": {"id": "research-cluster", "version": "1.0.0"}, "target_kind": "cluster"}, token=tokens[base_a])
        workspace = created["workspace"]["id"]
        if not fixture.remote_effect_started.wait(90):
            diagnose(base_a, tokens[base_a], tenant_a, workspace)
            raise AssertionError("The Actix research tool was never invoked")

        def actix_run():
            actix = next((t for t in api_request(base_a, f"/api/workspaces/{workspace}", token=tokens[base_a])["tasks"] if t["title"] == "Actix"), None)
            return actix and next((r for r in api_request(base_a, "/api/state", token=tokens[base_a])["runs"] if r["task_id"] == actix["id"]), None)

        worker_a.kill()
        worker_a.wait(timeout=10)
        print("Killed Node A worker after the Actix tool effect, before result persistence", flush=True)
        worker_a = launch(node_a, db_a, port_a, "worker")
        before = wait_for(actix_run, label="Actix research run")
        reconciled_key = reconcile_uncertain_http(base_a, before["id"], lambda: dict(fixture.effects), tokens[base_a])

        def complete():
            snapshot = api_request(base_a, f"/api/workspaces/{workspace}", token=tokens[base_a])
            failures = [t for t in snapshot["tasks"] if t["status"] in ["FAILED", "CANCELLED"]]
            if failures:
                print("Task failure:", json.dumps(failures), flush=True)
                return False
            return snapshot if len(snapshot["tasks"]) == 4 and all(t["status"] == "COMPLETED" for t in snapshot["tasks"]) else False

        snapshot = wait_for(complete, timeout=150, label="all four tasks and artifacts")
        recovered = wait_for(lambda: next((r for r in api_request(base_a, "/api/state", token=tokens[base_a])["runs"] if r["id"] == before["id"] and r["phase"] == "COMPLETED"), None), label="same durable research run")
        assert recovered["id"] == before["id"]
        assert any(e["kind"] == "run.recovered" and e["data"].get("run_id") == recovered["id"] for e in api_request(base_a, "/api/events", token=tokens[base_a]))
        assert len(snapshot["artifacts"]) == 4
        assert len(fixture.effects) == 3
        assert fixture.requests[reconciled_key] == 1, "An uncertain Unsafe HTTP invocation must not be replayed automatically"
        assert fixture.provider_calls["openrouter"] > 0
        wait_for(lambda: any(e["type"] == "task.completed" for e in stream_events), label="SSE result delivery")
        pending_events = int(psql(db_a, queries["pending_events"]))
        assert pending_events > 0, "Outage must leave durable events awaiting publication"
        assert int(psql(db_a, queries["inbox"])) == 0
        print(f"Servers, workers and SIGKILL recovery passed with NATS unavailable; {pending_events} events queued", flush=True)
        nats_proxy.restore()
        wait_for(lambda: int(psql(db_a, queries["pending_events"])) == 0, label="outbox flush after broker recovery")
        wait_for(lambda: int(psql(db_a, queries["inbox"])) > 0, label="JetStream durable consumption")
        assert len({e["id"] for e in stream_events}) == len(stream_events)
        last_sequence = stream_events[-1]["sequence"]
        replay = api_request(base_a, f"/api/events?after={last_sequence - 1}", token=tokens[base_a])
        assert replay[0]["sequence"] == last_sequence
        plugin_runs = []
        for base, agent_id, tool_config in [
            (base_b, "http-runner", {"transport": "http", "endpoint": fixture_url + "/echo", "replay": "unsafe"}),
            (base_a, "mcp-runner", {"transport": "mcp", "endpoint": fixture_url + "/mcp", "tool_name": "echo", "replay": "read_only", "idempotency_argument": None}),
        ]:
            tool_id = agent_id + "-tool"
            node = node_a if base == base_a else node_b
            register(base, [
                entity("tool", tool_id, tool_descriptor(node, tool_config)),
                entity("agent", agent_id, {"model": {"id": "fixture-model", "version": "1.0.0"}, "instructions": "Verify the registered plugin, then return its result.", "schema_version": 1, "bindings": [tool_binding(node, tool_id)], "remove_default": [], "max_steps": 32}),
            ])
            plugin_conversation = api_request(base, "/api/conversations", {"title": agent_id, "goal": "Execute the configured plugin", "target": {"id": agent_id, "version": "1.0.0"}, "target_kind": "agent"}, token=tokens[base])
            plugin_workspace = plugin_conversation["workspace"]["id"]
            result = wait_for(lambda base=base, plugin_workspace=plugin_workspace, task_id=plugin_conversation["task"]["id"]: next((t for t in api_request(base, f"/api/workspaces/{plugin_workspace}", token=tokens[base])["tasks"] if t["id"] == task_id and t["status"] == "COMPLETED"), None), label=agent_id)
            assert result["status"] == "COMPLETED"
            plugin_runs.append(agent_id)
        # This branch is mandatory: legacy recovery alone cannot establish
        # subject-scoped authority or the remote grant/admission contract.
        from scoped_golden_path import verify
        # Scoped scenarios own their exact Home approval poller; completed local
        # fixtures do not need a second full-state poller during fault injection.
        local_approvals.clear()
        scoped = verify(base_a, base_b, node_a, node_b, remote_worker, scoped_provider_socket,
                        lambda: launch(node_b, db_b, port_b, "worker"),
                        lambda: {"admissions": int(psql(db_b, queries["remote_admissions"])),
                                 "bindings": int(psql(db_a, queries["remote_bindings"]))})
        report = {"node_a": base_a, "node_b": base_b, "node_ids": [node_a, node_b], "workspace_id": workspace, "tasks": len(snapshot["tasks"]), "artifacts": len(snapshot["artifacts"]), "external_effects": len(fixture.effects), "tool_requests": dict(fixture.requests), "provider_calls": dict(fixture.provider_calls), "recovered_run_id": recovered["id"], "sse_events": len(stream_events), "database_a": db_a, "database_b": db_b, "goal_entry": "dashboard" if args.dashboard else "api", "nats_outage_startup_and_recovery": "passed", "events_queued_during_outage": pending_events, "additional_plugins": plugin_runs}
        report.update(source=source, binary_sha256=binary_sha256, scoped_remote_execution=scoped)
        local_approvals.set()
        if args.dashboard:
            browser_report = logs / "browser-report.json"
            for proxy in browser_provider_proxies:
                proxy.start()
            subprocess.run(["npm", "test", "--prefix", "web", "--", "--reporter=list,json"], cwd=ROOT, check=True, env={**os.environ, "PLAYWRIGHT_JSON_OUTPUT_FILE": str(browser_report), "AIDASH_E2E_URL": base_a, "AIDASH_E2E_REMOTE_URL": base_b, "AIDASH_E2E_KEYLESS_PORTS": ",".join(map(str, browser_provider_ports)), "AIDASH_E2E_KEYLESS_UPSTREAMS": keyless_upstreams.name})
            report["browser_scenarios"] = json.loads(browser_report.read_text())["stats"]["expected"]
        assert not approval_errors, approval_errors
        assert source == revision(), "Source changed while Golden Path was running"
        assert binary_sha256 == hashlib.sha256(pathlib.Path(args.binary).read_bytes()).hexdigest(), "Binary changed while Golden Path was running"
        (logs / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2))
        print("Golden path passed:", json.dumps(report, ensure_ascii=False), flush=True)
        if args.keep:
            (ROOT / "private/e2e-ready.json").write_text(json.dumps(report))
            while True:
                time.sleep(1)
    finally:
        approval_stop.set()
        for child in children:
            if child.poll() is None:
                child.send_signal(signal.SIGTERM)
        for child in children:
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
        nats_proxy.close()
        for proxy in browser_provider_proxies:
            proxy.close()
        keyless_upstreams.cleanup()
        fixture_server.shutdown()
        for file in files:
            file.close()
        for db in [db_a, db_b]:
            psql("aidash_a", f"DROP DATABASE IF EXISTS {db} WITH (FORCE)")
        (ROOT / "private/e2e-ready.json").unlink(missing_ok=True)
        profile_directory.cleanup()


if __name__ == "__main__":
    main()
