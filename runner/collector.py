"""Trusted collector in a separate container and PID namespace.

Only Kubernetes exec may hydrate/export or change the liveness deadline. The
HTTP listener is read-only; stdout of the execution container is never parsed.
"""
import base64
import fcntl
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import stat
import sys
import time
import threading
import socketserver

CONTROL = Path("/tmp/control.json")
REQUEST = Path("/request/request.json")
ROOTS = {"working": Path("/work"), "references": Path("/references"), "received": Path("/received")}
OUTPUT = Path("/tmp/output.bin")
OUTPUT_STATE = Path("/tmp/output.state")
OUTPUT_LIMIT = int(os.environ["AIDASH_OUTPUT_BYTES"])
# The last bytes of the output, kept even after the stored prefix is full: a
# Completion Check's failure summary comes at the end of a long run.
OUTPUT_TAIL = Path("/tmp/output.tail")
TAIL_LIMIT = 16384
# Present once the liveness deadline expired while the command still ran. Only
# that failing probe stops a command at its deadline, so this, not the measured
# runtime, says whether a Check timed out. The probe records it, and so does the
# deadline watcher when no probe came between the deadline and the command's exit.
EXPIRED = Path("/tmp/deadline.expired")


def save(path, value):
    temporary = path.with_suffix(".new")
    with temporary.open("w") as f:
        json.dump(value, f, separators=(",", ":"))
        f.flush()
        os.fsync(f.fileno())
    os.replace(temporary, path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def path_parts(value):
    parts = value.split("/")
    if (not value or len(value.encode()) > 1024 or "\\" in value
            or any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value)
            or any(p in ("", ".", "..") for p in parts)):
        raise ValueError("invalid relative file path")
    return parts


def hydrate():
    incoming = json.load(sys.stdin)
    if REQUEST.exists():
        if json.loads(REQUEST.read_bytes())["digest"] != incoming["digest"]:
            raise ValueError("input digest changed")
        state = json.loads(CONTROL.read_bytes()) if CONTROL.exists() else {}
        print(json.dumps({"accepted": True, "started_at": state.get("started_at")}))
        return
    total = 0
    seen = set()
    # Execution has not started before REQUEST is atomically published.
    for entry in incoming["files"]:
        parts = path_parts(entry["path"])
        identity = (entry["scope"], entry["path"])
        if identity in seen:
            raise ValueError("duplicate path")
        seen.add(identity)
        root = ROOTS[entry["scope"]]
        path = root.joinpath(*parts)
        total += entry["size"]
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as f:
            st = os.fstat(f.fileno())
            if not stat.S_ISREG(st.st_mode) or st.st_nlink != 1 or st.st_size != entry["size"]:
                raise ValueError("invalid input file")
            digest = hashlib.sha256()
            while data := f.read(1048576):
                digest.update(data)
        if total > incoming["working_bytes"] or digest.hexdigest() != entry["digest"]:
            raise ValueError("file integrity or quota")
    # The command starts once REQUEST is published below; its deadline and
    # the start it is charged from are taken together, just before.
    started_at = time.time()
    save(CONTROL, {"deadline": time.monotonic() + incoming["seconds"], "cancelled": False,
                   "started_at": started_at})
    if incoming["kind"] == "python":
        from jupyter_client.connect import write_connection_file
        write_connection_file('/request/kernel.json', ip='127.0.0.1', shell_port=7080,
                              iopub_port=7081, stdin_port=7082, hb_port=7083, control_port=7084,
                              key=os.urandom(32).hex().encode('ascii'))
        os.chmod('/request/kernel.json', 0o444)
    save(REQUEST, {k: incoming[k] for k in ("digest", "kind", "code", "processes")})
    print(json.dumps({"accepted": True, "started_at": started_at}))


def export():
    limit = int(sys.argv[2])
    total = 0
    files = []
    # Caller must first observe the execution container terminated. This is
    # never invoked concurrently with a live writer.
    for directory, dirs, names in os.walk(ROOTS["working"], followlinks=False):
        for name in dirs:
            if not stat.S_ISDIR(os.lstat(Path(directory) / name).st_mode):
                raise ValueError("unsafe directory")
        for name in names:
            path = Path(directory) / name
            relative = path.relative_to(ROOTS["working"]).as_posix()
            path_parts(relative)
            fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            with os.fdopen(fd, "rb") as f:
                st = os.fstat(f.fileno())
                if not stat.S_ISREG(st.st_mode) or st.st_nlink != 1:
                    raise ValueError("unsafe file or hard-link alias")
                total += st.st_size
                if total > limit or len(files) >= 4096:
                    raise ValueError("working file quota")
                digest = hashlib.sha256()
                size = 0
                while data := f.read(1048576):
                    digest.update(data)
                    size += len(data)
                if size != st.st_size:
                    raise ValueError("file changed during collection")
            files.append({"path": relative, "object_id": hashlib.sha256(relative.encode()).hexdigest(),
                          "digest": digest.hexdigest(), "size": size})
    json.dump({"files": files}, sys.stdout, separators=(",", ":"))


def write_input():
    if REQUEST.exists():
        raise ValueError("execution already started")
    scope, name, offset = sys.argv[2], sys.argv[3], int(sys.argv[4])
    path = ROOTS[scope].joinpath(*path_parts(name))
    path.parent.mkdir(parents=True, exist_ok=True)
    data = sys.stdin.buffer.read(4194305)
    if len(data) > 4194304 or offset < 0:
        raise ValueError("input chunk limit")
    flags = os.O_WRONLY | os.O_NOFOLLOW | os.O_CREAT
    if offset == 0:
        flags |= os.O_TRUNC
    fd = os.open(path, flags, 0o666)
    with os.fdopen(fd, "r+b") as f:
        os.fchmod(f.fileno(), 0o666)
        f.seek(offset)
        f.write(data)
        f.flush()
        os.fsync(f.fileno())


def read_output():
    name, offset = sys.argv[2], int(sys.argv[3])
    path = ROOTS["working"].joinpath(*path_parts(name))
    if offset < 0:
        raise ValueError("invalid output offset")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as f:
        st = os.fstat(f.fileno())
        if not stat.S_ISREG(st.st_mode) or st.st_nlink != 1:
            raise ValueError("unsafe output file")
        f.seek(offset)
        sys.stdout.buffer.write(f.read(4194304))


def append_output(data, connections=0):
    with Path("/tmp/output.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        state = json.loads(OUTPUT_STATE.read_bytes()) if OUTPUT_STATE.exists() else {"total": 0, "stored": 0, "connections": 0}
        state["connections"] += connections
        with OUTPUT.open("ab") as output:
            remaining = max(0, OUTPUT_LIMIT - state["stored"])
            captured = data[:remaining]
            output.write(captured)
            state["stored"] += len(captured)
            state["total"] += len(data)
            if captured:
                output.flush()
                os.fsync(output.fileno())
        if data:
            tail = (OUTPUT_TAIL.read_bytes() if OUTPUT_TAIL.exists() else b"") + data
            temporary = OUTPUT_TAIL.with_suffix(".new")
            temporary.write_bytes(tail[-TAIL_LIMIT:])
            os.replace(temporary, OUTPUT_TAIL)
        if data or connections:
            save(OUTPUT_STATE, state)
        return state


def logs(limit):
    deadline = time.monotonic() + 5
    state = append_output(b"")
    while "final" in sys.argv and state["connections"] and time.monotonic() < deadline:
        time.sleep(0.01)
        state = append_output(b"")
    if "tail" in sys.argv:
        data = OUTPUT_TAIL.read_bytes()[-min(limit, TAIL_LIMIT):] if OUTPUT_TAIL.exists() else b""
        cut = state["total"] > len(data)
    else:
        with OUTPUT.open("rb") as output:
            data = output.read(min(limit, OUTPUT_LIMIT))
        cut = state["total"] > OUTPUT_LIMIT
    print(json.dumps({"stdout": base64.b64encode(data).decode(), "bytes": state["stored"],
                      "truncated": cut or ("final" in sys.argv and state["connections"] > 0),
                      "deadline_expired": EXPIRED.exists()}))


def prepared():
    """The published request's digest and the start stamped with it, read
    only: a controller that restarts after the command ended still learns
    when it started."""
    state = json.loads(CONTROL.read_bytes()) if CONTROL.exists() else {}
    print(json.dumps({"digest": json.loads(REQUEST.read_bytes())["digest"] if REQUEST.exists() else None,
                      "started_at": state.get("started_at")}))


def deadline_state():
    """The command's liveness deadline, or None when none can expire: before
    the request is published, after a cancellation, or for a Python session."""
    try:
        state = json.loads(CONTROL.read_bytes())
        kind = json.loads(REQUEST.read_bytes())["kind"]
    except FileNotFoundError:
        return None
    return None if state["cancelled"] or kind == "python" else state["deadline"]


def expire_if_running():
    """Record an expired deadline once it has passed while the command's output
    stream is still open, whether or not a liveness probe arrived after it. The
    execution container's PID 1 holds that stream until the command and its
    background jobs exit. Returns whether the deadline is decided."""
    if not CONTROL.exists() or not REQUEST.exists():
        return False
    deadline = deadline_state()
    if deadline is None or EXPIRED.exists():
        return True
    if time.monotonic() < deadline:
        return False
    if append_output(b"")["connections"] > 0:
        EXPIRED.touch()
    return True


def expire_if_late():
    """Record an expired deadline when the command's output stream closes after
    it, which the watcher's sampling could miss."""
    deadline = deadline_state()
    if deadline is not None and time.monotonic() >= deadline:
        EXPIRED.touch()


def watch_deadline():
    while not expire_if_running():
        time.sleep(0.05)


class Output(socketserver.BaseRequestHandler):
    def handle(self):
        append_output(b"", connections=1)
        try:
            self.request.sendall(b"R")
            while data := self.request.recv(65536):
                append_output(data)
        finally:
            # The stream closes when the command exits: an exit after the
            # deadline timed out, whenever the watcher last looked.
            expire_if_late()
            append_output(b"", connections=-1)


class OutputServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


class Health(BaseHTTPRequestHandler):
    def do_GET(self):
        try:
            state = json.loads(CONTROL.read_bytes())
            healthy = not state["cancelled"] and time.monotonic() < state["deadline"]
            if not healthy and not state["cancelled"]:
                # Kubelet probes only a running container, so the command was
                # still running when its deadline passed.
                EXPIRED.touch()
        except FileNotFoundError:
            healthy = time.monotonic() < self.server.startup_deadline
        self.send_response(200 if healthy else 503)
        self.end_headers()

    def log_message(self, *_):
        pass


if __name__ == "__main__":
    mode = sys.argv[1]
    if mode == "hydrate":
        hydrate()
    elif mode == "export":
        export()
    elif mode == "write":
        write_input()
    elif mode == "read":
        read_output()
    elif mode == "prepared":
        prepared()
    elif mode == "cancel":
        save(CONTROL, {"cancelled": True, "deadline": 0})
    elif mode == "logs":
        logs(int(sys.argv[2]))
    elif mode == "serve":
        sys.path.insert(0, "/opt/aidash")
        import python_cells
        threading.Thread(target=python_cells.serve, daemon=True).start()
        output = OutputServer(("127.0.0.1", 7071), Output)
        threading.Thread(target=output.serve_forever, daemon=True).start()
        threading.Thread(target=watch_deadline, daemon=True).start()
        server = ThreadingHTTPServer(("0.0.0.0", 7070), Health)
        server.startup_deadline = time.monotonic() + 120
        server.serve_forever()
    else:
        raise SystemExit("unknown collector command")
