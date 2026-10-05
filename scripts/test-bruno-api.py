#!/usr/bin/env python3
"""Run Bruno against a real Aidash process and a disposable PostgreSQL database.

Only the external OIDC provider is a protocol fixture. Aidash routes, dependency
composition, authorization, cookies, CSRF, persistence and migrations are real.
"""

import argparse
import base64
import contextlib
import hashlib
import http.server
import json
import os
import pathlib
import re
import secrets
import shutil
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.parse
import urllib.request
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]
COLLECTION = ROOT / "server/tests/bruno"


def base64url(value):
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode("ascii")


def checked(command, **kwargs):
    return subprocess.run(command, check=True, capture_output=True, **kwargs)


class IdentityProvider:
    """A private RS256 issuer with bound authorization codes and S256 PKCE."""

    def __init__(self, directory):
        self.key = directory / "issuer-key.pem"
        checked(
            [
                "openssl",
                "genpkey",
                "-algorithm",
                "RSA",
                "-pkeyopt",
                "rsa_keygen_bits:2048",
                "-out",
                str(self.key),
            ]
        )
        self.key.chmod(0o600)
        modulus = (
            checked(
                ["openssl", "rsa", "-in", str(self.key), "-noout", "-modulus"],
                text=True,
            )
            .stdout.strip()
            .split("=", 1)[1]
        )
        self.jwk = {
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": "bruno-fixture",
            "n": base64url(bytes.fromhex(modulus)),
            "e": "AQAB",
        }
        self.codes = {}
        self.lock = threading.Lock()
        self.calls = {
            "discovery": 0,
            "authorization": 0,
            "exchange": 0,
            "jwks": 0,
            "account_status": 0,
        }
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), self.handler())
        self.server.daemon_threads = True
        self.origin = f"http://127.0.0.1:{self.server.server_port}"
        self.issuer = self.origin + "/realms/bruno"
        self.thread = threading.Thread(
            target=self.server.serve_forever, kwargs={"poll_interval": 0.1}, daemon=True
        )

    def __enter__(self):
        try:
            self.thread.start()
        except BaseException:
            self.server.server_close()
            raise
        return self

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def count(self, name):
        with self.lock:
            self.calls[name] += 1

    def token(self, nonce):
        now = int(time.time())
        header = base64url(
            json.dumps(
                {"alg": "RS256", "kid": "bruno-fixture"}, separators=(",", ":")
            ).encode()
        )
        claims = base64url(
            json.dumps(
                {
                    "iss": self.issuer,
                    "aud": "aidash-bruno",
                    "sub": "bruno-user",
                    "nonce": nonce,
                    "iat": now,
                    "exp": now + 600,
                    "sid": "bruno-provider-session",
                },
                separators=(",", ":"),
            ).encode()
        )
        payload = f"{header}.{claims}".encode()
        signature = checked(
            ["openssl", "dgst", "-sha256", "-sign", str(self.key)], input=payload
        ).stdout
        return payload.decode() + "." + base64url(signature)

    def handler(self):
        provider = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def respond(self, status, value=None, location=None):
                body = b"" if value is None else json.dumps(value).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                if location:
                    self.send_header("Location", location)
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                parsed = urllib.parse.urlsplit(self.path)
                if parsed.path == "/realms/bruno/.well-known/openid-configuration":
                    provider.count("discovery")
                    self.respond(
                        200,
                        {
                            "issuer": provider.issuer,
                            "authorization_endpoint": provider.issuer + "/authorize",
                            "token_endpoint": provider.issuer
                            + "/protocol/openid-connect/token",
                            "jwks_uri": provider.issuer + "/certs",
                            "response_types_supported": ["code"],
                            "subject_types_supported": ["public"],
                            "id_token_signing_alg_values_supported": ["RS256"],
                            "token_endpoint_auth_methods_supported": [
                                "client_secret_basic",
                                "client_secret_post",
                            ],
                            "code_challenge_methods_supported": ["S256"],
                        },
                    )
                elif parsed.path == "/realms/bruno/certs":
                    provider.count("jwks")
                    self.respond(200, {"keys": [provider.jwk]})
                elif parsed.path == "/realms/bruno/authorize":
                    query = urllib.parse.parse_qs(parsed.query)
                    if query.get("client_id") != ["aidash-bruno"] or query.get(
                        "code_challenge_method"
                    ) != ["S256"]:
                        self.respond(400, {"error": "invalid_authorization_request"})
                        return
                    redirect = query.get("redirect_uri", [""])[0]
                    if (
                        not redirect.startswith("http://127.0.0.1:")
                        or urllib.parse.urlsplit(redirect).path != "/auth/callback"
                    ):
                        self.respond(400, {"error": "invalid_redirect"})
                        return
                    code = secrets.token_urlsafe(24)
                    with provider.lock:
                        provider.codes[code] = {
                            "challenge": query["code_challenge"][0],
                            "nonce": query["nonce"][0],
                            "redirect": redirect,
                        }
                    provider.count("authorization")
                    self.respond(
                        307,
                        location=redirect
                        + "?"
                        + urllib.parse.urlencode(
                            {"code": code, "state": query["state"][0]}
                        ),
                    )
                elif parsed.path == "/admin/realms/bruno/users/bruno-user":
                    provider.count("account_status")
                    self.respond(200, {"id": "bruno-user", "enabled": True})
                else:
                    self.respond(404, {"error": "not_found"})

            def do_POST(self):
                if self.path != "/realms/bruno/protocol/openid-connect/token":
                    self.respond(404, {"error": "not_found"})
                    return
                form = urllib.parse.parse_qs(
                    self.rfile.read(
                        int(self.headers.get("Content-Length", "0"))
                    ).decode()
                )
                if form.get("grant_type") == ["client_credentials"]:
                    self.respond(
                        200,
                        {
                            "access_token": "local-status-fixture",
                            "token_type": "Bearer",
                            "expires_in": 600,
                        },
                    )
                    return
                with provider.lock:
                    code = provider.codes.pop(form.get("code", [""])[0], None)
                verifier = form.get("code_verifier", [""])[0]
                if (
                    code is None
                    or form.get("grant_type") != ["authorization_code"]
                    or form.get("redirect_uri") != [code["redirect"]]
                    or base64url(hashlib.sha256(verifier.encode()).digest())
                    != code["challenge"]
                ):
                    self.respond(400, {"error": "invalid_grant"})
                    return
                provider.count("exchange")
                self.respond(
                    200,
                    {
                        "access_token": "local-login-fixture",
                        "token_type": "Bearer",
                        "expires_in": 600,
                        "id_token": provider.token(code["nonce"]),
                    },
                )

        return Handler


@contextlib.contextmanager
def database(args, name):
    """Create/drop only this invocation's unique fixture database."""
    if not re.fullmatch(r"aidash_bruno_[0-9a-f]{16}", name):
        raise ValueError("invalid fixture database name")
    prefix = (
        ["docker", "exec", "-i", args.postgres_container]
        if args.postgres_container
        else ["docker", "compose", "exec", "-T", "postgres"]
    )
    command = [
        *prefix,
        "psql",
        "-U",
        args.postgres_user,
        "-d",
        "postgres",
        "-v",
        "ON_ERROR_STOP=1",
    ]
    # The native history must own extension creation. Avoid inheriting extensions
    # or application objects from a developer's customized template1 database.
    checked(
        command, input=f'CREATE DATABASE "{name}" TEMPLATE template0;'.encode(), cwd=ROOT
    )
    try:
        yield
    finally:
        checked(
            command, input=f'DROP DATABASE "{name}" WITH (FORCE);'.encode(), cwd=ROOT
        )


@contextlib.contextmanager
def node(binary, environment, log):
    with log.open("wb") as output:
        child = subprocess.Popen(
            [str(binary), "server"],
            cwd=ROOT,
            env=environment,
            stdout=output,
            stderr=output,
        )
        try:
            yield child
        finally:
            if child.poll() is None:
                child.send_signal(signal.SIGTERM)
                try:
                    child.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=10)


def wait_for_health(child, base):
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        if child.poll() is not None:
            raise RuntimeError(
                f"Aidash exited during startup with code {child.returncode}"
            )
        try:
            with urllib.request.urlopen(base + "/health", timeout=2) as response:
                if response.status == 200:
                    return
        except OSError:
            pass
        time.sleep(0.2)
    raise RuntimeError("Aidash startup did not become healthy")


def summarize(report, expected):
    """Publish only names and pass/fail evidence; never headers, URLs or bodies."""
    iterations = json.loads(report.read_text())
    requests = []
    for iteration in iterations:
        for result in iteration.get("results", []):
            checks = []
            for key in [
                "testResults",
                "assertionResults",
                "preRequestTestResults",
                "postResponseTestResults",
            ]:
                for check in result.get(key) or []:
                    checks.append(
                        {
                            "name": check.get("description", check.get("name", key)),
                            "status": check.get("status"),
                        }
                    )
            passed = (
                not result.get("error")
                and not result.get("skipped")
                and bool(checks)
                and all(check["status"] == "pass" for check in checks)
            )
            requests.append(
                {
                    "name": result["name"],
                    "status": result.get("response", {}).get("status"),
                    "passed": passed,
                    "checks": checks,
                }
            )
            if result.get("error"):
                # Classify failures without retaining values from URLs, headers,
                # exception messages or request bodies in published evidence.
                message = str(result["error"]).lower()
                requests[-1]["error_categories"] = [
                    category
                    for category in (
                        "parse error",
                        "unexpected content-length",
                        "invalid header",
                        "socket hang up",
                        "connection refused",
                        "timeout",
                        "not a function",
                        "not defined",
                        "undefined",
                        "properties",
                        "json",
                        "redirect",
                    )
                    if category in message
                ]
    actual = [request["name"] for request in requests]
    complete = len(actual) == len(expected) and set(actual) == set(expected)
    return {
        "complete": complete,
        "passed": complete and all(request["passed"] for request in requests),
        "expected_requests": len(expected),
        "executed_requests": len(requests),
        "passed_requests": sum(request["passed"] for request in requests),
        "requests": requests,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--cli", default="bru")
    parser.add_argument(
        "--postgres-container",
        help="Existing fixture container; otherwise use the project's Compose postgres service",
    )
    parser.add_argument(
        "--postgres-port",
        type=int,
        default=int(os.environ.get("AIDASH_POSTGRES_PORT", "54370")),
    )
    parser.add_argument("--postgres-user", default="aidash")
    parser.add_argument(
        "--nats-url",
        default=os.environ.get(
            "NATS_URL",
            f"nats://127.0.0.1:{os.environ.get('AIDASH_NATS_PORT', '42270')}",
        ),
    )
    parser.add_argument(
        "--evidence-dir", type=pathlib.Path, default=ROOT / ".ignore/bruno"
    )
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    cli = shutil.which(args.cli)
    if cli is None:
        raise SystemExit("Bruno CLI is required; install @usebruno/cli@3.1.3")
    if not 1 <= args.postgres_port <= 65535:
        raise SystemExit("invalid PostgreSQL port")
    checked(["docker", "ps"], cwd=ROOT)
    run_id = uuid.uuid4().hex[:16]
    database_name = "aidash_bruno_" + run_id
    node_id = "aidash://bruno-" + run_id
    evidence = args.evidence_dir.resolve() / run_id
    evidence.mkdir(parents=True, mode=0o700)
    # Record the executable and actual source state independently of test outcomes.
    revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
    ).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    source = {
        "head": revision,
        "dirty": dirty,
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "cli_version": checked([cli, "--version"], text=True).stdout.strip(),
    }
    expected = [
        re.search(r"^  name: (.+)$", path.read_text(), re.MULTILINE).group(1)
        for path in sorted(COLLECTION.glob("[0-9]*.bru"))
    ]
    (evidence / "source.json").write_text(json.dumps(source, indent=2) + "\n")
    with tempfile.TemporaryDirectory(
        prefix="aidash-bruno-", dir="/tmp"
    ) as private_path:
        private = pathlib.Path(private_path)
        with contextlib.ExitStack() as resources:
            resources.enter_context(database(args, database_name))
            provider = resources.enter_context(IdentityProvider(private))
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            base = f"http://127.0.0.1:{port}"
            operator = secrets.token_urlsafe(32)
            settings = private / "settings"
            settings.mkdir()
            template = (
                (ROOT / "server/settings/base.example.toml")
                .read_text()
                .replace(
                    "allowed_hosts = []", 'allowed_hosts = ["127.0.0.1", "localhost"]'
                )
            )
            (settings / "base.toml").write_text(template)
            (settings / "local.toml").write_text("")
            web = private / "web"
            web.mkdir()
            environment = {
                key: value
                for key, value in os.environ.items()
                if not key.startswith(("AIDASH_", "REINHARDT_"))
            }
            environment.update(
                {
                    "REINHARDT_ENV": "local",
                    "REINHARDT_SETTINGS_DIR": str(settings),
                    "DATABASE_URL": f"postgres://{urllib.parse.quote(args.postgres_user, safe='')}:{urllib.parse.quote(os.environ.get('AIDASH_BRUNO_POSTGRES_PASSWORD', 'aidash-local'), safe='')}@127.0.0.1:{args.postgres_port}/{database_name}",
                    "NATS_URL": args.nats_url,
                    "AIDASH_NODE_ID": node_id,
                    "AIDASH_ENDPOINT": base,
                    "AIDASH_LISTEN": f"127.0.0.1:{port}",
                    "AIDASH_API_TOKEN": operator,
                    "AIDASH_API_RATE_BURST": "1000",
                    "AIDASH_WEB_DIR": str(web),
                    "AIDASH_OIDC_ISSUER": provider.issuer,
                    "AIDASH_OIDC_CLIENT_ID": "aidash-bruno",
                    "AIDASH_OIDC_CLIENT_SECRET": secrets.token_urlsafe(32),
                    "AIDASH_OIDC_PUBLIC_ORIGIN": base,
                    "AIDASH_OIDC_KEYCLOAK_ADMIN_URL": provider.origin
                    + "/admin/realms/bruno",
                    "AIDASH_OIDC_STATUS_CLIENT_ID": "aidash-status",
                    "AIDASH_OIDC_STATUS_CLIENT_SECRET": secrets.token_urlsafe(32),
                }
            )
            environment["NO_PROXY"] = environment["no_proxy"] = "127.0.0.1,localhost"
            child = resources.enter_context(
                node(binary, environment, private / "server.log")
            )
            wait_for_health(child, base)
            variables = {
                "base_url": base,
                "node_id": node_id,
                "tenant": "bruno-" + run_id,
                "issuer": provider.issuer,
                "operator_token": operator,
            }
            env_file = private / "environment.json"
            env_file.write_text(
                json.dumps(
                    {
                        "name": "Disposable local fixture",
                        "variables": [
                            {
                                "name": key,
                                "value": value,
                                "enabled": True,
                                "secret": key == "operator_token",
                            }
                            for key, value in variables.items()
                        ],
                    }
                )
            )
            env_file.chmod(0o600)
            report = private / "raw-report.json"
            with (private / "bruno.log").open("wb") as output:
                completed = subprocess.run(
                    [
                        cli,
                        "run",
                        "--env-file",
                        str(env_file),
                        "--disable-cookies",
                        "--sandbox",
                        "safe",
                        "--bail",
                        "--reporter-skip-all-headers",
                        "--reporter-json",
                        str(report),
                    ],
                    cwd=COLLECTION,
                    env={
                        **os.environ,
                        "NO_PROXY": "127.0.0.1,localhost",
                        "no_proxy": "127.0.0.1,localhost",
                    },
                    stdout=output,
                    stderr=output,
                    timeout=300,
                )
            result = (
                summarize(report, expected)
                if report.exists()
                else {
                    "complete": False,
                    "passed": False,
                    "expected_requests": len(expected),
                    "executed_requests": 0,
                    "passed_requests": 0,
                    "requests": [],
                }
            )
            result.update(
                {
                    "source": source,
                    "provider_calls": provider.calls.copy(),
                    "cli_exit_code": completed.returncode,
                    "server_exit_code": child.poll(),
                }
            )
            # Panic locations are useful failure evidence; surrounding log lines
            # may contain identity or database values and remain private.
            log = (private / "server.log").read_text(errors="replace")
            result["server_panic_locations"] = re.findall(
                r"panicked at ([^\s:]+:\d+:\d+)", log
            )
            result["passed"] = (
                result["passed"]
                and completed.returncode == 0
                and all(provider.calls[name] > 0 for name in provider.calls)
            )
            (evidence / "report.json").write_text(json.dumps(result, indent=2) + "\n")
            # Private raw reports contain credentials and are removed by the directory guard.
            print(
                f"Bruno real API: {result['passed_requests']}/{result['expected_requests']} requests passed; complete={result['complete']}; evidence={evidence}"
            )
            if not result["passed"]:
                failed = [
                    request["name"]
                    for request in result["requests"]
                    if not request["passed"]
                ]
                print("Failed request names: " + ", ".join(failed))
                raise SystemExit(1)


if __name__ == "__main__":
    main()
