#!/usr/bin/env python3
"""Exercise the two native entry points without Cloud, workers or NATS.

Requires Docker, OpenSSL, built service binaries and the built Auth Pages assets.
Only infrastructure DDL goes through psql; service commands own all migrations.
"""

import argparse
import base64
from contextlib import ExitStack
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


ROOT = Path(__file__).resolve().parents[1]


def run(*args, **kwargs):
    return subprocess.run(args, check=True, capture_output=True, text=True, **kwargs)


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, url):
        return None


HTTP = urllib.request.build_opener(NoRedirect)


def request(url):
    try:
        response = HTTP.open(url, timeout=3)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        return response.status, response.headers, response.read()


def ready(process, origin):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        assert process.poll() is None, "service exited before readiness; inspect its log"
        try:
            if request(origin + "/healthz")[0] == 200:
                return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.1)
    raise AssertionError("service readiness timed out")


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--auth-binary", type=Path, required=True)
    parser.add_argument("--marketplace-binary", type=Path, required=True)
    parser.add_argument("--auth-assets", type=Path, required=True)
    parser.add_argument("--marketplace-assets", type=Path, required=True)
    args = parser.parse_args()
    assert (args.auth_assets / "index.html").is_file(), "build Auth Pages first"
    assert (args.marketplace_assets / "index.html").is_file(), "build Marketplace Pages first"
    auth_binary = str(args.auth_binary.resolve())
    marketplace_binary = str(args.marketplace_binary.resolve())
    (ROOT / "private").mkdir(exist_ok=True)
    with ExitStack() as stack:
        directory = Path(stack.enter_context(tempfile.TemporaryDirectory(
            prefix="standalone-smoke-", dir=ROOT / "private")))
        password = secrets.token_hex(24)
        container = run("docker", "run", "--detach", "--rm", "-e",
                        "POSTGRES_PASSWORD=" + password, "-e", "POSTGRES_DB=aidash",
                        "-p", "127.0.0.1::5432", "postgres:17-alpine").stdout.strip()
        stack.callback(lambda: run("docker", "rm", "-f", container))
        for _ in range(100):
            if subprocess.run(["docker", "exec", container, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
                break
            time.sleep(0.1)
        else:
            raise AssertionError("PostgreSQL did not become ready")
        port = run("docker", "port", container, "5432/tcp").stdout.strip().rsplit(":", 1)[1]

        def ddl(sql):
            run("docker", "exec", "-i", container, "env", "PGPASSWORD=" + password, "psql", "-h", "127.0.0.1", "-v", "ON_ERROR_STOP=1",
                "-U", "postgres", "-d", "aidash", input=sql)

        for product in ("auth", "marketplace"):
            ddl((ROOT / "enterprise" / product / "bootstrap.sql").read_text())
        role_passwords = {
            role: secrets.token_hex(24)
            for role in ("aidash_auth_owner", "aidash_auth_app", "marketplace_owner", "marketplace_app")
        }
        for role, role_password in role_passwords.items():
            ddl(f"ALTER ROLE {role} LOGIN PASSWORD '{role_password}';")
        ddl("REVOKE ALL ON SCHEMA public FROM PUBLIC;")

        def database(role):
            return f"postgres://{role}:{role_passwords[role]}@127.0.0.1:{port}/aidash"

        def write(name, value):
            path = directory / name
            path.write_text(value)
            path.chmod(0o600)
            return str(path)

        auth_port, marketplace_port, grpc_port = free_port(), free_port(), free_port()
        issuer = f"http://127.0.0.1:{auth_port}"
        origin = f"http://127.0.0.1:{marketplace_port}"
        key, certificate = directory / "key.pem", directory / "certificate.pem"
        run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-keyout", str(key), "-out", str(certificate), "-subj", "/CN=authentication.internal")
        client_secret, resource_secret = secrets.token_urlsafe(40), secrets.token_urlsafe(40)
        config = {
            "issuer": issuer, "listen": f"127.0.0.1:{auth_port}", "schema": "aidash_auth",
            "database_url_file": write("owner.url", database("aidash_auth_owner")),
            "gcip_project": "standalone-fixture", "gcip_tenants": ["shared"],
            "gcip_browser_api_key": "fixture-public-key",
            "gcip_auth_domain": "standalone-fixture.firebaseapp.com",
            "signing_key_file": str(key), "signing_key_id": "smoke",
            "token_encryption_key_file": write("encryption", base64.b64encode(secrets.token_bytes(32)).decode()),
            "assets_dir": str(args.auth_assets.resolve()), "loopback_development": True,
        }
        body = "\n".join(f"{name} = {json.dumps(value)}" for name, value in config.items()
                         if not isinstance(value, bool)) + "\nloopback_development = true\n"
        body += "\n[[clients]]\nid = \"marketplace\"\n"
        body += "redirect_uri = " + json.dumps(origin + "/auth/callback") + "\n"
        body += "secret_file = " + json.dumps(write("client", client_secret)) + "\n"
        body += "introspection_secret_file = " + json.dumps(write("resource", resource_secret)) + "\n"
        body += f'\n[grpc]\nlisten = "127.0.0.1:{grpc_port}"\n'
        for name, path in (("certificate_file", certificate), ("private_key_file", key), ("client_ca_file", certificate)):
            body += f"{name} = {json.dumps(str(path))}\n"
        body += 'administrator_peers = ["spiffe://aidash/auth-admin"]\n'
        body += '[grpc.product_peers]\ncloud = "spiffe://aidash/cloud"\n'
        owner_config = write("auth-owner.toml", body)
        runtime_config = write("auth-runtime.toml", body.replace(config["database_url_file"],
                               write("runtime.url", database("aidash_auth_app"))))
        run(auth_binary, "--config", owner_config, "migrate")
        ddl((ROOT / "enterprise/auth/seal-ledger.sql").read_text())
        environment = {**os.environ, "AIDASH_MARKETPLACE_DATABASE_URL": database("marketplace_owner")}
        run(marketplace_binary, "migrate", env=environment)
        environment.update({"AIDASH_MARKETPLACE_DATABASE_URL": database("marketplace_app"),
                            "AIDASH_MARKETPLACE_LISTEN": f"127.0.0.1:{marketplace_port}",
                            "AIDASH_MARKETPLACE_ASSETS_DIR": str(args.marketplace_assets.resolve()),
                            "AIDASH_MARKETPLACE_ORIGIN": origin, "AIDASH_AUTH_ISSUER": issuer})
        for name, value in (("CLIENT", client_secret), ("RESOURCE", resource_secret),
                            ("BILLING", secrets.token_urlsafe(40)),
                            ("ENROLLMENT", secrets.token_urlsafe(40)), ("RECALL", secrets.token_urlsafe(40))):
            environment[f"AIDASH_MARKETPLACE_{name}_SECRET_FILE"] = write(name.lower(), value)

        def launch(binary, *arguments, env=None):
            log = stack.enter_context((directory / (Path(binary).name + ".log")).open("w+"))
            process = subprocess.Popen([binary, *arguments], env=env, stdout=log, stderr=log)
            stack.callback(stop, process)
            return process

        marketplace = launch(marketplace_binary, env=environment)
        ready(marketplace, origin)
        assert request(origin + "/api/marketplace/catalog")[0] == 200
        assert request(origin + "/auth/login")[0] == 503
        print("Marketplace starts and serves public metadata while Authentication is absent.")
        auth = launch(auth_binary, "--config", runtime_config, "serve")
        ready(auth, issuer)
        assert json.loads(request(issuer + "/.well-known/openid-configuration")[2])["issuer"] == issuer
        status, headers, _ = request(origin + "/auth/login")
        assert status in (302, 303)
        parameters = urllib.parse.parse_qs(urllib.parse.urlsplit(headers["Location"]).query)
        assert parameters["code_challenge_method"] == ["S256"]
        assert parameters["response_type"] == ["code"]
        assert "HttpOnly" in headers["Set-Cookie"]
        assert request(issuer + "/sign-in")[0] == 200
        stop(marketplace)
        assert request(issuer + "/healthz")[0] == 200
        print("Authentication serves OIDC/Pages independently; Marketplace BFF begins PKCE with an HttpOnly cookie.")
        print("Both service migrations and native processes passed without any Cloud schema/process.")


if __name__ == "__main__":
    main()
