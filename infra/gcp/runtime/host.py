#!/usr/bin/env python3
"""Trusted VM bootstrap and lifecycle adapter; never executes submitted tools."""

import argparse
import base64
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import tarfile
import time
import urllib.request

from policy import IMAGE_KINDS, idle_due, meaningful_request

ROOT = Path("/var/lib/aidash")
BUNDLE = Path(__file__).resolve().parent
RUN = Path("/run/aidash")
K3S_VERSION = "v1.34.11+k3s1"
K3S_SHA = "c1991a83985375d318560ac10f2def2fa117995d94d0319d801f283ca074d1b0"
GVISOR_VERSION = "20260921.0"
GVISOR_SHA = "3dd478770dd751d09c257ba14d739b179348a36c5f2d9e954b773f5f90bff646"


def command(*args, data=None, timeout=120, check=True):
    result = subprocess.run(
        [str(arg) for arg in args],
        input=data,
        capture_output=True,
        timeout=timeout,
        check=False,
    )
    if check and result.returncode:
        # Subprocess stderr can include sensitive arguments/URLs; keep it private.
        raise RuntimeError(f"{args[0]} failed with status {result.returncode}")
    return result.stdout


def private(path, value, mode=0o600):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".new")
    with temporary.open("wb") as file:
        os.fchmod(file.fileno(), mode)
        file.write(value.encode() if isinstance(value, str) else value)
        file.flush()
        os.fsync(file.fileno())
    temporary.replace(path)


def request(url, token=None, data=None, timeout=120):
    headers = (
        {"Authorization": "Bearer " + token} if token else {"Metadata-Flavor": "Google"}
    )
    with urllib.request.urlopen(
        urllib.request.Request(url, data=data, headers=headers), timeout=timeout
    ) as response:
        return response.read()


def cloud_token():
    return json.loads(
        request(
            "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token"
        )
    )["access_token"]


def checked_download(url, checksum):
    data = request(url)
    if hashlib.sha256(data).hexdigest() != checksum:
        raise RuntimeError("runtime download checksum mismatch")
    return data


def mount_disk(device, destination):
    for _ in range(60):
        if Path(device).exists():
            break
        time.sleep(1)
    filesystem = (
        command("blkid", "-o", "value", "-s", "TYPE", device, check=False)
        .decode()
        .strip()
    )
    if not filesystem:
        # Refuse any disk with existing signatures; only a new blank disk is formatted.
        signatures = json.loads(command("wipefs", "--json", device)).get(
            "signatures", []
        )
        if signatures:
            raise RuntimeError("unrecognized data disk; refusing to format")
        command("mkfs.ext4", "-F", device)
    elif filesystem != "ext4":
        raise RuntimeError("unexpected retained data filesystem")
    destination.mkdir(parents=True, exist_ok=True)
    if not os.path.ismount(destination):
        command("mount", device, destination)
    line = f"{device} {destination} ext4 defaults 0 2\n"
    fstab = Path("/etc/fstab")
    if line not in fstab.read_text():
        with fstab.open("a") as file:
            file.write(line)


def mount_data():
    mount_disk("/dev/disk/by-id/google-aidash-data", ROOT)
    for name in (
        "objects",
        "journal",
        "postgres",
        "nats",
        "k3s",
        "docker",
        "kubelet",
        "tls",
    ):
        (ROOT / name).mkdir(exist_ok=True, mode=0o700)
    os.chown(ROOT / "objects", 10001, 10001)
    # Persistent K3s state preserves journal-to-Pod identities across normal stops.
    for path, source in (
        ("/var/lib/rancher/k3s", ROOT / "k3s"),
        ("/var/lib/kubelet", ROOT / "kubelet"),
    ):
        destination = Path(path)
        if not destination.exists():
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.symlink_to(source, target_is_directory=True)


def runtime_file(path, data, mode=0o600):
    """Install atomically and report whether a service restart is needed."""
    path = Path(path)
    data = data.encode() if isinstance(data, str) else data
    if (
        path.is_file()
        and path.read_bytes() == data
        and path.stat().st_mode & 0o777 == mode
    ):
        return False
    private(path, data, mode)
    return True


def binary_matches(path, checksum):
    return (
        path.is_file()
        and path.stat().st_mode & 0o777 == 0o755
        and hashlib.sha256(path.read_bytes()).hexdigest() == checksum
    )


def configure_runtime():
    changed = False
    if not binary_matches(Path("/usr/local/bin/k3s"), K3S_SHA):
        private(
            "/usr/local/bin/k3s",
            checked_download(
                f"https://github.com/k3s-io/k3s/releases/download/{K3S_VERSION}/k3s",
                K3S_SHA,
            ),
            0o755,
        )
        changed = True
    receipt = Path("/usr/local/share/aidash/gvisor.json")
    installed = json.loads(receipt.read_text()) if receipt.exists() else {}
    files = installed.get("files", {})
    if not (
        installed.get("archive_sha") == GVISOR_SHA
        and {"runsc", "containerd-shim-runsc-v1"} <= files.keys()
        and all(
            not Path(name).is_absolute()
            and ".." not in Path(name).parts
            and binary_matches(Path("/usr/local/bin") / name, checksum)
            for name, checksum in files.items()
        )
    ):
        data = checked_download(
            f"https://storage.googleapis.com/gvisor/releases/release/{GVISOR_VERSION}/x86_64/gvisor.tar.bz2",
            GVISOR_SHA,
        )
        files = {}
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:bz2") as tar:
            for member in tar.getmembers():
                if Path(member.name).is_absolute() or ".." in Path(member.name).parts:
                    raise RuntimeError("invalid runtime archive path")
                if member.isdir():
                    continue
                if not member.isfile():
                    raise RuntimeError("runtime archive must contain regular files")
                content = tar.extractfile(member).read()
                private(Path("/usr/local/bin") / member.name, content, 0o755)
                files[member.name] = hashlib.sha256(content).hexdigest()
        if not {"runsc", "containerd-shim-runsc-v1"} <= files.keys():
            raise RuntimeError("incomplete runtime archive")
        private(receipt, json.dumps({"archive_sha": GVISOR_SHA, "files": files}))
        changed = True
    changed |= runtime_file(
        "/etc/rancher/k3s/config.yaml",
        """disable:
  - traefik
  - servicelb
  - metrics-server
write-kubeconfig-mode: '0600'
node-name: aidash
kubelet-arg:
  - pod-max-pids=128
  - container-log-max-size=10Mi
  - container-log-max-files=2
""",
    )
    changed |= runtime_file(
        ROOT / "k3s/agent/etc/containerd/config-v3.toml.tmpl",
        """{{ template "base" . }}
[plugins.'io.containerd.cri.v1.runtime'.containerd.runtimes.runsc]
runtime_type = "io.containerd.runsc.v1"
[plugins.'io.containerd.cri.v1.runtime'.containerd.runtimes.runsc.options]
TypeUrl = "io.containerd.runsc.v1.options"
ConfigPath = "/etc/containerd/runsc.toml"
""",
    )
    changed |= runtime_file(
        "/etc/containerd/runsc.toml",
        'root = "/run/containerd/runsc"\n[runsc_config]\nplatform = "systrap"\n',
    )
    # The existing node guard resolves ctr using a restricted PATH. Point it at
    # K3s's socket, not the unrelated Docker containerd socket.
    private("/usr/bin/ctr", '#!/bin/sh\nexec /usr/local/bin/k3s ctr "$@"\n', 0o755)
    private(
        "/usr/bin/kubectl", '#!/bin/sh\nexec /usr/local/bin/k3s kubectl "$@"\n', 0o755
    )
    if not Path("/usr/bin/runsc").exists():
        Path("/usr/bin/runsc").symlink_to("/usr/local/bin/runsc")
    changed |= runtime_file(
        "/etc/systemd/system/k3s.service",
        """[Unit]
Description=Aidash single-node Kubernetes
After=network-online.target
RequiresMountsFor=/var/lib/aidash
[Service]
Type=notify
ExecStart=/usr/local/bin/k3s server
KillMode=process
Delegate=yes
Restart=on-failure
LimitNOFILE=1048576
TasksMax=infinity
[Install]
WantedBy=multi-user.target
""",
    )
    command("systemctl", "daemon-reload")
    command("systemctl", "enable", "k3s")
    command("systemctl", "restart" if changed else "start", "k3s", timeout=240)
    command(
        "kubectl",
        "--kubeconfig",
        "/etc/rancher/k3s/k3s.yaml",
        "wait",
        "--for=condition=Ready",
        "node/aidash",
        "--timeout=180s",
        timeout=190,
    )
    resources = {
        "apiVersion": "v1",
        "kind": "List",
        "items": [
            {
                "apiVersion": "node.k8s.io/v1",
                "kind": "RuntimeClass",
                "metadata": {"name": "aidash-gvisor"},
                "handler": "runsc",
            },
            {
                "apiVersion": "v1",
                "kind": "Namespace",
                "metadata": {
                    "name": "aidash-sandbox",
                    "labels": {
                        "pod-security.kubernetes.io/enforce": "restricted",
                        "pod-security.kubernetes.io/enforce-version": "v1.34",
                    },
                },
            },
        ],
    }
    command(
        "kubectl",
        "--kubeconfig",
        "/etc/rancher/k3s/k3s.yaml",
        "apply",
        "-f",
        "-",
        data=json.dumps(resources).encode(),
    )


def environment_file(values):
    for key, value in values.items():
        if (
            not re.fullmatch(r"[A-Z][A-Z0-9_]*", key)
            or not isinstance(value, str)
            or any(c in value for c in "\r\n\x00")
        ):
            raise ValueError(
                "runtime configuration must contain single-line environment strings"
            )
    return "".join(f"{key}={value}\n" for key, value in sorted(values.items()))


def configuration(host):
    payload = json.loads(
        request(
            f"https://secretmanager.googleapis.com/v1/projects/{host['project']}/secrets/{host['secret']}/versions/latest:access",
            cloud_token(),
        )
    )
    external = json.loads(base64.b64decode(payload["payload"]["data"]))
    if not all(
        key.startswith("AIDASH_SECRET_")
        or key
        in {
            "AIDASH_OIDC_CLIENT_ID",
            "AIDASH_OIDC_CLIENT_SECRET",
            "AIDASH_OIDC_SESSION_ABSOLUTE_SECONDS",
            "AIDASH_OIDC_SESSION_IDLE_SECONDS",
        }
        for key in external
    ):
        raise ValueError(
            "runtime secret may contain only Google client/session and provider credential configuration"
        )
    if not external.get("AIDASH_OIDC_CLIENT_ID") or not external.get(
        "AIDASH_OIDC_CLIENT_SECRET"
    ):
        raise ValueError("Google OAuth client configuration is required")
    path = ROOT / "identity.json"
    if not path.exists():
        private(
            path,
            json.dumps(
                {
                    name: secrets.token_hex(32)
                    for name in ("database", "api", "runner")
                }
            ),
        )
    identity = json.loads(path.read_text())
    result = dict(
        external,
        DATABASE_URL=f"postgres://aidash:{identity['database']}@127.0.0.1:5432/aidash_a",
        NATS_URL="nats://127.0.0.1:4222",
        AIDASH_API_TOKEN=identity["api"],
        AIDASH_NODE_ID="aidash://" + host["secret"],
        AIDASH_ENDPOINT="https://" + host["hostname"],
        AIDASH_LISTEN="127.0.0.1:18080",
        AIDASH_PROBE_LISTEN="127.0.0.1:18081",
        AIDASH_AUTH_TRUSTED_PROXY_IPS="127.0.0.1",
        AIDASH_OIDC_ISSUER="https://accounts.google.com",
        AIDASH_OIDC_PUBLIC_ORIGIN="https://" + host["hostname"],
        AIDASH_CAPABILITY_PROFILE=str(ROOT / "profile.json"),
        AIDASH_CORE_RUNNER_TOKEN=identity["runner"],
    )
    private(RUN / "app.env", environment_file(result))
    private(
        RUN / "observer.env", environment_file({"DATABASE_URL": result["DATABASE_URL"]})
    )
    private(
        RUN / "postgres.env",
        environment_file(
            {
                "POSTGRES_USER": "aidash",
                "POSTGRES_DB": "aidash_a",
                "POSTGRES_PASSWORD": identity["database"],
            }
        ),
    )
    private(
        RUN / "runner.env",
        environment_file(
            {
                "AIDASH_CORE_RUNNER_TOKEN": identity["runner"],
                "AIDASH_CAPABILITY_PROFILE": str(ROOT / "profile.json"),
            }
        ),
    )


def replace_container(name, image, options, arguments, restart="unless-stopped"):
    command("docker", "rm", "-f", name, check=False)
    command(
        "docker",
        "run",
        "-d",
        "--name",
        name,
        "--restart",
        restart,
        "--log-opt",
        "max-size=10m",
        "--log-opt",
        "max-file=2",
        "--network",
        "host",
        *options,
        image,
        *arguments,
    )


def install():
    RUN.mkdir(parents=True, exist_ok=True, mode=0o700)
    private(RUN / "draining", "bootstrap\n", 0o644)
    (RUN / "serving").unlink(missing_ok=True)
    mount_data()
    host = json.loads((BUNDLE / "host.json").read_text())
    # Stop Caddy before changing its store, including on a resumed retained VM.
    command("systemctl", "stop", "caddy", check=False)
    if host.get("preview"):
        mount_disk("/dev/disk/by-id/google-aidash-preview-tls", ROOT / "tls")
    command("apt-get", "update", "-qq", timeout=240)
    command(
        "apt-get",
        "install",
        "-y",
        "--no-install-recommends",
        "ca-certificates",
        "docker.io",
        "nginx",
        "libnginx-mod-http-lua",
        "lua-cjson",
        "caddy",
        timeout=600,
    )
    private(
        "/etc/docker/daemon.json",
        json.dumps(
            {
                "data-root": str(ROOT / "docker"),
                "log-driver": "json-file",
                "log-opts": {"max-size": "10m", "max-file": "2"},
            }
        ),
    )
    # Keep application admission frozen across daemon/dependency restarts. A
    # resumed VM must install/migrate before the previous app can start workers.
    command("docker", "rm", "-f", "aidash-app", check=False)
    command("systemctl", "kill", "--signal=SIGCONT", "aidash-runner", check=False)
    command("systemctl", "stop", "aidash-runner", check=False)
    command("systemctl", "restart", "docker")
    release = json.loads((BUNDLE / "release.json").read_text())
    configuration(host)
    configure_runtime()
    registry = release["images"]["app"].split("/", 1)[0]
    command(
        "docker",
        "login",
        registry,
        "-u",
        "oauth2accesstoken",
        "--password-stdin",
        data=cloud_token().encode(),
    )
    for image in release["images"].values():
        command("docker", "pull", image, timeout=600)
    # Import a verified local image without giving long-lived registry credentials to Kubernetes.
    archive = RUN / "sandbox.tar"
    command("docker", "tag", release["images"]["sandbox"], "aidash-sandbox:loaded")
    command("docker", "save", "-o", archive, "aidash-sandbox:loaded", timeout=180)
    command("ctr", "-n", "k8s.io", "images", "import", archive, timeout=180)
    command(
        "ctr",
        "-n",
        "k8s.io",
        "images",
        "tag",
        "--force",
        "docker.io/library/aidash-sandbox:loaded",
        release["images"]["sandbox"],
    )
    archive.unlink()
    command("docker", "logout", registry)
    profile = {
        "admission": True,
        "storage": str(ROOT / "objects"),
        "outbound_origins": [],
        "package_origins": [],
        "working_bytes": 1073741824,
        "retained_bytes": 10737418240,
        "temporary_bytes": 268435456,
        "cpu": 1,
        "memory_bytes": 2147483648,
        "processes": 128,
        "operation_seconds": 120,
        "maximum_seconds": 600,
        "install_seconds": 300,
        "idle_seconds": 1800,
        "recovery_seconds": 604800,
        "grant_seconds": 3600,
        "approval_seconds": 86400,
        "output_bytes": 8388608,
        "staging_seconds": 86400,
        "runner": {
            "endpoint": "http://127.0.0.1:18949",
            "token_env": "AIDASH_CORE_RUNNER_TOKEN",
            "image": release["images"]["sandbox"],
            "runtime_class": "aidash-gvisor",
            "namespace": "aidash-sandbox",
            "journal": str(ROOT / "journal"),
            "kubectl": "/usr/bin/kubectl",
            "kubeconfig": "/etc/rancher/k3s/k3s.yaml",
            "listen_host": "127.0.0.1",
            "listen_port": 18949,
            "node_guard": ["/usr/bin/python3", str(BUNDLE / "node_guard.py")],
        },
    }
    private(ROOT / "profile.json", json.dumps(profile), 0o644)
    for name, arguments in (
        ("runner", "control.py"),
        ("node-guard", "node_guard.py watch"),
    ):
        private(
            f"/etc/systemd/system/aidash-{name}.service",
            f"""[Unit]
Description=Aidash trusted {name}
After=k3s.service
Requires=k3s.service
RequiresMountsFor=/var/lib/aidash
[Service]
EnvironmentFile={RUN}/runner.env
ExecStart=/usr/bin/python3 {BUNDLE}/{arguments}
Restart=on-failure
RestartSec=5
TimeoutStopSec=30
[Install]
WantedBy=multi-user.target
""",
        )
    command("systemctl", "daemon-reload")
    command("systemctl", "kill", "--signal=SIGCONT", "aidash-runner", check=False)
    command("systemctl", "enable", "aidash-node-guard", "aidash-runner")
    command("systemctl", "restart", "aidash-node-guard", "aidash-runner")
    replace_container(
        "aidash-postgres",
        release["images"]["postgres"],
        [
            "--env-file",
            str(RUN / "postgres.env"),
            "-v",
            f"{ROOT}/postgres:/var/lib/postgresql/data",
        ],
        ["-c", "listen_addresses=127.0.0.1"],
    )
    replace_container(
        "aidash-nats",
        release["images"]["nats"],
        ["-v", f"{ROOT}/nats:/data"],
        ["-js", "-sd", "/data", "--addr", "127.0.0.1"],
    )
    for _ in range(90):
        if b"accepting connections" in command(
            "docker",
            "exec",
            "aidash-postgres",
            "pg_isready",
            "-U",
            "aidash",
            check=False,
        ):
            break
        time.sleep(2)
    mount = [
        "--env-file",
        str(RUN / "app.env"),
        "-v",
        f"{ROOT}/objects:{ROOT}/objects",
        "-v",
        f"{ROOT}/profile.json:{ROOT}/profile.json:ro",
    ]
    # Migrate explicitly; a failed migration never drops/recreates a retained database.
    command(
        "docker",
        "run",
        "--rm",
        "--network",
        "host",
        *mount,
        release["images"]["app"],
        "migrate",
        timeout=300,
    )
    replace_container(
        "aidash-app", release["images"]["app"], mount, ["serve"], restart="on-failure:3"
    )
    private(ROOT / "release.json", json.dumps(release))
    private(
        "/etc/nginx/conf.d/aidash.conf",
        (BUNDLE / "nginx.conf").read_text(),
        0o644,
    )
    Path("/etc/nginx/sites-enabled/default").unlink(missing_ok=True)
    private(
        "/etc/caddy/Caddyfile",
        f"{host['hostname']} {{\n  reverse_proxy 127.0.0.1:8088\n}}\n",
        0o644,
    )
    # Previews share this store across PRs; other hosts retain it with their data.
    # The Caddy uid can differ between images used by successive previews.
    command("chown", "-R", "caddy:caddy", ROOT / "tls")
    private(
        "/etc/systemd/system/caddy.service.d/aidash.conf",
        "[Unit]\nRequiresMountsFor=/var/lib/aidash/tls\n[Service]\nEnvironment=XDG_DATA_HOME=/var/lib/aidash/tls\nReadWritePaths=/var/lib/aidash/tls\n",
        0o644,
    )
    private(
        "/etc/systemd/system/aidash-observe.service",
        f"[Unit]\nAfter=docker.service\n[Service]\nType=oneshot\nExecStart=/usr/bin/python3 {BUNDLE}/host.py observe\n",
    )
    private(
        "/etc/systemd/system/aidash-observe.timer",
        "[Unit]\nDescription=Observe Aidash activity\n[Timer]\nOnBootSec=60\nOnUnitActiveSec=60\n[Install]\nWantedBy=timers.target\n",
    )
    command("systemctl", "daemon-reload")
    command("nginx", "-t")
    command("systemctl", "restart", "nginx", "caddy")
    command("systemctl", "enable", "--now", "aidash-observe.timer")
    private(RUN / "ready", release["source_sha"])
    keepalive()


def keepalive():
    private(ROOT / "last-active", str(time.time()))


def snapshot():
    release = json.loads((ROOT / "release.json").read_text())
    result = json.loads(
        command(
            "docker",
            "run",
            "--rm",
            "--network",
            "host",
            "--env-file",
            RUN / "observer.env",
            release["images"]["observer"],
            timeout=20,
        )
    )
    # Access-phase accounting also covers requests which have not committed a
    # database row yet (for example a slow upload). This endpoint is loopback
    # only on a separate port, never routed by the public proxy.
    activity = json.loads(request("http://127.0.0.1:8089/activity"))
    result["counts"]["http_requests"] = int(activity["inflight"])
    result["last_request_completed"] = float(activity["last_active"])
    if activity["inflight"] != 0:
        result["busy"] = True
    for path in (ROOT / "journal").glob("*.json"):
        if path.name.startswith(("area-", "session-")):
            continue
        value = json.loads(path.read_text())
        terminal = value["status"] in {"completed", "failed", "cancelled", "uncertain"}
        safe_writer = value.get("termination_confirmed") or value.get("writer_frozen")
        if not terminal or not safe_writer:
            result["busy"] = True
            result["counts"]["runner"] = result["counts"].get("runner", 0) + 1
    return result


def observe():
    result = snapshot()
    now = time.time()
    last = (
        float((ROOT / "last-active").read_text())
        if (ROOT / "last-active").exists()
        else now
    )
    last = max(
        last,
        result.get("last_request_completed", 0),
        result.get("last_work_completed", 0),
    )
    log = Path("/var/log/nginx/aidash-activity.log")
    # Bounded log rotation is configured by the distribution. Process only the
    # current log; last-active persists across rotations and host restarts.
    if log.exists():
        with log.open() as stream:
            for line in stream:
                fields = line.split()
                if len(fields) == 4 and meaningful_request(
                    fields[1], fields[2], int(fields[3])
                ):
                    last = max(last, float(fields[0]))
    # Periodic idle database bookkeeping isn't user/Agent work. It is checked
    # separately after pausing application admission during sealing.
    active = any(
        count for key, count in result["counts"].items() if key != "database_work"
    )
    previous = (
        json.loads((ROOT / "activity.json").read_text())
        if (ROOT / "activity.json").exists()
        else {}
    )
    # Transfers have durable receipts but no database completion timestamp.
    # First observation is conservative: never shorten the post-transfer hour.
    if set(result["completed_transfers"]) - set(
        previous.get("completed_transfers", [])
    ):
        result["last_work_completed"] = max(result.get("last_work_completed", 0), now)
        last = max(last, result["last_work_completed"])
    # Completion starts a fresh hour. An observation gap is unknown activity,
    # so it also starts a new interval rather than causing immediate shutdown.
    if active or previous.get("busy") or now - previous.get("observed_at", 0) > 120:
        last = now
    result.update(busy=active, observed_at=now, last_active=last)
    private(ROOT / "last-active", str(last))
    private(ROOT / "activity.json", json.dumps(result))
    return result


def unseal():
    command("systemctl", "kill", "--signal=SIGCONT", "aidash-runner", check=False)
    command("docker", "unpause", "aidash-app", check=False)
    request("http://127.0.0.1:8089/admission/open", data=b"", timeout=10)
    (RUN / "draining").unlink(missing_ok=True)
    (RUN / "sealed").unlink(missing_ok=True)
    private(RUN / "serving", "ready\n", 0o644)


def gate():
    private(RUN / "draining", "failed deployment\n", 0o644)
    (RUN / "serving").unlink(missing_ok=True)
    request("http://127.0.0.1:8089/admission/close", data=b"", timeout=10)


def seal(force=False, idle_only=False):
    try:
        gate()
        state = json.loads(
            command("docker", "inspect", "--format={{json .State}}", "aidash-app")
        )
        if not state["Running"]:
            raise RuntimeError("application admission state unavailable")
        if not state["Paused"]:
            command("docker", "pause", "aidash-app")
        command("systemctl", "kill", "--signal=SIGSTOP", "aidash-runner")
        result = snapshot()
        if result["busy"] and not force:
            unseal()
            return {"sealed": False, "reason": "active_work"}
        if idle_only and not idle_due(observe(), time.time()):
            unseal()
            return {"sealed": False, "reason": "new_activity"}
    except Exception:
        unseal()
        raise
    private(RUN / "sealed", str(time.time()))
    return {"sealed": True}


def prune_release_images(release):
    current = set(release["images"].values())
    registry = release["images"]["app"].rsplit("/", 1)[0]

    def superseded(image, kinds):
        return image not in current and any(
            re.fullmatch(
                re.escape(f"{registry}/{kind}@sha256:") + r"[a-f0-9]{64}", image
            )
            for kind in kinds
        )

    images = (
        command(
            "docker",
            "image",
            "ls",
            "--digests",
            "--format",
            "{{.Repository}}@{{.Digest}}",
        )
        .decode()
        .splitlines()
    )
    for image in sorted(set(images)):
        if superseded(image, IMAGE_KINDS):
            # No force: Docker retains images still referenced by a container.
            command("docker", "image", "rm", image, check=False)
    pods = json.loads(
        command(
            "kubectl",
            "--kubeconfig",
            "/etc/rancher/k3s/k3s.yaml",
            "get",
            "pods",
            "--all-namespaces",
            "-o",
            "json",
        )
    )
    in_use = {
        container["image"]
        for pod in pods["items"]
        for kind in ("containers", "initContainers", "ephemeralContainers")
        for container in pod["spec"].get(kind, [])
    }
    images = (
        command("ctr", "-n", "k8s.io", "images", "list", "-q").decode().splitlines()
    )
    for image in images:
        if image not in in_use and superseded(image, ("sandbox",)):
            command("ctr", "-n", "k8s.io", "images", "rm", image, check=False)


def health():
    release = json.loads((ROOT / "release.json").read_text())
    profile = json.loads((ROOT / "profile.json").read_text())
    token = json.loads((ROOT / "identity.json").read_text())["runner"]
    runner = json.loads(request(profile["runner"]["endpoint"] + "/v1/health", token))
    if not runner["verified"] or not runner["python_verified"]:
        raise RuntimeError("sandbox readiness failed")
    request("http://127.0.0.1:18081/ready")
    # Missing/incompatible observer schema blocks readiness, rather than silently
    # deploying an environment that can never meet the idle-stop contract.
    observe()
    # Collect only obsolete Aidash digests after application, observer and
    # Runner readiness succeeded. Keep current observer/sandbox images even
    # when no container uses them yet; live Pods retain their pinned images.
    prune_release_images(release)
    return {"ready": True, "source_sha": release["source_sha"], "runner_verified": True}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "action",
        choices=["install", "observe", "health", "seal", "unseal", "keepalive", "gate"],
    )
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--idle-only", action="store_true")
    args = parser.parse_args()
    RUN.mkdir(parents=True, exist_ok=True, mode=0o755)
    with (RUN / "lifecycle.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if args.action == "seal":
            result = seal(args.force, args.idle_only)
        else:
            result = globals()[args.action]()
        print(json.dumps(result or {"ok": True}))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(
            json.dumps(
                {
                    "error": type(error).__name__,
                    "message": "host operation failed; inspect protected local service diagnostics",
                }
            ),
            file=sys.stderr,
        )
        sys.exit(1)
