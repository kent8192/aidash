"""A private OIDC issuer and a headless browser session for acceptance scripts.

Only the external OIDC provider is a protocol fixture. Aidash's `/auth/login`,
`/auth/callback`, cookies, CSRF, Mapping selection and authorization stay real.

A Deployment Administrator holds no Tenant authority (#484). Operations that
administer a Tenant (Node creation, Node Registrations, Peer Mappings, other
members' Subject Credentials) belong to a Tenant Administrator, which only a
person acting through a browser Mapping can be. `tenant_administrator` plays
that person: it logs in through the issuer, asks for a Mapping, has the
Deployment Administrator approve it through the administered
`/api/dashboard/registrations/{id}/approve` route, and then acts with the
dashboard's cookie, CSRF token, Origin and Mapping context.
"""

import base64
import hashlib
import http.server
import json
import secrets
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

CLIENT_ID = "aidash-bruno"
DEFAULT_SUBJECT = "bruno-user"
API_TIMEOUT = 60


def base64url(value):
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode("ascii")


def checked(command, **kwargs):
    return subprocess.run(command, check=True, capture_output=True, **kwargs)


class IdentityProvider:
    """A private RS256 issuer with bound authorization codes and S256 PKCE.

    Every login signs in the person named by `next_subject` (the `sub` claim),
    which `BrowserSession.login` sets before it starts. Aidash maps an External
    Identity by issuer and `sub`, so distinct subjects are distinct people.
    """

    def __init__(self, directory, client_id=CLIENT_ID):
        self.client_id = client_id
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
        self.next_subject = DEFAULT_SUBJECT
        self.lock = threading.Lock()
        self.login_lock = threading.Lock()
        self.calls = {
            "discovery": 0,
            "authorization": 0,
            "exchange": 0,
            "jwks": 0,
        }
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), self.handler())
        self.server.daemon_threads = True
        self.origin = f"http://127.0.0.1:{self.server.server_port}"
        self.issuer = self.origin + "/issuer"
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

    def token(self, nonce, subject=DEFAULT_SUBJECT):
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
                    "aud": self.client_id,
                    "sub": subject,
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
                if parsed.path == "/issuer/.well-known/openid-configuration":
                    provider.count("discovery")
                    self.respond(
                        200,
                        {
                            "issuer": provider.issuer,
                            "authorization_endpoint": provider.issuer + "/authorize",
                            "token_endpoint": provider.issuer + "/token",
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
                elif parsed.path == "/issuer/certs":
                    provider.count("jwks")
                    self.respond(200, {"keys": [provider.jwk]})
                elif parsed.path == "/issuer/authorize":
                    query = urllib.parse.parse_qs(parsed.query)
                    if query.get("client_id") != [provider.client_id] or query.get(
                        "code_challenge_method"
                    ) != ["S256"]:
                        self.respond(400, {"error": "invalid_authorization_request"})
                        return
                    redirect = query.get("redirect_uri", [""])[0]
                    if (
                        not redirect.startswith("https://127.0.0.1:")
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
                            "subject": provider.next_subject,
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
                else:
                    self.respond(404, {"error": "not_found"})

            def do_POST(self):
                if self.path != "/issuer/token":
                    self.respond(404, {"error": "not_found"})
                    return
                form = urllib.parse.parse_qs(
                    self.rfile.read(
                        int(self.headers.get("Content-Length", "0"))
                    ).decode()
                )
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
                        "id_token": provider.token(code["nonce"], code["subject"]),
                    },
                )

        return Handler


def oidc_environment(provider, browser_origin, idle_seconds=None):
    """The server environment that points a Node at `provider`.

    `browser_origin` is the HTTPS origin the dashboard is served from; the
    fixture issuer accepts only a callback below `https://127.0.0.1:`. The
    server itself stays on plain HTTP: the session cookies are `Secure`, so
    `BrowserSession` carries them by hand. `idle_seconds` lengthens the session
    for scripts whose browser phase runs long after the login.
    """
    environment = {
        "AIDASH_OIDC_ISSUER": provider.issuer,
        "AIDASH_OIDC_CLIENT_ID": provider.client_id,
        "AIDASH_OIDC_CLIENT_SECRET": secrets.token_urlsafe(32),
        "AIDASH_OIDC_PUBLIC_ORIGIN": browser_origin,
    }
    if idle_seconds:
        environment["AIDASH_OIDC_SESSION_IDLE_SECONDS"] = str(idle_seconds)
        environment["AIDASH_OIDC_SESSION_ABSOLUTE_SECONDS"] = str(
            max(idle_seconds, 43200)
        )
    return environment


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_):
        return None


# A direct opener: it follows no redirect and honors no proxy variable.
_OPENER = urllib.request.build_opener(_NoRedirect, urllib.request.ProxyHandler({}))


def _send(request):
    """The status, headers and raw body of `request`, whatever the status."""
    try:
        with _OPENER.open(request, timeout=API_TIMEOUT) as response:
            return response.status, response.headers, response.read()
    except urllib.error.HTTPError as error:
        with error:
            return error.code, error.headers, error.read()


def _json(raw):
    try:
        return json.loads(raw) if raw else None
    except ValueError:
        return raw.decode(errors="replace")


class BrowserSession:
    """A headless browser: a cookie jar over the server's real login flow.

    `login` runs `/auth/login` -> the issuer's `/authorize` -> `/auth/callback`
    exactly as Bruno's journeys 18-20 do. The session cookie is `Secure` and
    the server listens on plain HTTP, so cookies are kept by hand and sent with
    each request, together with the Origin the dashboard would send, the CSRF
    token it reads from its cookie, and the `x-aidash-context` Mapping selector.
    """

    def __init__(self, base, browser_origin, provider):
        self.base = base
        self.browser_origin = browser_origin
        self.provider = provider
        self.cookies = {}
        self.csrf = None
        self.context = None

    def _take_cookies(self, headers):
        for header in headers.get_all("Set-Cookie") or []:
            pair, *attributes = [part.strip() for part in header.split(";")]
            name, _, value = pair.partition("=")
            if value == "" or "max-age=0" in (a.lower() for a in attributes):
                self.cookies.pop(name, None)
            else:
                self.cookies[name] = value

    def _cookie_header(self):
        return "; ".join(f"{name}={value}" for name, value in self.cookies.items())

    def login(self, subject=DEFAULT_SUBJECT):
        """Sign in as the person `subject` and keep the opaque session and CSRF token."""
        self.cookies.clear()
        status, headers, _ = _send(
            urllib.request.Request(
                self.base + "/auth/login", headers={"Accept": "application/json"}
            )
        )
        assert status == 307, f"/auth/login answered {status}"
        authorization = headers["Location"]
        assert authorization.startswith(self.provider.issuer), authorization
        self._take_cookies(headers)
        # The issuer, not Aidash, answers /authorize; it names the person to sign in.
        # Logins are serialized so that concurrent sessions cannot swap people.
        with self.provider.login_lock:
            self.provider.next_subject = subject
            status, headers, _ = _send(urllib.request.Request(authorization))
        assert status == 307, f"the issuer's authorize answered {status}"
        callback = headers["Location"]
        assert callback.startswith(self.browser_origin + "/auth/callback?"), callback
        status, headers, _ = _send(
            urllib.request.Request(
                self.base + callback[len(self.browser_origin) :],
                headers={
                    "Accept": "application/json",
                    "Cookie": self._cookie_header(),
                },
            )
        )
        assert status == 303, f"/auth/callback answered {status}"
        self._take_cookies(headers)
        assert "__Host-aidash-session" in self.cookies, "login set no session cookie"
        self.csrf = self.cookies["aidash-csrf"]
        return self

    def headers(self, context=True):
        """The headers the dashboard sends with a write: cookies, Origin, CSRF token and Mapping context.

        Other processes (Playwright's request context) reuse the session
        with them.
        """
        headers = {
            "Cookie": self._cookie_header(),
            "Origin": self.browser_origin,
            "X-Aidash-CSRF": self.csrf,
        }
        if context and self.context:
            headers["X-Aidash-Context"] = self.context
        return headers

    def call(self, method, path, body=None, context=True):
        """The status and parsed body of one dashboard request.

        `context` sends the selected Mapping (see `select`) with the request.
        """
        headers = {"Accept": "application/json", **self.headers(context)}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        status, _, raw = _send(
            urllib.request.Request(
                self.base + path, data=data, headers=headers, method=method
            )
        )
        return status, _json(raw)

    def api(self, path, body=None):
        """Like the scripts' `api_request`: POST `body` when given, else GET; raise on failure."""
        status, parsed = self.call("GET" if body is None else "POST", path, body)
        if status >= 400:
            raise RuntimeError(f"{path} answered {status}: {parsed}")
        return parsed

    def select(self, tenant):
        """Act through this person's Mapping of `tenant`."""
        status, session = self.call("GET", "/auth/session", context=False)
        assert status == 200, f"/auth/session answered {status}"
        mapping = next(m for m in session["mappings"] if m["tenant"] == tenant)
        self.context = f"mapping:{tenant}:{mapping['id']}"
        return mapping


def tenant_administrator(base, browser_origin, provider, api_token, tenant, subject):
    """A browser session acting as `subject` of `tenant`, a Tenant Administrator.

    The person registers through the dashboard; the Deployment Administrator
    approves the Mapping through the administered approve route and holds no
    other Tenant authority. `subject` must exist in the Tenant's policy and be
    allowed the administration it will exercise. The person's External Identity
    is unique to the Tenant and subject, so one Node can host many such people.
    """
    session = BrowserSession(base, browser_origin, provider)
    session.login(f"administrator-{tenant}-{subject}")
    status, registration = session.call("POST", "/auth/registration", context=False)
    assert status == 200 and registration["status"] == "pending", registration
    request = urllib.request.Request(
        base + f"/api/dashboard/registrations/{registration['id']}/approve",
        data=json.dumps({"tenant": tenant, "subject": subject}).encode(),
        headers={
            "Authorization": f"Bearer {api_token}",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    status, _, raw = _send(request)
    assert status == 200, f"approving the Mapping answered {status}: {raw[:300]!r}"
    session.select(tenant)
    return session

