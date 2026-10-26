#!/usr/bin/env python3
"""Issue/renew only the preview wildcard; keep PEM and DNS tokens out of logs/state.

The protected certificate-renewal workflow owns this operation. Terraform owns
Secret metadata and a secret-scoped read/add identity, never certificate versions.
Certbot's official DNS plugin automates and cleans up its DNS-01 TXT challenge:
https://certbot-dns-cloudflare.readthedocs.io/en/stable/
The reviewed v5.8.0 OCI index below contains all dependencies, with no pip/latest.
"""

import argparse
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import re
import sys
from tempfile import TemporaryDirectory
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "gcp" / "control"))
from cloud import github, run
from policy import Refused

DOMAIN = "*.preview.aidash.run"
CERTBOT = "certbot/dns-cloudflare:v5.8.0@sha256:c45edb002b883da1a1235abb205dff474a7a1a459d878e8d5fdc7f9d83073aea"
KEYS = {"certificate_pem", "private_key_pem"}
ACCOUNT_PATH = re.compile(r"accounts/acme-v02\.api\.letsencrypt\.org/directory/[a-f0-9]{32}/(private_key|regr|meta)\.json")


def configuration(value):
    if not isinstance(value, dict) or (
        not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", value.get("project_id", ""))
        or value.get("deployment_tier") != "nonproduction"
        or value.get("domain") != "aidash.run"
        or not re.fullmatch(r"[A-Za-z0-9_-]{1,255}", value.get("preview_tls_secret", ""))
        or not re.fullmatch(r"[A-Za-z0-9_-]{1,255}", value.get("acme_state_secret", "aidash-preview-acme-state"))
        or value.get("preview_tls_secret") == value.get("acme_state_secret", "aidash-preview-acme-state")
    ):
        raise Refused("Preview TLS renewal requires the explicit nonproduction project and preview TLS secret")
    return value


def private_file(path, data):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as output:
        output.write(data)


def inspect_bundle(bundle):
    """Return the leaf validity window; expired matching certificates can renew."""
    if not isinstance(bundle, dict) or set(bundle) != KEYS or any(
        not isinstance(value, str) or not value or len(value.encode()) > 1024 * 1024 or "\x00" in value
        for value in bundle.values()
    ):
        raise Refused("The preview TLS secret must contain exactly certificate_pem and private_key_pem")
    with TemporaryDirectory(prefix="aidash-tls-validation-") as directory:
        cert, key = Path(directory) / "certificate.pem", Path(directory) / "private-key.pem"
        private_file(cert, bundle["certificate_pem"])
        private_file(key, bundle["private_key_pem"])
        try:
            alternatives = run("openssl", "x509", "-in", cert, "-noout", "-ext", "subjectAltName", timeout=30).decode()
            if re.findall(r"DNS:([^,\s]+)", alternatives) != [DOMAIN]:
                raise Refused("Refusing a certificate for any domain other than *.preview.aidash.run")
            public = run("openssl", "x509", "-in", cert, "-noout", "-pubkey", timeout=30)
            if public.strip() != run("openssl", "pkey", "-in", key, "-pubout", timeout=30).strip():
                raise Refused("Preview TLS certificate and private key do not match")
            dates = dict(line.split("=", 1) for line in run("openssl", "x509", "-in", cert, "-noout", "-dates", timeout=30).decode().splitlines())
            return tuple(datetime.strptime(dates[field], "%b %d %H:%M:%S %Y %Z").replace(tzinfo=timezone.utc)
                         for field in ("notBefore", "notAfter"))
        except Refused:
            raise
        except (RuntimeError, ValueError, KeyError) as error:
            raise Refused("Invalid preview TLS certificate or private key; private command output suppressed") from error


def current_bundle(config, secret_id=None):
    """Read newest enabled payload and whether the latest alias can access it."""
    secret_id = secret_id or config["preview_tls_secret"]
    # `versions list` takes the secret as its required positional SECRET argument.
    versions = json.loads(run("gcloud", "secrets", "versions", "list", secret_id,
                              "--project", config["project_id"], "--format=json(name,state)", timeout=60))
    if not isinstance(versions, list) or any(not re.fullmatch(r"projects/[^/]+/secrets/" + re.escape(secret_id) + r"/versions/[1-9][0-9]*", version.get("name", ""))
                                             for version in versions):
        raise Refused("Secret Manager returned an invalid preview certificate version inventory")
    enabled = [version for version in versions if version.get("state") == "ENABLED"]
    if not enabled:
        return None, False
    newest = max(enabled, key=lambda version: int(version["name"].rsplit("/", 1)[1]))
    number = newest["name"].rsplit("/", 1)[1]
    bundle = json.loads(run("gcloud", "secrets", "versions", "access", number, "--secret", secret_id,
                            "--project", config["project_id"], timeout=60))
    latest = max(versions, key=lambda version: int(version["name"].rsplit("/", 1)[1]))
    return bundle, latest == newest


def account_files(state):
    if state is None:
        return {}
    if not isinstance(state, dict) or set(state) != {"schema", "files"} or state["schema"] != "aidash-acme-account/1" or not isinstance(state["files"], dict):
        raise Refused("Invalid managed ACME account state")
    files = state["files"]
    if len(files) != 3 or len({str(Path(name).parent) for name in files}) != 1 or any(
        not ACCOUNT_PATH.fullmatch(name) or not isinstance(value, str) or "\x00" in value for name, value in files.items()
    ) or len(json.dumps(state).encode()) > 65536:
        raise Refused("ACME account state contains unsupported or excessive private files")
    for name, value in files.items():
        parsed = json.loads(value)
        if not isinstance(parsed, dict):
            raise Refused("Invalid ACME account JSON")
        if name.endswith("/regr.json") and not re.fullmatch(r"https://acme-v02\.api\.letsencrypt\.org/acme/acct/[1-9][0-9]*", parsed.get("uri", "")):
            raise Refused("ACME account registration must use the fixed Let's Encrypt HTTPS server")
        if name.endswith("/private_key.json") and (parsed.get("kty") not in {"RSA", "EC"} or not isinstance(parsed.get("d"), str) or not parsed["d"]):
            raise Refused("Invalid private ACME account key")
    return files


def issue_certificate(email, token, account_state, persist_account):
    if not re.fullmatch(r"[^\s@<>]+@[^\s@<>]+\.[^\s@<>]+", email) or any(c in email for c in "\r\n\x00"):
        raise Refused("Set one monitored ACME contact email")
    if not re.fullmatch(r"[A-Za-z0-9_-]{20,256}", token):
        raise Refused("Configure the certificate-renewal Environment's scoped Cloudflare DNS token")
    with TemporaryDirectory(prefix="aidash-certbot-") as directory:
        root = Path(directory).resolve()
        private_file(root / "cloudflare.ini", "dns_cloudflare_api_token = " + token + "\n")
        for name, value in account_files(account_state).items():
            path = root / "config" / name
            path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
            private_file(path, value)
        # The container receives only this private directory. No GCP/GitHub
        # credentials, source checkout, host Docker socket or runtime secrets.
        # The unprivileged UID also permits complete cleanup on Linux runners.
        try:
            run("docker", "run", "--rm", "--cidfile", str(root / "container.cid"), "--user", f"{os.getuid()}:{os.getgid()}", "--cap-drop", "ALL",
                "--security-opt", "no-new-privileges", "--read-only", "--tmpfs", "/tmp:rw,noexec,nosuid,size=64m",
                "--mount", f"type=bind,source={root},target=/tls", CERTBOT,
                "certonly", "--non-interactive", "--agree-tos", "--email", email,
                "--server", "https://acme-v02.api.letsencrypt.org/directory",
                "--dns-cloudflare", "--dns-cloudflare-credentials", "/tls/cloudflare.ini",
                "--dns-cloudflare-propagation-seconds", "60", "--preferred-challenges", "dns",
                "--config-dir", "/tls/config", "--work-dir", "/tls/work", "--logs-dir", "/tls/logs",
                "--cert-name", "aidash-preview", "--key-type", "ecdsa", "--elliptic-curve", "secp256r1",
                "--domain", DOMAIN, timeout=900)
        finally:
            # Killing a timed-out Docker CLI does not always stop its container.
            # Stop the exact CID before deleting private files or reading state.
            cid = root / "container.cid"
            if cid.exists():
                container_id = cid.read_text().strip()
                if re.fullmatch(r"[a-f0-9]{64}", container_id):
                    try:
                        run("docker", "rm", "--force", container_id, timeout=30)
                    except RuntimeError:
                        pass  # --rm already removed a completed container.
            # DNS challenges can fail after registering an ACME account. Persist
            # it even on failure, before cleaning up, so retries reuse that key.
            accounts = root / "config" / "accounts"
            if accounts.exists():
                files = {}
                for path in accounts.rglob("*"):
                    if path.is_file():
                        if path.is_symlink() or not path.resolve().is_relative_to(root):
                            raise Refused("ACME account output escaped its private directory")
                        files[str(path.relative_to(root / "config"))] = path.read_text()
                if files:
                    state = {"schema": "aidash-acme-account/1", "files": files}
                    account_files(state)
                    persist_account(state)
        live = root / "config" / "live" / "aidash-preview"
        files = [live / "fullchain.pem", live / "privkey.pem"]
        if any(not path.resolve().is_relative_to(root) for path in files):
            raise Refused("Certificate output escaped its private directory")
        return dict(zip(("certificate_pem", "private_key_pem"), (path.read_text() for path in files)))


def renew(config, email, token, renewal_days=30):
    config = configuration(config)
    if type(renewal_days) is not int or not 7 <= renewal_days <= 60:
        raise Refused("Renewal window must be between 7 and 60 days")
    now = datetime.now(timezone.utc)
    before, active = current_bundle(config)
    if before is not None:
        start, end = inspect_bundle(before)
        if active and start <= now and end > now + timedelta(days=renewal_days):
            return "current"
    state_secret = config.get("acme_state_secret", "aidash-preview-acme-state")
    account_state, _ = current_bundle(config, state_secret)
    account_files(account_state)

    def persist_account(state):
        if state != account_state:
            run("gcloud", "secrets", "versions", "add", state_secret, "--project", config["project_id"],
                "--data-file=-", "--quiet", data=json.dumps(state).encode(), timeout=60)

    issued = issue_certificate(email, token, account_state, persist_account)
    now = datetime.now(timezone.utc)
    start, end = inspect_bundle(issued)
    if not start <= now < end or end <= now + timedelta(days=renewal_days):
        raise Refused("Issued preview certificate is invalid or already inside its renewal window")
    # A second caller may finish while ACME is in flight. Prefer its valid
    # version instead of creating another one; workflow concurrency serializes
    # normal scheduled/manual runs without canceling certificate issuance.
    current, active = current_bundle(config)
    if current is not None and current != before and active:
        start, end = inspect_bundle(current)
        if start <= now and end > now + timedelta(days=renewal_days):
            return "superseded"
    run("gcloud", "secrets", "versions", "add", config["preview_tls_secret"], "--project", config["project_id"],
        "--data-file=-", "--quiet", data=json.dumps(issued).encode(), timeout=60)
    return "renewed"


def preflight(config):
    config = configuration(config)
    repository = config.get("repository")
    if repository != "kent8192/aidash" or os.environ.get("GITHUB_REPOSITORY") != repository:
        raise Refused("Certificate renewal is restricted to the Aidash repository")
    repo = github("repos/" + repository)
    branch = config.get("automation_branch")
    if not branch or repo["default_branch"] != branch or os.environ.get("GITHUB_REF") != "refs/heads/" + branch:
        raise Refused("Certificate renewal must execute from the configured protected default automation branch")
    actual = github("repos/" + repository + "/branches/" + quote(branch, safe=""))
    sha = actual.get("commit", {}).get("sha", "")
    if not actual.get("protected") or not re.fullmatch(r"[a-f0-9]{40}", sha):
        raise Refused("Certificate workflow and trusted code anchor must be the protected automation branch")
    return sha


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["preflight", "renew"])
    args = parser.parse_args()
    try:
        config = json.loads(os.environ.get("AIDASH_GCP_CONFIG") or "{}")
        if args.action == "preflight":
            sha = preflight(config)
            with open(os.environ["GITHUB_OUTPUT"], "a") as output:
                output.write("sha=" + sha + "\n")
        else:
            result = renew(config, os.environ.get("AIDASH_ACME_EMAIL", ""), os.environ.get("CLOUDFLARE_CERTIFICATE_DNS_TOKEN", ""),
                           int(os.environ.get("AIDASH_TLS_RENEWAL_DAYS") or "30"))
            print("Preview TLS certificate: " + result + "; no private certificate or command output logged")
    except Exception:
        # Providers can include credentials or a private payload in errors.
        # Never print their exception/traceback or Certbot's diagnostic files.
        raise SystemExit("Preview certificate operation failed; private command output suppressed") from None


if __name__ == "__main__":
    main()
