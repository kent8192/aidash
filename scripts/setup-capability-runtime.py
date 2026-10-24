#!/usr/bin/env python3
"""Create an isolated, disposable kind/gVisor acceptance environment.

Never reuses an existing cluster or changes the user's default kubeconfig. The
generated profile enables admission only in this explicit test installation.
Production Deployments must separately provision equivalent isolation and storage.
"""
import argparse
import bz2
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import shutil
import subprocess
import sys
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
NODE = "kindest/node@sha256:7416a61b42b1662ca6ca89f02028ac133a309a2a30ba309614e8ec94d976dc5a"
# The same pin the Runner image bundles; local setups still download it.
PIN = json.loads((ROOT / "runner/gvisor.json").read_text())
GVISOR = PIN["version"]
CILIUM = "1.20.2"
HASHES = PIN["sha256"]


def run(args, *, data=None, capture=False):
    result = subprocess.run([str(v) for v in args], input=data, check=True,
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout if capture else b""


def write_private(path, value):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as file:
        file.write(value)
        file.flush()
        os.fsync(file.fileno())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True, help="New, disposable kind cluster name")
    parser.add_argument("--directory", type=Path, required=True, help="New private state directory")
    parser.add_argument("--port", type=int, default=18949, help="Loopback runner port")
    parser.add_argument("--idle-seconds", type=int, default=1800,
                        help="Real interpreter idle timeout (1..1800; default unchanged)")
    parser.add_argument("--observe-allocations", action="store_true",
                        help="Capture allowlisted provider evidence before the controller observes it")
    args = parser.parse_args()
    if not re.fullmatch(r"aidash-[a-z0-9][a-z0-9-]{0,35}", args.name):
        parser.error("name must begin with aidash- and contain lowercase letters/digits/hyphens")
    if not 1024 <= args.port <= 65535:
        parser.error("port must be between 1024 and 65535")
    if not 1 <= args.idle_seconds <= 1800:
        parser.error("idle-seconds must be between 1 and 1800")
    executables = {name: shutil.which(name) for name in ("docker", "kind", "kubectl", "helm")}
    if not all(executables.values()):
        parser.error("docker, kind, kubectl and helm must be installed")
    if args.name in run([executables["kind"], "get", "clusters"], capture=True).decode().split():
        parser.error("cluster already exists; refusing to alter it")
    directory = args.directory.expanduser().resolve()
    directory.mkdir(parents=True, mode=0o700, exist_ok=False)
    kubectl = executables["kubectl"]
    if args.observe_allocations:
        evidence = directory / "allocation-evidence"
        evidence.mkdir(mode=0o700)
        observer = directory / "observe-kubectl"
        # The controller intentionally passes a minimal environment. Bind both
        # executable and capture path in this private wrapper, never via env.
        command = [sys.executable, str(ROOT / "scripts/observe-capability-kubectl.py"),
                   kubectl, str(evidence)]
        write_private(observer, "#!/bin/sh\nexec " +
                      " ".join(shlex.quote(value) for value in command) + ' "$@"\n')
        observer.chmod(0o700)
        kubectl = str(observer)
    write_private(directory / "owner.json", json.dumps({"cluster": args.name, "purpose": "Aidash capability acceptance", "source": str(ROOT)}))
    kubeconfig = directory / "kubeconfig"
    kind_config = directory / "kind.yaml"
    write_private(kind_config, f"""kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
networking:
  disableDefaultCNI: true
  podSubnet: 10.201.0.0/16
nodes:
- role: control-plane
  image: {NODE}
  kubeadmConfigPatches:
  - |
    kind: KubeletConfiguration
    podPidsLimit: 1024
    containerLogMaxSize: 10Mi
    containerLogMaxFiles: 2
""")
    run([executables["kind"], "create", "cluster", "--name", args.name,
         "--config", kind_config, "--kubeconfig", kubeconfig])
    kubeconfig.chmod(0o600)
    node = args.name + "-control-plane"
    docker = [executables["docker"], "exec", "-i", node]
    ctr = run(docker + ["sh", "-c", "command -v ctr"], capture=True).decode().strip()
    if not re.fullmatch(r"/[A-Za-z0-9_./+-]+", ctr):
        raise SystemExit("invalid node ctr executable path")
    run(docker + ["test", "-x", ctr])
    ctr_version = run(docker + [ctr, "--version"], capture=True).decode().strip()
    if not ctr_version:
        raise SystemExit("node ctr executable returned no version")
    print(json.dumps({"node_ctr": ctr, "node_ctr_version": ctr_version}), flush=True)
    kube = [executables["kubectl"], "--kubeconfig", kubeconfig]
    chart = run([executables["helm"], "template", "cilium", "cilium", "--repo", "https://helm.cilium.io/",
                 "--version", CILIUM, "--namespace", "kube-system", "--set", "operator.replicas=1",
                 "--set", "ipam.mode=kubernetes", "--set", "kubeProxyReplacement=false",
                 "--set", "securityContext.privileged=true"], capture=True)
    # The chart contains generated certificates. Send it straight to Kubernetes;
    # never persist or print it as an evidence artifact.
    run(kube + ["apply", "-f", "-"], data=chart)
    run(kube + ["-n", "kube-system", "rollout", "status", "daemonset/cilium", "--timeout=240s"])
    run(kube + ["wait", "--for=condition=Ready", "node/" + node, "--timeout=180s"])
    arch = run(docker + ["uname", "-m"], capture=True).decode().strip()
    if arch not in HASHES:
        raise SystemExit("unsupported node architecture")
    archive = directory / "gvisor.tar.bz2"
    url = f"https://storage.googleapis.com/gvisor/releases/release/{GVISOR}/{arch}/gvisor.tar.bz2"
    with urllib.request.urlopen(url, timeout=120) as response:
        archive.write_bytes(response.read())
    if hashlib.sha256(archive.read_bytes()).hexdigest() != HASHES[arch]:
        raise SystemExit("gVisor archive integrity mismatch")
    # Stream directly: Docker cp can write beneath a kind tmpfs mount on some
    # desktop engines. The verified archive already has runsc at its root.
    run(docker + ["tar", "-xf", "-", "-C", "/usr/local/bin"], data=bz2.decompress(archive.read_bytes()))
    run(docker + ["apt-get", "update", "-qq"])
    run(docker + ["apt-get", "install", "-y", "--no-install-recommends", "python3"])
    configuration = b'''from pathlib import Path
Path('/etc/containerd/runsc.toml').write_text('[runsc_config]\\n  platform = "systrap"\\n')
with Path('/etc/containerd/config.toml').open('a') as f:
 f.write('\\n[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc]\\n runtime_type = "io.containerd.runsc.v1"\\n [plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc.options]\\n  TypeUrl = "io.containerd.runsc.v1.options"\\n  ConfigPath = "/etc/containerd/runsc.toml"\\n')
Path('/opt/aidash').mkdir(exist_ok=True)
'''
    run(docker + ["python3", "-"], data=configuration)
    run(docker + ["systemctl", "restart", "containerd"])
    run(docker + ["python3", "-c", "from pathlib import Path; import sys; Path('/opt/aidash/node_guard.py').write_bytes(sys.stdin.buffer.read())"], data=(ROOT / "runner/node_guard.py").read_bytes())
    service = f"""[Unit]
Description=Aidash isolated runtime writer guardian
After=containerd.service
[Service]
Environment=AIDASH_CTR_BINARY={ctr}
ExecStart=/usr/bin/python3 /opt/aidash/node_guard.py watch
Restart=always
RestartSec=1
[Install]
WantedBy=multi-user.target
"""
    run(docker + ["python3", "-c", "from pathlib import Path; import sys; Path('/etc/systemd/system/aidash-node-guard.service').write_text(sys.stdin.read())"], data=service.encode())
    run(docker + ["systemctl", "daemon-reload"])
    run(docker + ["systemctl", "enable", "--now", "aidash-node-guard"])
    resources = {"apiVersion": "v1", "kind": "List", "items": [
        {"apiVersion": "node.k8s.io/v1", "kind": "RuntimeClass", "metadata": {"name": "aidash-gvisor"}, "handler": "runsc"},
        {"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": "aidash-sandbox", "labels": {
            "pod-security.kubernetes.io/enforce": "restricted", "pod-security.kubernetes.io/enforce-version": "v1.34"}}},
    ]}
    run(kube + ["apply", "-f", "-"], data=json.dumps(resources).encode())
    tag = "docker.io/library/aidash-sandbox:" + args.name
    metadata = directory / "image-metadata.json"
    run([executables["docker"], "buildx", "build", "--load", "--provenance=false", "--metadata-file", metadata,
         "--tag", tag, ROOT / "runner"])
    digest = json.loads(metadata.read_text())["containerimage.digest"]
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise SystemExit("invalid built image digest")
    run([executables["kind"], "load", "docker-image", "--name", args.name, tag])
    image = "docker.io/library/aidash-sandbox@" + digest
    run(docker + [ctr, "-n", "k8s.io", "images", "tag", tag, image])
    # Shared with the Rust capability runtime, whose Profile denies unknown fields:
    # Runner-only settings such as host_tasks stay out (the Runner defaults to 512).
    profile = {
        "admission": True, "storage": str(directory / "objects"), "outbound_origins": [], "package_origins": [],
        "working_bytes": 1073741824, "retained_bytes": 10737418240, "temporary_bytes": 268435456,
        "cpu": 2, "memory_bytes": 2147483648, "processes": 128,
        "operation_seconds": 120, "maximum_seconds": 600, "install_seconds": 300,
        "idle_seconds": args.idle_seconds, "recovery_seconds": 604800, "grant_seconds": 3600,
        "approval_seconds": 86400, "output_bytes": 8388608, "staging_seconds": 86400,
        "runner": {
            "endpoint": f"http://127.0.0.1:{args.port}", "token_env": "AIDASH_CORE_RUNNER_TOKEN", "image": image,
            "runtime_class": "aidash-gvisor", "namespace": "aidash-sandbox", "journal": str(directory / "journal"),
            "kubectl": kubectl, "kubeconfig": str(kubeconfig), "listen_host": "127.0.0.1", "listen_port": args.port,
            "node_guard": docker + ["env", "AIDASH_CTR_BINARY=" + ctr, "python3", "/opt/aidash/node_guard.py"],
        },
    }
    write_private(directory / "profile.json", json.dumps(profile, indent=2) + "\n")
    write_private(directory / "token", secrets.token_urlsafe(48) + "\n")
    print(json.dumps({"cluster": args.name, "profile": str(directory / "profile.json"), "image": image,
                      "next": f"python3 {ROOT / 'scripts/start-capability-runner.py'} {directory}"}, indent=2))


if __name__ == "__main__":
    main()
