#!/usr/bin/env python3
"""Exercise the shipped container/Compose lifecycle, with disposable credentials.

Builds are optional; all four product images must exist otherwise. The dedicated
project and its volume are removed even on failure. No Cloud process is started.
"""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import uuid


ROOT = Path(__file__).resolve().parents[1]


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        return None


HTTP = urllib.request.build_opener(NoRedirect)


def request(url, expected):
    try:
        response = HTTP.open(url, timeout=20)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        assert response.status == expected, f"{url}: expected {expected}, got {response.status}"
        return response.headers, response.read()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", action="store_true", help="build all four images, with dev-profile native binaries")
    args = parser.parse_args()
    (ROOT / "private").mkdir(exist_ok=True)
    with ExitStack() as cleanup:
        directory = Path(cleanup.enter_context(tempfile.TemporaryDirectory(prefix="compose-acceptance-", dir=ROOT / "private")))
        ports = set()
        while len(ports) < 3:
            ports.add(free_port())
        auth_port, marketplace_port, grpc_port = sorted(ports)
        environment = dict(os.environ, AIDASH_STANDALONE_PROJECT="aidash-standalone-smoke-" + uuid.uuid4().hex[:10],
                           AIDASH_STANDALONE_DIRECTORY=str(directory), AIDASH_AUTH_PORT=str(auth_port),
                           AIDASH_MARKETPLACE_PORT=str(marketplace_port), AIDASH_AUTH_GRPC_PORT=str(grpc_port))
        compose_command = ["docker", "compose", "--env-file", "/dev/null", "-f", str(ROOT / "enterprise/compose.yaml")]

        def run(command, *, timeout=300):
            result = subprocess.run(command, cwd=ROOT, env=environment, capture_output=True, text=True, timeout=timeout)
            if result.returncode:
                raise RuntimeError(f"Command failed: {' '.join(command)}\n{result.stderr[-4000:]}")
            return result.stdout

        def compose(*arguments, profiles=("auth", "marketplace"), timeout=300):
            profile_arguments = [part for profile in profiles for part in ("--profile", profile)]
            return run([*compose_command, *profile_arguments, *arguments], timeout=timeout)

        run(["python3", "scripts/prepare-standalone-compose.py", "--directory", str(directory)])
        cleanup.callback(lambda: compose("down", "-v", "--remove-orphans"))
        if args.build:
            print("Building all four standalone product images", flush=True)
            compose("build", "--build-arg", "CARGO_PROFILE=dev", "authentication", "marketplace", "marketplace-web", timeout=3600)
            run(["docker", "build", "-t", "aidash-auth-web:local", "enterprise/auth/frontend"], timeout=1800)
            print("PASS: four independent product images built", flush=True)
        compose("up", "--no-build", "-d", "--wait", "--wait-timeout", "180", profiles=("marketplace",))
        origin = f"http://127.0.0.1:{marketplace_port}"
        issuer = f"http://127.0.0.1:{auth_port}"
        assert b".js" in request(origin + "/", 200)[1]
        json.loads(request(origin + "/api/marketplace/catalog", 200)[1])
        request(origin + "/auth/login", 503)
        print("PASS: Marketplace container/Pages start with Authentication absent; login fails closed", flush=True)
        compose("up", "--no-build", "-d", "--wait", "--wait-timeout", "180", profiles=("auth",))
        discovery = json.loads(request(issuer + "/.well-known/openid-configuration", 200)[1])
        assert discovery["issuer"] == issuer
        assert b".js" in request(issuer + "/sign-in", 200)[1]
        headers, _ = request(origin + "/auth/login", 302)
        redirect = urllib.parse.urlparse(headers["location"])
        query = urllib.parse.parse_qs(redirect.query)
        assert redirect.geturl().startswith(issuer + "/oauth/authorize?")
        assert query["code_challenge_method"] == ["S256"]
        assert query["redirect_uri"] == [origin + "/auth/callback"]
        assert "HttpOnly" in headers["set-cookie"]
        assert not any(key in query for key in ("access_token", "refresh_token", "id_token"))
        print("PASS: real frontend proxy/BFF/loopback transport returns exact PKCE redirect and HttpOnly cookie", flush=True)
        frontend = run(["docker", "run", "--rm", "-d", "--read-only", "--user", "10001:10001",
                        "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true", "--tmpfs", "/tmp",
                        "--network", environment["AIDASH_STANDALONE_PROJECT"] + "_default",
                        "-p", "127.0.0.1::8080", "-e", "AUTHENTICATION_BACKEND_URL=http://authentication:8081",
                        "aidash-auth-web:local"]).strip()
        cleanup.callback(lambda: run(["docker", "rm", "-f", frontend]))
        frontend_port = run(["docker", "port", frontend, "8080/tcp"]).strip().rsplit(":", 1)[1]
        # Docker exec reaches the real proxy only after its entrypoint has rendered config.
        run(["docker", "exec", frontend, "sh", "-ec",
             "for attempt in 1 2 3 4 5 6 7 8 9 10; do wget -q -O /dev/null http://127.0.0.1:8080/sign-in && exit 0; sleep 1; done; exit 1"])
        assert b".js" in request(f"http://127.0.0.1:{frontend_port}/sign-in", 200)[1]
        proxied = json.loads(request(f"http://127.0.0.1:{frontend_port}/.well-known/openid-configuration", 200)[1])
        assert proxied["issuer"] == issuer
        print("PASS: separate Auth frontend image serves Pages/proxies OIDC as non-root with read-only root", flush=True)
        compose("stop", "marketplace-web", "authentication-loopback", "marketplace")
        compose("restart", "authentication")
        compose("up", "--no-build", "-d", "--wait", "--wait-timeout", "60", profiles=("auth",))
        request(issuer + "/healthz", 200)
        print("PASS: Authentication restarts without Marketplace, Cloud, worker or NATS", flush=True)


if __name__ == "__main__":
    main()
