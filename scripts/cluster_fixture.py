#!/usr/bin/env python3
"""Cluster-local deterministic model/tool fixture; never calls a paid provider."""
import http.server
import json

from golden_path import Fixture

fixture = Fixture("aidash://ops-a", "aidash://ops-b")
base_handler = fixture.handler()


class Handler(base_handler):
    def do_GET(self):
        if self.path != "/status":
            self.send_error(404)
            return
        with fixture.lock:
            body = json.dumps({"remote_effect_started": fixture.remote_effect_started.is_set(), "effects": fixture.effects, "requests": dict(fixture.requests), "provider_calls": dict(fixture.provider_calls)}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


http.server.ThreadingHTTPServer(("0.0.0.0", 8000), Handler).serve_forever()
