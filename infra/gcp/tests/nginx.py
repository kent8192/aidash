#!/usr/bin/env python3
"""Verify admission and in-flight accounting in disposable Ubuntu/Nginx."""

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading
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
            self.wfile.write(b"{}")

        def do_POST(self):
            self.rfile.read(int(self.headers.get("Content-Length", 0)))
            if self.path == "/api/tasks":
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

    def call(path, data=None, port=8088):
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}{path}", data=data, timeout=10
        ) as response:
            return json.load(response)

    def activity():
        return call("/activity", port=8089)

    try:
        assert activity() == {"inflight": 0, "last_active": 0}
        call("/health")
        call("/api/runs/example/shell/poll", b"{}")
        call("/api/runs/example/python/poll", b"{}")
        assert activity() == {"inflight": 0, "last_active": 0}, "polling must stay idle"
        failures = []

        def submit():
            try:
                call("/api/tasks", b"{}")
            except Exception as error:
                failures.append(error)

        thread = threading.Thread(target=submit)
        thread.start()
        assert entered.wait(5)
        assert activity()["inflight"] == 1, "an uncommitted request must block stopping"
        call("/admission/close", b"", port=8089)
        try:
            call("/api/tasks", b"{}")
            raise AssertionError("closed admission accepted a new request")
        except urllib.error.HTTPError as error:
            assert error.code == 503
        assert activity()["inflight"] == 1
        release.set()
        thread.join(10)
        assert not thread.is_alive() and not failures
        observed = activity()
        assert observed["inflight"] == 0 and observed["last_active"] > 0
        call("/admission/open", b"", port=8089)
        call("/api/runs/example/python/poll", b"{}")
        assert activity() == observed
        print(
            "Nginx: polling excluded; uncommitted requests counted; admission closed and reopened"
        )
    finally:
        print(Path("/var/log/nginx/error.log").read_text()[-2000:])
        release.set()
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
                "apt-get update -qq && apt-get install -y -qq --no-install-recommends nginx libnginx-mod-http-lua lua-cjson python3 >/dev/null && python3 /checks/tests/nginx.py",
            ],
            check=True,
            timeout=600,
        )
