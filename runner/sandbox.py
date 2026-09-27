"""Untrusted execution container. Its stdout never carries control-plane state."""
import json
import os
from pathlib import Path
import subprocess
import time
import resource
import socket

request_path = Path("/request/request.json")
while not request_path.exists():
    time.sleep(0.1)
request = json.loads(request_path.read_bytes())
os.chdir("/work")
output = socket.create_connection(("127.0.0.1", 7071), timeout=5)
if output.recv(1) != b"R":
    raise RuntimeError("output collector did not register the stream")
output.settimeout(None)
os.dup2(output.fileno(), 1)
os.dup2(output.fileno(), 2)
output.close()
resource.setrlimit(resource.RLIMIT_NPROC, (request["processes"], request["processes"]))
environment = {key: value for key, value in os.environ.items() if key in {
    "PATH", "HOME", "LANG", "PYTHONUNBUFFERED", "PYTHONDONTWRITEBYTECODE",
    "MPLCONFIGDIR", "OPENBLAS_NUM_THREADS", "OMP_NUM_THREADS"
}}
if request["kind"] == "python":
    os.execve("/usr/local/bin/python", ["python", "-I", "/opt/aidash/kernel.py"], environment)
process = subprocess.Popen(["/bin/sh", "-c", request["code"]], start_new_session=True, env=environment)
result = process.wait()
# PID 1 adopts orphaned descendants. Background jobs retain the operation's
# identity and deadline until they exit, are cancelled, or the container dies.
while True:
    try:
        child, _ = os.waitpid(-1, os.WNOHANG)
        if child == 0:
            time.sleep(0.1)
    except ChildProcessError:
        break
raise SystemExit(result if result >= 0 else 128 - result)
