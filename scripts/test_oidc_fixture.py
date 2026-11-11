"""The headless browser session logs people in through the fixture issuer."""
import base64
import hashlib
import http.server
import json
import pathlib
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.parse
import urllib.request

import oidc_fixture


class FakeAidash(http.server.BaseHTTPRequestHandler):
    """Just enough of /auth/* and the approval route to drive a session."""

    origin = ""
    issuer = ""
    verifier = "v" * 43
    subjects = []
    approved = []

    def log_message(self, *_):
        pass

    def reply(self, status, body=None, headers=()):
        raw = b"" if body is None else json.dumps(body).encode()
        self.send_response(status)
        for name, value in headers:
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        path = urllib.parse.urlsplit(self.path)
        if path.path == "/auth/login":
            challenge = oidc_fixture.base64url(hashlib.sha256(self.verifier.encode()).digest())
            query = urllib.parse.urlencode({
                "client_id": oidc_fixture.CLIENT_ID, "code_challenge_method": "S256", "code_challenge": challenge,
                "nonce": "n", "state": "s", "redirect_uri": self.origin_https + "/auth/callback"})
            self.reply(307, headers=[("Location", f"{self.issuer}/authorize?{query}"), ("Set-Cookie", "__Host-aidash-login=l; Secure")])
        elif path.path == "/auth/callback":
            assert "__Host-aidash-login=l" in self.headers.get("Cookie", "")
            code = urllib.parse.parse_qs(path.query)["code"][0]
            form = urllib.parse.urlencode({"grant_type": "authorization_code", "code": code, "code_verifier": self.verifier,
                                           "redirect_uri": self.origin_https + "/auth/callback"}).encode()
            with urllib.request.urlopen(urllib.request.Request(self.issuer + "/token", data=form)) as token:
                claims = json.load(token)["id_token"].split(".")[1]
            subject = json.loads(base64.urlsafe_b64decode(claims + "=" * (-len(claims) % 4)))["sub"]
            self.subjects.append(subject)
            self.reply(303, headers=[("Set-Cookie", "__Host-aidash-login=; Max-Age=0"),
                                     ("Set-Cookie", "__Host-aidash-session=s1; HttpOnly; Secure"), ("Set-Cookie", "aidash-csrf=c1")])
        elif path.path == "/auth/session":
            mappings = [{"id": "m1", "tenant": "acme", "subject": "alice"}] if self.approved else []
            self.reply(200, {"mappings": mappings})
        else:
            self.reply(404)

    def do_POST(self):
        if self.path == "/auth/registration":
            assert self.headers["X-Aidash-CSRF"] == "c1" and self.headers["Origin"] == self.origin_https
            self.reply(200, {"id": "r1", "status": "pending"})
        elif self.path == "/api/dashboard/registrations/r1/approve":
            assert self.headers["Authorization"] == "Bearer admin-token"
            self.approved.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            self.reply(200, {"tenant": "acme", "subject": "alice"})
        elif self.path == "/api/tenants/acme/nodes":
            assert self.headers["X-Aidash-Context"] == "mapping:acme:m1"
            self.reply(201, {"name": "lab"})
        else:
            self.reply(404)


class BrowserSessionTests(unittest.TestCase):
    def test_a_tenant_administrator_acts_through_an_approved_mapping(self):
        with tempfile.TemporaryDirectory() as directory, oidc_fixture.IdentityProvider(pathlib.Path(directory)) as provider:
            FakeAidash.subjects, FakeAidash.approved = [], []
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakeAidash)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            self.addCleanup(server.server_close)
            self.addCleanup(server.shutdown)
            base = f"http://127.0.0.1:{server.server_port}"
            FakeAidash.issuer = provider.issuer
            FakeAidash.origin_https = f"https://127.0.0.1:{server.server_port}"

            session = oidc_fixture.tenant_administrator(base, FakeAidash.origin_https, provider, "admin-token", "acme", "alice")

            self.assertEqual(FakeAidash.subjects, ["administrator-acme-alice"])
            self.assertEqual(FakeAidash.approved, [{"tenant": "acme", "subject": "alice"}])
            self.assertNotIn("__Host-aidash-login", session.cookies)
            self.assertEqual(session.api("/api/tenants/acme/nodes", {"label": "Lab"}), {"name": "lab"})
            self.assertEqual(session.headers()["X-Aidash-Context"], "mapping:acme:m1")
            self.assertEqual(session.headers()["Cookie"], "__Host-aidash-session=s1; aidash-csrf=c1")
            self.assertEqual(provider.calls["exchange"], 1)

    def test_the_approval_creates_the_administrator_in_the_given_groups(self):
        with tempfile.TemporaryDirectory() as directory, oidc_fixture.IdentityProvider(pathlib.Path(directory)) as provider:
            FakeAidash.subjects, FakeAidash.approved = [], []
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakeAidash)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            self.addCleanup(server.server_close)
            self.addCleanup(server.shutdown)
            FakeAidash.issuer = provider.issuer
            FakeAidash.origin_https = f"https://127.0.0.1:{server.server_port}"

            oidc_fixture.tenant_administrator(f"http://127.0.0.1:{server.server_port}", FakeAidash.origin_https, provider,
                                              "admin-token", "acme", "alice", groups=["administrators"])

            self.assertEqual(FakeAidash.approved, [{"tenant": "acme", "subject": "alice", "groups": ["administrators"]}])


def authorize(issuer, **extra):
    """The subject the issuer signs in for one authorization request."""
    query = urllib.parse.urlencode({"client_id": oidc_fixture.CLIENT_ID, "code_challenge_method": "S256",
                                    "code_challenge": "c", "nonce": "n", "state": "s",
                                    "redirect_uri": "https://127.0.0.1:1/auth/callback", **extra})
    status, headers, _ = oidc_fixture._send(urllib.request.Request(f"{issuer}/authorize?{query}"))
    assert status == 307, status
    return urllib.parse.parse_qs(urllib.parse.urlsplit(headers["Location"]).query)["code"][0]


class IssuerTests(unittest.TestCase):
    def test_login_hint_names_the_person_and_its_absence_the_default(self):
        with tempfile.TemporaryDirectory() as directory, oidc_fixture.IdentityProvider(pathlib.Path(directory)) as provider:
            hinted = authorize(provider.issuer, login_hint="carol")
            plain = authorize(provider.issuer)

            self.assertEqual(provider.codes[hinted]["subject"], "carol")
            self.assertEqual(provider.codes[plain]["subject"], oidc_fixture.DEFAULT_SUBJECT)

    def test_a_pod_issuer_names_the_url_its_relays_answer_at_and_reaches_through_one(self):
        with tempfile.TemporaryDirectory() as directory:
            key = oidc_fixture.generate_key(pathlib.Path(directory) / "given.pem")
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            issuer = f"http://127.0.0.1:{port}/issuer"
            with oidc_fixture.IdentityProvider(key=key, issuer=issuer) as provider:
                upstream = f"127.0.0.1:{provider.server.server_port}"
                relay = subprocess.Popen([sys.executable, "-c", oidc_fixture.RELAY, str(port), upstream])
                self.addCleanup(relay.wait, 10)
                self.addCleanup(relay.terminate)
                for _ in range(100):
                    self.assertIsNone(relay.poll(), "the relay exited")
                    try:
                        socket.create_connection(("127.0.0.1", port), timeout=0.1).close()
                        break
                    except OSError:
                        time.sleep(0.05)

                with urllib.request.urlopen(issuer + "/.well-known/openid-configuration", timeout=5) as response:
                    discovery = json.load(response)
                code = authorize(issuer, login_hint="dave")

                self.assertEqual(provider.key, key)
                self.assertEqual(discovery["issuer"], issuer)
                self.assertEqual(discovery["authorization_endpoint"], issuer + "/authorize")
                claims = provider.token("n", provider.codes[code]["subject"]).split(".")[1]
                claims = json.loads(base64.urlsafe_b64decode(claims + "=" * (-len(claims) % 4)))
                self.assertEqual((claims["iss"], claims["sub"]), (issuer, "dave"))

    def test_the_relay_sidecar_runs_inline_and_forwards_to_the_issuer_service(self):
        container = oidc_fixture.relay_container(4100, "oidc:8000", "python:3.13-slim")

        self.assertEqual(container["command"], ["python3", "-c", oidc_fixture.RELAY, "4100", "oidc:8000"])
        self.assertNotIn("volumeMounts", container)


if __name__ == "__main__":
    unittest.main()
