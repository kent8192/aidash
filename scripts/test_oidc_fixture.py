"""The headless browser session logs people in through the fixture issuer."""
import base64
import hashlib
import http.server
import json
import pathlib
import tempfile
import threading
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


if __name__ == "__main__":
    unittest.main()
