"""Admission probe executed only from the immutable runtime image."""
import errno
import json
import importlib.metadata
import os
from pathlib import Path
import platform
import resource
import socket
import subprocess
import sys
import tempfile

report = {"kernel": platform.release(), "python": platform.python_version(), "uid": os.getuid(),
          "process_limit": resource.getrlimit(resource.RLIMIT_NPROC)[1]}
report["service_account_absent"] = not Path("/var/run/secrets/kubernetes.io/serviceaccount/token").exists()
report["host_sockets_absent"] = not any(Path(path).exists() for path in (
    "/var/run/docker.sock", "/run/containerd/containerd.sock", "/run/podman/podman.sock"))
try:
    Path("/etc/aidash-probe").write_text("must fail")
    report["root_read_only"] = False
except OSError:
    report["root_read_only"] = True
report["egress_denied"] = {}
for address, port in (("1.1.1.1", 443), ("169.254.169.254", 80)):
    try:
        with socket.create_connection((address, port), timeout=2):
            report["egress_denied"][address] = False
    except OSError:
        report["egress_denied"][address] = True
report["environment"] = sorted(os.environ)
report['disk_enforced'] = {}
for root, budget in [('/work', int(sys.argv[1])), ('/tmp', int(sys.argv[2]))]:
    with tempfile.TemporaryFile(dir=root) as file:
        try:
            os.posix_fallocate(file.fileno(), 0, budget + 4096)
            report['disk_enforced'][root] = False
        except OSError as error:
            report['disk_enforced'][root] = error.errno == errno.ENOSPC
# Verify fork works before lowering this probe's inherited hard limit. Exhausting
# the entire Pod pids.max also consumes the Sentry's host threads and can kill
# the verifier itself. The trusted node adapter independently checks the exact
# configured pids.max; this bounded probe exercises enforcement inside gVisor.
subprocess.run(['true'], check=True)
report['fork_probe_limit'] = 8
resource.setrlimit(resource.RLIMIT_NPROC, (report['fork_probe_limit'], report['fork_probe_limit']))
processes = []
try:
    for _ in range(report['fork_probe_limit'] + 1):
        processes.append(subprocess.Popen(['sleep','10']))
    report['fork_denied'] = False
except OSError as error:
    report['fork_denied'] = error.errno == errno.EAGAIN and len(processes) <= report['fork_probe_limit']
    report['fork_error'] = errno.errorcode.get(error.errno, str(error.errno))
finally:
    for process in processes:
        process.terminate()
    for process in processes:
        process.wait()
report['children_before_limit'] = len(processes)
report['base_packages'] = {}
for name in ('ipykernel','numpy','pandas','matplotlib','openpyxl','pypdf'):
    report['base_packages'][name] = importlib.metadata.version(name)
assert all(report['disk_enforced'].values()), report
assert report['fork_denied'], report
print(json.dumps(report, sort_keys=True))
assert "gvisor" in report["kernel"].lower()
assert report["uid"] != 0
assert report["service_account_absent"] and report["host_sockets_absent"] and report["root_read_only"]
assert all(report["egress_denied"].values())
