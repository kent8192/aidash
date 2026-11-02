"""Disposable positive-control listener for Execution Pod admission."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import socket
import sys
import threading

seen = threading.Event()
udp_ready = threading.Event()
control_seen = threading.Event()


def receive_udp(port, nonce):
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server:
        server.bind(('0.0.0.0', port))
        udp_ready.set()
        while True:
            data, _ = server.recvfrom(65535)
            if data == nonce:
                seen.set()
            elif data == b'control' + nonce:
                control_seen.set()


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if not udp_ready.is_set():
            self.send_error(503)
            return
        if self.path == '/seen':
            body = b'true' if seen.is_set() else b'false'
        elif self.path == '/control-seen':
            body = b'true' if control_seen.is_set() else b'false'
        else:
            body = b'ready'
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


if __name__ == '__main__':
    tcp, udp, nonce = int(sys.argv[1]), int(sys.argv[2]), bytes.fromhex(sys.argv[3])
    if len(nonce) != 16:
        raise SystemExit('invalid canary nonce')
    threading.Thread(target=receive_udp, args=(udp, nonce), daemon=True).start()
    ThreadingHTTPServer(('0.0.0.0', tcp), Handler).serve_forever()
