#!/usr/bin/env python3
"""Verify admission and in-flight accounting in disposable Ubuntu/Nginx."""

from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading
import time
import urllib.error
import urllib.request


def inside():
    entered, release = threading.Event(), threading.Event()

    class Upstream(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            self.send_response(200)
            self.end_headers()
            self.wfile.write(
                json.dumps({"client_ip": self.headers.get("X-Real-IP")}).encode()
            )

        def do_POST(self):
            self.rfile.read(int(self.headers.get("Content-Length", 0)))
            if self.path in {
                "/api/tasks",
                "/federation/v0.1/scoped/files/commit",
                "/federation/v0.1/transactions/prepare",
            }:
                entered.set()
                if not release.wait(10):
                    raise RuntimeError("fixture request was not released")
            self.do_GET()

    server = ThreadingHTTPServer(("127.0.0.1", 18080), Upstream)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    shutil.copy("/checks/runtime/nginx.conf", "/etc/nginx/conf.d/aidash.conf")
    Path("/etc/nginx/sites-enabled/default").unlink(missing_ok=True)
    Path("/run/aidash").mkdir(mode=0o755, exist_ok=True)
    Path("/run/aidash/serving").touch(mode=0o644)
    subprocess.run(["nginx", "-t"], check=True)
    subprocess.run(["nginx"], check=True)
    Path("/tmp/Caddyfile").write_text(
        "http://127.0.0.1:8090 {\n reverse_proxy 127.0.0.1:8088\n}\n"
    )
    caddy = subprocess.Popen(
        ["caddy", "run", "--config", "/tmp/Caddyfile", "--adapter", "caddyfile"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )

    def call(path, data=None, port=8088):
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}{path}", data=data, timeout=10
        ) as response:
            return json.load(response)

    def activity():
        return call("/activity", port=8089)

    def wait_inflight(expected):
        # A response can reach the client before Nginx's log/connection cleanup.
        # Wait for that phase while requiring any held upstream to stay counted.
        for _ in range(50):
            observed = activity()
            assert observed["inflight"] >= expected, observed
            if observed["inflight"] == expected:
                return observed
            time.sleep(0.1)
        raise AssertionError(("in-flight cleanup did not finish", observed))

    try:
        for _ in range(50):
            try:
                call("/health", port=8090)
                break
            except urllib.error.URLError:
                time.sleep(0.1)
        else:
            raise AssertionError("Caddy fixture did not start")
        for address in ("127.0.0.2", "127.0.0.3"):
            client = HTTPConnection(
                "127.0.0.1", 8090, timeout=10, source_address=(address, 0)
            )
            try:
                client.request(
                    "GET",
                    "/auth/config",
                    headers={
                        "X-Forwarded-For": "198.51.100.1",
                        "X-Real-IP": "198.51.100.2",
                        "Forwarded": "for=198.51.100.3",
                    },
                )
                response = client.getresponse()
                assert response.status == 200
                identity = json.load(response)
                assert identity["client_ip"] == address, identity
            finally:
                client.close()
        assert activity() == {"inflight": 0, "last_active": 0}
        call("/health")
        call("/api/runs/example/shell/poll", b"{}")
        call("/api/runs/example/python/poll", b"{}")
        call("/federation/v0.1/observe")
        for path in [
            "/federation/v0.1/discover",
            "/federation/v0.1/workspace",
            "/federation/v0.1/scoped/files/status",
            "/federation/v0.1/scoped/execution/status",
            "/federation/v0.1/scoped/execution/admissions/id/verify",
        ]:
            call(path, b"{}")
        assert activity() == {"inflight": 0, "last_active": 0}, "polling must stay idle"
        for path in [
            "/api/tasks",
            "/federation/v0.1/scoped/files/commit",
            "/federation/v0.1/transactions/prepare",
        ]:
            entered.clear()
            release.clear()
            failures = []

            def submit(path=path, failures=failures):
                try:
                    call(path, b"{}")
                except Exception as error:
                    failures.append(error)

            thread = threading.Thread(target=submit)
            thread.start()
            assert entered.wait(5)
            assert activity()["inflight"] == 1, (
                "an uncommitted request must block stopping"
            )
            call("/admission/close", b"", port=8089)
            try:
                call(path, b"{}")
                raise AssertionError("closed admission accepted a new request")
            except urllib.error.HTTPError as error:
                assert error.code == 503
                error.read()
                error.close()
            wait_inflight(1)
            release.set()
            thread.join(10)
            assert not thread.is_alive() and not failures
            observed = wait_inflight(0)
            assert observed["inflight"] == 0 and observed["last_active"] > 0
            call("/admission/open", b"", port=8089)
            call("/api/runs/example/python/poll", b"{}")
            assert activity() == observed
        # Access-phase accounting must begin before a slow body is buffered.
        client = HTTPConnection("127.0.0.1", 8088, timeout=10)
        try:
            client.putrequest("POST", "/federation/v0.1/scoped/files/chunk")
            client.putheader("Content-Length", "2")
            client.endheaders()
            client.send(b"{")
            for _ in range(50):
                if activity()["inflight"] == 1:
                    break
                time.sleep(0.1)
            else:
                raise AssertionError("slow federation body was not counted")
            call("/admission/close", b"", port=8089)
            assert activity()["inflight"] == 1
            client.send(b"}")
            response = client.getresponse()
            assert response.status == 200
            response.read()
            wait_inflight(0)
            call("/admission/open", b"", port=8089)
        finally:
            client.close()
        # Session polling stays idle; the dashboard's interaction heartbeat renews.
        # The log phase decrements in-flight before it records last_active, so let
        # the preceding request's log phase finish before taking the baseline.
        wait_inflight(0)
        time.sleep(0.2)
        before = activity()["last_active"]
        call("/auth/session")
        time.sleep(0.2)
        assert activity()["last_active"] == before
        call("/auth/activity", b"")
        for _ in range(50):
            if activity()["last_active"] > before:
                break
            time.sleep(0.1)
        else:
            raise AssertionError("dashboard heartbeat did not renew activity")
        print(
            "Caddy/Nginx: client IPs preserved and spoofed headers replaced; polling excluded; API/federation writes, heartbeats and slow bodies counted; admission closed and reopened"
        )
    finally:
        print(Path("/var/log/nginx/error.log").read_text()[-2000:])
        release.set()
        caddy.terminate()
        caddy.wait(timeout=10)
        server.shutdown()
        subprocess.run(["nginx", "-s", "quit"], check=False)


if __name__ == "__main__":
    if os.environ.get("AIDASH_NGINX_FIXTURE") == "1":
        inside()
    else:
        root = Path(__file__).resolve().parents[1]
        subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "--label",
                "purpose=aidash-infra-check",
                "-e",
                "AIDASH_NGINX_FIXTURE=1",
                "-e",
                "DEBIAN_FRONTEND=noninteractive",
                "-v",
                f"{root}:/checks:ro",
                "ubuntu:24.04",
                "bash",
                "-euc",
                "apt-get update -qq && apt-get install -y -qq --no-install-recommends nginx libnginx-mod-http-lua lua-cjson caddy python3 >/dev/null && python3 /checks/tests/nginx.py",
            ],
            check=True,
            timeout=600,
        )
