#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

"""Register the owner-only delivery App from a manifest; finalize only Aidash access."""

import argparse
import base64
import hmac
from http.server import BaseHTTPRequestHandler, HTTPServer
import html
import json
import os
from pathlib import Path
import re
import secrets
import ssl
import stat
import time
import urllib.error
import urllib.parse
import urllib.request

from setup import Refused, command, install_secrets, private_json

PERMISSIONS = {
    "actions": "read", "checks": "write", "contents": "write",
    "metadata": "read", "pull_requests": "write",
}
HOMEPAGE = "https://github.com/kent8192/aidash"
OWNER, REPOSITORY = "kent8192", "kent8192/aidash"
DIRECTORY = Path(__file__).resolve().parents[4] / "secrets/delivery/operator/github-app"


def manifest(port, session):
    return {
        "name": "Aidash delivery", "url": HOMEPAGE,
        "description": "Aidash release snapshots and deployment checks",
        "redirect_url": f"http://127.0.0.1:{port}/{session}/callback",
        "hook_attributes": {"url": HOMEPAGE, "active": False},
        "callback_urls": [], "default_permissions": dict(PERMISSIONS),
        "default_events": [], "request_oauth_on_install": False,
        "public": False,
    }


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, newurl):
        return None


class GitHubAPI:
    def __init__(self, token=None):
        self.token = token
        self.opener = urllib.request.build_opener(urllib.request.HTTPSHandler(context=ssl.create_default_context()), NoRedirect())

    def call(self, method, path, value=None):
        if method not in {"GET", "POST", "DELETE"} or not re.fullmatch(r"(?:app(?:/installations(?:/[1-9][0-9]*/access_tokens)?)?|app-manifests/[A-Za-z0-9_-]{8,256}/conversions|installation/(?:repositories|token))(?:\?per_page=100)?", path):
            raise Refused("Invalid fixed GitHub App operation")
        headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28", "Content-Type": "application/json"}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        request = urllib.request.Request("https://api.github.com/" + path, headers=headers, method=method,
                                         data=None if value is None else json.dumps(value).encode())
        try:
            with self.opener.open(request, timeout=30) as response:
                raw = response.read(1048577)
            if len(raw) > 1048576:
                raise Refused("GitHub App response exceeds its protected bound")
            return json.loads(raw) if raw else None
        except (OSError, ValueError, urllib.error.URLError):
            raise Refused("GitHub App operation failed; private provider output suppressed") from None


def validate_app(value, private_key=False):
    if (not isinstance(value, dict) or type(value.get("id")) is not int or value["id"] <= 0
            or value.get("owner", {}).get("login") != OWNER or value.get("owner", {}).get("type") != "User"
            or value.get("permissions") != PERMISSIONS or not re.fullmatch(r"[a-z0-9]+(?:-[a-z0-9]+)*", value.get("slug", ""))
            or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", value.get("client_id", ""))):
        raise Refused("The App must have the declared owner and exactly the release permissions")
    if private_key and (not isinstance(value.get("pem"), str) or len(value["pem"].encode()) > 16384 or not value["pem"].startswith(("-----BEGIN RSA PRIVATE KEY-----", "-----BEGIN PRIVATE KEY-----"))):
        raise Refused("Registration did not return a valid bounded private key")
    return {"id": value["id"], "client_id": value["client_id"], "slug": value["slug"], "owner": OWNER, "permissions": dict(PERMISSIONS)}


def save_registration(value, directory):
    metadata = validate_app(value, private_key=True)
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(directory, 0o700)
    key = directory / "release-app.pem"
    if key.exists():
        raise Refused("An App key already exists; finalize or inspect that registration before creating another")
    descriptor = os.open(key, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(value["pem"])
    command(["openssl", "pkey", "-in", str(key), "-check", "-noout"])
    private_json(directory / "metadata.json", metadata)
    return metadata


class RegistrationServer(HTTPServer):
    def __init__(self, directory, port=0):
        super().__init__(("127.0.0.1", port), RegistrationHandler)
        self.directory = Path(directory)
        self.session, self.state = secrets.token_urlsafe(32), secrets.token_urlsafe(32)
        self.deadline, self.finished, self.result = time.monotonic() + 3600, False, None
        self.timeout = 1

    def callback(self, query):
        state, code = query.get("state", []), query.get("code", [])
        if (time.monotonic() >= self.deadline or set(query) != {"state", "code"} or len(state) != 1 or len(code) != 1
                or not hmac.compare_digest(state[0], self.state) or not re.fullmatch(r"[A-Za-z0-9_-]{8,256}", code[0])):
            raise Refused("The App registration callback is expired or has invalid CSRF state")
        if self.finished:
            raise Refused("The registration callback has already been consumed")
        self.finished = True
        value = GitHubAPI().call("POST", "app-manifests/" + code[0] + "/conversions")
        self.result = save_registration(value, self.directory)
        return self.result


class RegistrationHandler(BaseHTTPRequestHandler):
    def log_message(self, format, *arguments):
        pass  # OAuth callback code and query must never enter an HTTP access log.

    def respond(self, status, body):
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("Content-Security-Policy", "default-src 'none'; form-action https://github.com; frame-ancestors 'none'; base-uri 'none'")
        self.end_headers()
        self.wfile.write(body.encode())

    def do_GET(self):
        port = self.server.server_address[1]
        if self.headers.get("Host") != f"127.0.0.1:{port}" or time.monotonic() >= self.server.deadline:
            self.respond(403, "Expired or invalid loopback registration request")
            return
        parsed = urllib.parse.urlsplit(self.path)
        if parsed.path == "/" + self.server.session and not parsed.query:
            payload = html.escape(json.dumps(manifest(port, self.server.session)), quote=True)
            target = "https://github.com/settings/apps/new?state=" + self.server.state
            self.respond(200, '<!doctype html><title>Aidash release App</title><p>Review the owner-only App permissions on GitHub, then install it only on kent8192/aidash.</p>'
                         + '<form method="post" action="' + target + '"><input type="hidden" name="manifest" value="' + payload + '"><button>Review GitHub App registration</button></form>')
        elif parsed.path == "/" + self.server.session + "/callback":
            try:
                metadata = self.server.callback(urllib.parse.parse_qs(parsed.query, strict_parsing=True))
                self.respond(200, 'App key saved privately. Install only on kent8192/aidash, then run finalize. <a href="https://github.com/apps/' + metadata["slug"] + '/installations/new">Install App</a>')
            except (Refused, ValueError, OSError):
                self.respond(403, "Registration could not be finalized. Inspect the local authenticated setup; no private provider output is displayed.")
        else:
            self.respond(404, "Not found")


def private_read(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(descriptor) as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) & 0o077:
            raise Refused("App input must be a private file owned by the current user")
        value = source.read(16385)
    if len(value.encode()) > 16384:
        raise Refused("Private App input is too large")
    return value


def jwt(metadata, key):
    def encode(value):
        return base64.urlsafe_b64encode(json.dumps(value, separators=(",", ":")).encode()).rstrip(b"=")
    now = int(time.time())
    data = encode({"alg": "RS256", "typ": "JWT"}) + b"." + encode({"iat": now - 60, "exp": now + 540, "iss": metadata["client_id"]})
    signature = command(["openssl", "dgst", "-sha256", "-sign", str(key)], data=data)
    return (data + b"." + base64.urlsafe_b64encode(signature).rstrip(b"=")).decode()


def validate_installations(metadata, api):
    actual = api.call("GET", "app")
    if validate_app(actual) != metadata:
        raise Refused("The stored App metadata differs from the authenticated App")
    installations = api.call("GET", "app/installations?per_page=100")
    if (not isinstance(installations, list) or len(installations) != 1 or installations[0].get("account", {}).get("login") != OWNER
            or installations[0].get("app_id") != metadata["id"] or installations[0].get("repository_selection") != "selected"
            or installations[0].get("suspended_at") is not None or type(installations[0].get("id")) is not int):
        raise Refused("Install the release App only on the declared owner's selected Aidash repository")
    installation = installations[0]
    result = api.call("POST", f"app/installations/{installation['id']}/access_tokens", {"permissions": {"metadata": "read"}})
    token = result.get("token", "")
    if not isinstance(token, str) or not token or len(token) > 8192:
        raise Refused("GitHub did not return the requested metadata-only verification token")
    client = GitHubAPI(token)
    try:
        if result.get("permissions") != {"metadata": "read"}:
            raise Refused("GitHub did not return the requested metadata-only verification token")
        inventory = client.call("GET", "installation/repositories?per_page=100")
        repositories = inventory.get("repositories", [])
        if inventory.get("total_count") != 1 or len(repositories) != 1 or repositories[0].get("full_name") != REPOSITORY:
            raise Refused("The App has access to repositories outside Aidash")
    finally:
        client.call("DELETE", "installation/token")
    return installation["id"]


def finalize(directory):
    directory = Path(directory)
    metadata = json.loads(private_read(directory / "metadata.json"))
    if (not isinstance(metadata, dict) or set(metadata) != {"id", "client_id", "slug", "owner", "permissions"}
            or metadata.get("owner") != OWNER or metadata.get("permissions") != PERMISSIONS
            or type(metadata.get("id")) is not int or metadata["id"] <= 0
            or not re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", metadata.get("client_id", ""))):
        raise Refused("Stored App metadata is invalid")
    key = directory / "release-app.pem"
    pem = private_read(key)
    api = GitHubAPI(jwt(metadata, key))
    installation = validate_installations(metadata, api)
    install_secrets({"release-automation": {"RELEASE_APP_PRIVATE_KEY": pem}})
    values = {"release_app": {name: metadata[name] for name in ("id", "client_id", "slug")}}
    values["release_app"]["installation_id"] = installation
    private_json(directory / "release_app.tfvars.json", values)
    return {"app_id": metadata["id"], "installation_id": installation, "protected_key_installed": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["manifest", "register", "finalize"])
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--directory", type=Path, default=DIRECTORY)
    args = parser.parse_args()
    try:
        if args.action == "manifest":
            print(json.dumps(manifest(args.port, secrets.token_urlsafe(32)), indent=2))
        elif args.action == "finalize":
            print(json.dumps(finalize(args.directory), sort_keys=True))
        else:
            if (args.directory / "release-app.pem").exists():
                raise Refused("An existing App registration must be finalized or inspected first")
            with RegistrationServer(args.directory, args.port) as server:
                print(f"Open http://127.0.0.1:{server.server_address[1]}/{server.session} to review registration", flush=True)
                while not server.finished and time.monotonic() < server.deadline:
                    server.handle_request()
                if not server.result:
                    raise Refused("Registration was not completed; no automatic retry or duplicate App was created")
                print("Install only Aidash at https://github.com/apps/" + server.result["slug"] + "/installations/new, then run finalize")
    except (Refused, OSError, ValueError, KeyError, TypeError):
        print("Release App setup incomplete; inspect private local inputs and authenticated GitHub settings")
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
