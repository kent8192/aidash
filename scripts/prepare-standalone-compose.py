#!/usr/bin/env python3
"""Prepare only local standalone Compose credentials, never Cloud configuration.

Existing passwords/keys are retained. Generated files live in a private 0700
directory, with readable file modes for Compose's per-file non-root secret
mounts. No credential value is printed or placed in the rendered Compose model.
"""

import argparse
import base64
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]


def prepare(directory, auth_port, marketplace_port, gcip):
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    directory.chmod(0o700)

    def write(name, value, *, retain=False):
        path = directory / name
        if not retain or not path.exists():
            with tempfile.NamedTemporaryFile("w", dir=directory, delete=False) as output:
                output.write(value + "\n")
                temporary = Path(output.name)
            temporary.chmod(0o444)
            temporary.replace(path)
        path.chmod(0o444)
        return path.read_text().strip()

    def openssl(*args):
        subprocess.run(["openssl", *args], cwd=directory, check=True, capture_output=True)

    write("postgres-password", secrets.token_hex(24), retain=True)
    for prefix, role in (("auth-owner", "aidash_auth_owner"), ("auth-app", "aidash_auth_app"),
                         ("marketplace-owner", "marketplace_owner"), ("marketplace-app", "marketplace_app")):
        password = write(prefix + "-password", secrets.token_hex(24), retain=True)
        write(prefix + "-url", f"postgres://{role}:{password}@postgres:5432/standalone")
    for name in ("client", "resource", "billing", "enrollment", "recall"):
        write(f"marketplace-{name}-secret", secrets.token_urlsafe(40), retain=True)
    write("auth-token-encryption-key", base64.b64encode(secrets.token_bytes(32)).decode(), retain=True)
    if not (directory / "auth-signing-key.pem").exists():
        openssl("genrsa", "-out", "auth-signing-key.pem", "2048")
    if not (directory / "auth-client-ca.pem").exists():
        openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "30",
                "-keyout", "development-ca.key", "-out", "auth-client-ca.pem",
                "-subj", "/CN=Aidash standalone development CA")
    for name, san, usage in (("auth-server", "DNS:authentication,DNS:localhost", "serverAuth"),
                             ("cloud-client", "URI:spiffe://aidash/cloud", "clientAuth")):
        if not (directory / (name + ".crt")).exists():
            openssl("req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", name + ".key",
                    "-out", name + ".csr", "-subj", "/CN=" + name)
            extensions = write(name + ".ext", f"subjectAltName={san}\nextendedKeyUsage={usage}")
            assert extensions
            openssl("x509", "-req", "-in", name + ".csr", "-CA", "auth-client-ca.pem",
                    "-CAkey", "development-ca.key", "-CAcreateserial", "-out", name + ".crt",
                    "-days", "30", "-extfile", name + ".ext")
        (directory / (name + ".key")).chmod(0o444)
        (directory / (name + ".crt")).chmod(0o444)
    (directory / "auth-signing-key.pem").chmod(0o444)
    (directory / "auth-client-ca.pem").chmod(0o444)
    (directory / "development-ca.key").chmod(0o600)
    config = {
        "issuer": f"http://127.0.0.1:{auth_port}", "listen": "0.0.0.0:8081", "schema": "aidash_auth",
        "database_url_file": "/run/secrets/auth-database-url",
        "gcip_project": gcip["project"], "gcip_tenants": [gcip["tenant"]],
        "gcip_browser_api_key": gcip["api_key"], "gcip_auth_domain": gcip["auth_domain"],
        "signing_key_file": "/run/secrets/auth-signing-key.pem", "signing_key_id": "local-1",
        "token_encryption_key_file": "/run/secrets/auth-token-encryption-key", "assets_dir": "/app/auth-web",
    }
    body = "\n".join(f"{key} = {json.dumps(value)}" for key, value in config.items())
    body += '\nloopback_development = true\n\n[gcip_providers]\n'
    body += f'{json.dumps(gcip["tenant"])} = ["google.com", "password"]\n'
    body += '\n[[clients]]\nid = "marketplace"\n'
    body += f'redirect_uri = "http://127.0.0.1:{marketplace_port}/auth/callback"\n'
    body += 'secret_file = "/run/secrets/marketplace-client-secret"\n'
    body += 'introspection_secret_file = "/run/secrets/marketplace-resource-secret"\n'
    body += '\n[grpc]\nlisten = "0.0.0.0:9091"\n'
    body += 'certificate_file = "/run/secrets/auth-server.crt"\n'
    body += 'private_key_file = "/run/secrets/auth-server.key"\n'
    body += 'client_ca_file = "/run/secrets/auth-client-ca.pem"\n'
    body += 'administrator_peers = ["spiffe://aidash/authentication-administration"]\n'
    body += 'source_restore_clients = ["cloud"]\n'
    body += '\n[grpc.product_peers]\ncloud = "spiffe://aidash/cloud"\n'
    for mode in ("owner", "runtime"):
        write(f"auth-{mode}.toml", body)


def port(value):
    value = int(value)
    if not 1024 <= value <= 65535:
        raise argparse.ArgumentTypeError("use an unprivileged TCP port")
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=ROOT / "private/standalone-compose")
    parser.add_argument("--auth-port", type=port, default=os.environ.get("AIDASH_AUTH_PORT", "18081"))
    parser.add_argument("--marketplace-port", type=port, default=os.environ.get("AIDASH_MARKETPLACE_PORT", "18082"))
    parser.add_argument("--gcip-project")
    parser.add_argument("--gcip-tenant")
    parser.add_argument("--gcip-api-key")
    parser.add_argument("--gcip-auth-domain")
    args = parser.parse_args()
    if args.auth_port == args.marketplace_port:
        parser.error("Auth and Marketplace require separate public origins")
    config_file = args.directory / "auth-runtime.toml"
    existing = tomllib.loads(config_file.read_text()) if config_file.exists() else {}
    # Re-running cargo make must not replace an explicitly configured provider
    # with startup-only placeholders. Override it only through explicit flags.
    gcip = {"project": args.gcip_project or existing.get("gcip_project", "standalone-fixture"),
            "tenant": args.gcip_tenant or existing.get("gcip_tenants", ["shared"])[0],
            "api_key": args.gcip_api_key or existing.get("gcip_browser_api_key", "replace-with-public-firebase-api-key"),
            "auth_domain": args.gcip_auth_domain or existing.get("gcip_auth_domain", "standalone-fixture.firebaseapp.com")}
    prepare(args.directory.resolve(), args.auth_port, args.marketplace_port, gcip)
    print("Standalone Compose files prepared; existing credentials retained.")
    if gcip["project"] == "standalone-fixture":
        print("GCIP uses placeholder configuration: live sign-in is unverified and requires a configured project.")


if __name__ == "__main__":
    main()
