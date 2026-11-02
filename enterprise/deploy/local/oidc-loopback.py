# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

"""Preserve the exact development loopback issuer inside Marketplace's network."""
import os
import selectors
import socket
import socketserver


class Relay(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            upstream = socket.create_connection(("authentication", 8081), timeout=10)
        except OSError:
            self.request.sendall(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            return
        with upstream, selectors.DefaultSelector() as selector:
            selector.register(self.request, selectors.EVENT_READ, upstream)
            selector.register(upstream, selectors.EVENT_READ, self.request)
            while True:
                for key, _ in selector.select(timeout=30):
                    try:
                        data = key.fileobj.recv(65536)
                        if not data:
                            return
                        key.data.sendall(data)
                    except OSError:
                        return


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


with Server(("127.0.0.1", int(os.environ["AUTH_PORT"])), Relay) as server:
    print("Authentication loopback relay ready", flush=True)
    server.serve_forever()
