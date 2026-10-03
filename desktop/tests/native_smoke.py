"""Exercise the packaged Tauri binary using real Linux WebKitGTK and Secret Service.
The HTTP/OIDC broker is a deterministic fixture; Google's service is not used.
"""

import base64
import hashlib
import http.server
import json
from pathlib import Path
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

ARTIFACTS = Path("/workspace/desktop/artifacts")
SCENE = json.loads((ARTIFACTS / "scene.json").read_text())
SERVERS = []


class Fixture(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass  # Do not log callback URLs or credentials.

    def reply(self, status=200, value=None, extra=None, body=None):
        body = json.dumps(value).encode() if body is None else body
        self.send_response(status)
        self.send_header(
            "Content-Type", (extra or {}).get("Content-Type", "application/json")
        )
        self.send_header("Cache-Control", "no-store")
        origin = self.headers.get("Origin")
        if origin in ("tauri://localhost", "http://tauri.localhost"):
            self.send_header("Access-Control-Allow-Origin", origin)
        self.send_header(
            "Access-Control-Allow-Headers",
            "authorization,content-type,x-aidash-context,last-event-id",
        )
        self.send_header("Access-Control-Allow-Methods", "GET,POST,OPTIONS")
        for name, value in (extra or {}).items():
            if name != "Content-Type":
                self.send_header(name, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self):
        self.reply(204, body=b"")

    def do_POST(self):
        data = json.loads(
            self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}"
        )
        if self.path == "/auth/desktop/start":
            request = str(uuid.uuid4())
            self.server.pending[request] = data
            self.reply(
                value={
                    "authorization_url": f"http://127.0.0.1:{self.server.server_port}/auth/desktop/authorize?request={request}"
                }
            )
        elif self.path == "/auth/desktop/exchange":
            pending = self.server.codes.pop(data["code"])
            assert pending["state"] == data["state"]
            assert pending["redirect_uri"] == data["redirect_uri"]
            assert pending["code_challenge"] == base64.urlsafe_b64encode(
                hashlib.sha256(data["verifier"].encode()).digest()
            ).decode().rstrip("=")
            self.server.logins += 1
            self.tokens("aidash_refresh_" + uuid.uuid4().hex * 2)
        elif self.path == "/auth/desktop/refresh":
            if data["refresh_token"] not in self.server.refresh:
                self.reply(401, {})
            else:
                self.server.renewals += 1
                self.tokens(data["next_token"])
        elif self.path == "/auth/desktop/revoke":
            self.server.refresh.clear()
            self.server.access = None
            self.reply(204, body=b"")
        elif self.path == "/auth/activity":
            self.reply(204, body=b"")
        else:
            self.reply(404, {})

    def tokens(self, refresh):
        self.server.access = "aidash_desktop_" + uuid.uuid4().hex * 2
        self.server.refresh.add(refresh)
        self.reply(
            value={
                "access_token": self.server.access,
                "refresh_token": refresh,
                "expires_in": 300,
            }
        )

    def do_GET(self):
        path = urllib.parse.urlparse(self.path)
        if path.path == "/auth/config":
            self.reply(
                value={"enabled": True, "provider": "google", "desktop_protocol": 1}
            )
            return
        if path.path == "/auth/desktop/authorize":
            request = urllib.parse.parse_qs(path.query)["request"][0]
            pending = self.server.pending.pop(request)
            code = uuid.uuid4().hex * 2
            self.server.codes[code] = pending
            self.reply(
                302,
                {},
                {
                    "Location": pending["redirect_uri"]
                    + "?"
                    + urllib.parse.urlencode({"code": code, "state": pending["state"]})
                },
            )
            return
        self.server.requests.append(
            {
                "path": path.path,
                "cursor": self.headers.get("Last-Event-ID"),
                "context": self.headers.get("X-Aidash-Context"),
                "cookie": self.headers.get("Cookie"),
                "origin": self.headers.get("Origin"),
            }
        )
        if (
            not self.server.access
            or self.headers.get("Authorization") != "Bearer " + self.server.access
        ):
            self.reply(401, {})
            return
        if path.path == "/auth/session":
            self.reply(
                value={
                    "id": f"session-{self.server.server_port}",
                    "operator": True,
                    "mappings": [],
                }
            )
        elif path.path == "/auth/registration":
            self.reply(value=None)
        elif path.path == "/api/session":
            self.reply(
                value={
                    "access": {"kind": "operator"},
                    "node_id": SCENE["data"]["node"]["id"],
                }
            )
        elif path.path == "/api/state":
            self.reply(value=SCENE["data"])
        elif path.path == "/api/mesh":
            self.reply(value={"nodes": [], "errors": []})
        elif path.path == "/api/discover":
            self.reply(value=SCENE["discovery"])
        elif path.path.endswith("/message-history"):
            self.reply(value={"messages": [], "next_before": None})
        elif path.path == "/api/events/stream":
            self.server.streams.append(self.headers.get("Last-Event-ID"))
            self.reply(
                body=f"id: {self.server.server_port}\ndata: {{}}\n\n".encode(),
                extra={"Content-Type": "text/event-stream"},
            )
        elif path.path == "/api/workspaces/product-lab":
            data = SCENE["data"]
            self.reply(
                value={
                    "workspace": data["workspaces"][0],
                    "tasks": data["tasks"],
                    "artifacts": data["artifacts"],
                    "events": data["events"],
                    "messages": [],
                }
            )
        else:
            self.reply(value=[])


def fixture(port):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Fixture)
    server.pending, server.codes = {}, {}
    server.access = None
    server.refresh, server.requests, server.streams = set(), [], []
    server.logins = server.renewals = 0
    threading.Thread(target=server.serve_forever, daemon=True).start()
    SERVERS.append(server)
    return server


def webdriver(method, path, data=None):
    request = urllib.request.Request(
        "http://127.0.0.1:4444" + path,
        data=None if data is None else json.dumps(data).encode(),
        method=method,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=40) as response:
            result = json.load(response)["value"]
    except urllib.error.HTTPError as error:
        raise AssertionError(error.read().decode()) from error
    if isinstance(result, dict) and result.get("error"):
        raise AssertionError(result)
    return result


def wait(fn, description, timeout=30):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = fn()
            if value:
                return value
        except Exception as error:
            last = error
        time.sleep(0.2)
    raise AssertionError(f"Timed out: {description}: {last}")


class Browser:
    def __init__(self):
        result = webdriver(
            "POST",
            "/session",
            {
                "capabilities": {
                    "alwaysMatch": {
                        "tauri:options": {"application": "/build/debug/aidash-desktop"}
                    }
                }
            },
        )
        self.id = result["sessionId"]

    def js(self, script, *args):
        return webdriver(
            "POST",
            f"/session/{self.id}/execute/sync",
            {"script": script, "args": list(args)},
        )

    def async_js(self, script, *args):
        return webdriver(
            "POST",
            f"/session/{self.id}/execute/async",
            {"script": script, "args": list(args)},
        )

    def text(self):
        return self.js("return document.body.innerText")

    def button(self, text):
        wait(
            lambda: self.js(
                'const b=[...document.querySelectorAll("button")].find(b=>b.textContent.trim()===arguments[0]); if(b && !b.disabled){b.click(); return true} return false',
                text,
            ),
            "button " + text,
        )

    def invoke(self, command, **args):
        result = self.async_js(
            "const done=arguments[arguments.length-1]; window.__TAURI_INTERNALS__.invoke(arguments[0],arguments[1]).then(value=>done({value}),error=>done({error:String(error)}));",
            command,
            args,
        )
        if "error" in result:
            raise AssertionError(result["error"])
        return result.get("value")

    def close(self):
        webdriver("DELETE", f"/session/{self.id}")

    def screenshot(self, name):
        image = webdriver("GET", f"/session/{self.id}/screenshot")
        (ARTIFACTS / name).write_bytes(base64.b64decode(image))


def select(browser, id):
    browser.js(
        'const s=document.querySelector(".desktop-connections select"); s.value=arguments[0];s.dispatchEvent(new Event("change",{bubbles:true}));',
        id,
    )


def run():
    one, two = fixture(19081), fixture(19082)
    browser = Browser()
    print("Native session started", flush=True)
    try:
        wait(lambda: "Aidash" in browser.text(), "bundled startup")
        browser.js(
            'localStorage.setItem("aidash-locale","en-US"); history.replaceState({},"","/graph?channel=product-lab"); location.reload()'
        )
        wait(lambda: "Connect to Aidash" in browser.text(), "connections UI")
        # Invoke the restricted profile command; all subsequent lifecycle UI uses
        # the production adapter and native runtime, without mocked IPC.
        first = browser.invoke(
            "save_connection", name="First", origin="http://127.0.0.1:19081"
        )
        browser.js("location.reload()")
        wait(
            lambda: browser.js(
                'return document.querySelectorAll(".desktop-connections option").length'
            )
            == 2,
            "saved connection",
        )
        select(browser, first["id"])
        print("First profile selected", flush=True)
        browser.button("Sign in with Google")
        browser.button("operator")
        wait(
            lambda: one.streams and str(one.server_port) in one.streams,
            "SSE reconnect cursor",
        )
        assert one.streams[0] == "-1", one.streams
        # Exercise Graph View, the actual Cytoscape renderer and DOM labels.
        browser.js(
            'history.pushState({},"","/graph?channel=product-lab"); window.dispatchEvent(new PopStateEvent("popstate"))'
        )
        wait(
            lambda: browser.js(
                'return document.querySelectorAll(".mesh-canvas canvas").length'
            )
            > 0,
            "Graph canvas",
        )
        wait(
            lambda: browser.js(
                'return document.querySelectorAll(".mesh-node-label").length'
            )
            > 0,
            "Graph labels",
        )
        browser.button("Fit entire graph")
        geometry = wait(
            lambda: browser.js(
                'return [...document.querySelectorAll(".mesh-canvas canvas")].map(c=>({width:c.width,height:c.height,left:c.getBoundingClientRect().left,top:c.getBoundingClientRect().top}))'
            ),
            "Graph geometry",
        )
        assert all(c["width"] > 0 and c["height"] > 0 for c in geometry)
        browser.screenshot("linux-webkitgtk-graph.png")
        print("Graph and SSE verified", flush=True)
        # Persist credentials across process exit and restart. Browser cookies
        # cannot provide this session: the broker only issues Bearer credentials.
        browser.close()
        browser = Browser()
        browser.button("operator")
        print("Process restart restored the session", flush=True)
        assert one.logins == 1 and one.renewals >= 1, (one.logins, one.renewals)
        second = browser.invoke(
            "save_connection", name="Second", origin="http://127.0.0.1:19082"
        )
        browser.js("location.reload()")
        wait(
            lambda: browser.js(
                'return document.querySelectorAll(".desktop-connections option").length'
            )
            == 3,
            "second profile",
        )
        print("Second profile ready", flush=True)
        select(browser, second["id"])
        browser.button("Sign in with Google")
        browser.button("operator")
        wait(lambda: len(two.streams) >= 2, "second SSE")
        assert two.streams[0] == "-1", two.streams
        assert str(one.server_port) not in two.streams, two.streams
        assert not any(r["cookie"] for s in SERVERS for r in s.requests), (
            "desktop leaked cookies"
        )
        select(browser, first["id"])
        browser.button("operator")
        assert one.logins == 1, "switching connections must preserve login"
        # The native window cannot navigate to a remote document.
        before = browser.js("return location.href")
        browser.js('location.href="https://example.com/"')
        time.sleep(0.3)
        assert browser.js("return location.href") == before
        try:
            browser.invoke("plugin:opener|open_url", url="https://example.com/")
        except AssertionError:
            pass
        else:
            raise AssertionError("generic opener IPC was permitted")
        browser.js('document.querySelector(".account-popover").open = true')
        browser.button("Log out on this device")
        wait(lambda: "Sign in with Google" in browser.text(), "current-device logout")
        browser.close()
        browser = Browser()
        wait(lambda: "Sign in with Google" in browser.text(), "logout survives restart")
        assert one.access is None and not one.refresh
        report = {
            "startup": True,
            "external_browser_fixture_login": True,
            "persistent_secret_service_login": True,
            "connection_switch": True,
            "current_device_logout_persists": True,
            "sse_last_event_id": [one.streams, two.streams],
            "graph": geometry,
            "remote_navigation_blocked": True,
            "generic_native_opener_denied": True,
            "google_live": False,
            "platform": "Linux WebKitGTK / aarch64",
        }
        (ARTIFACTS / "native-results.json").write_text(json.dumps(report, indent=2))
        print(json.dumps(report, indent=2), flush=True)
    except Exception:
        browser.js(
            'const b=[...document.querySelectorAll("button")].find(b=>b.textContent.includes("Show Error")); if(b)b.click()'
        )
        print(browser.text(), flush=True)
        print(json.dumps([s.requests[-8:] for s in SERVERS]), flush=True)
        try:
            browser.screenshot("linux-failure.png")
        except Exception:
            pass
        raise
    finally:
        browser.close()
        for server in SERVERS:
            server.shutdown()


if __name__ == "__main__":
    driver = subprocess.Popen(
        ["tauri-driver"],
        stdout=open(ARTIFACTS / "driver.log", "w"),
        stderr=subprocess.STDOUT,
    )
    try:
        wait(lambda: webdriver("GET", "/status"), "WebKit driver")
        run()
    finally:
        driver.terminate()
        driver.wait(timeout=10)
