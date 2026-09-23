"""Handle the state and Secrets that kind, kubectl, and Helm cannot express as flags.

The build and rollout sequence lives in Makefile.toml. This module only owns
the dedicated cluster's local state and the Secret values derived from .env.
"""

import base64
import json
import os
from pathlib import Path
import shlex
import shutil
import socket
import subprocess
import sys
import time
from urllib.error import URLError
from urllib.request import urlopen
from urllib.parse import quote


ROOT = Path(__file__).resolve().parent.parent
STATE = ROOT / ".ignore/local-k8s"
KUBECONFIG = STATE / "kubeconfig"
CLUSTER = "aidash-local"
NAMESPACE = "aidash-local"


def fail(message: str) -> None:
    raise SystemExit(message)


def run(*args: str, input: str | None = None, capture: bool = False) -> str:
    result = subprocess.run(
        args, input=input, text=True, check=True, cwd=ROOT,
        capture_output=capture,
    )
    return result.stdout if capture else ""


def kubectl(*args: str, input: str | None = None, capture: bool = False) -> str:
    return run("kubectl", "--kubeconfig", str(KUBECONFIG), *args, input=input, capture=capture)


def cluster_exists() -> bool:
    return CLUSTER in run("kind", "get", "clusters", capture=True).splitlines()


def environment() -> dict[str, str]:
    values = dict(os.environ)
    dotenv = ROOT / ".env"
    if dotenv.exists():
        for number, line in enumerate(dotenv.read_text().splitlines(), 1):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("export "):
                line = line[7:].strip()
            key, separator, raw = line.partition("=")
            key = key.strip()
            if not separator or not key.isidentifier():
                fail(f"Invalid .env entry at line {number}")
            try:
                parsed = shlex.split(raw, comments=False)
            except ValueError as error:
                fail(f"Invalid .env entry at line {number}: {error}")
            if len(parsed) > 1:
                fail(f"Invalid .env value at line {number}; quote values containing spaces")
            values.setdefault(key, parsed[0] if parsed else "")
    return values


def port_number(value: str) -> int:
    try:
        port = int(value)
    except ValueError:
        fail("AIDASH_K8S_PORT must be a port number from 1 to 65535")
    if not 1 <= port <= 65535:
        fail("AIDASH_K8S_PORT must be a port number from 1 to 65535")
    return port


def apply_secret(name: str, values: dict[str, str]) -> None:
    current = kubectl("-n", NAMESPACE, "get", "secret", name, "-o", "json",
                      "--ignore-not-found", capture=True)
    metadata = {"name": name}
    if current:
        metadata["resourceVersion"] = json.loads(current)["metadata"]["resourceVersion"]
    secret = {
        "apiVersion": "v1", "kind": "Secret", "metadata": metadata,
        "type": "Opaque",
        "data": {key: base64.b64encode(value.encode()).decode()
                 for key, value in values.items()},
    }
    kubectl("-n", NAMESPACE, "replace" if current else "create", "-f", "-",
            input=json.dumps(secret))


def application_values(values: dict[str, str], password: str, token: str,
                       qdrant_key: str, port: int) -> dict[str, str]:
    app_values = {
        "DATABASE_URL": f"postgres://aidash:{quote(password, safe='')}@postgres:5432/aidash_a",
        "NATS_URL": "nats://nats:4222",
        "AIDASH_API_TOKEN": token,
    }
    app_values.update({key: value for key, value in values.items()
                       if key.startswith("AIDASH_SECRET_")})
    app_values.update({key: values[key] for key in ("AIDASH_JEV_ENDPOINT", "AIDASH_JEV_MODEL")
                       if key in values})
    app_values.update({key: value for key, value in values.items()
                       if key.startswith("AIDASH_OIDC_") and value})
    if any(key in app_values for key in (
            "AIDASH_OIDC_ISSUER", "AIDASH_OIDC_CLIENT_ID", "AIDASH_OIDC_CLIENT_SECRET")):
        app_values["AIDASH_OIDC_PUBLIC_ORIGIN"] = f"http://127.0.0.1:{port}"
    app_values["AIDASH_SECRET_TEST_QDRANT"] = qdrant_key
    return app_values


def prepare() -> None:
    for tool in ("kind", "kubectl", "docker"):
        if shutil.which(tool) is None:
            fail(f"Missing required tool: {tool}")
    run("docker", "info", capture=True)
    STATE.mkdir(parents=True, exist_ok=True)
    STATE.chmod(0o700)
    values = environment()
    token = values.get("AIDASH_API_TOKEN") or "local-development-token"
    password = values.get("AIDASH_LOCAL_POSTGRES_PASSWORD") or "aidash-local"
    qdrant_key = values.get("AIDASH_SECRET_TEST_QDRANT") or "local-semantic-vector-fixture-key-0123456789"
    if len(token) < 16:
        fail("AIDASH_API_TOKEN must be at least 16 characters")

    if cluster_exists():
        port_file = STATE / "port"
        if not port_file.is_file() or not port_file.read_text().strip():
            fail(f"Cluster exists without {port_file}; run k8s-down before upgrading")
        port = port_number(port_file.read_text().strip())
        if values.get("AIDASH_K8S_PORT") and port_number(values["AIDASH_K8S_PORT"]) != port:
            fail(f"Cluster uses port {port}; run k8s-down before changing AIDASH_K8S_PORT")
        if not KUBECONFIG.is_file() or not KUBECONFIG.stat().st_size:
            run("kind", "export", "kubeconfig", "--name", CLUSTER,
                "--kubeconfig", str(KUBECONFIG))
    else:
        port = port_number(values.get("AIDASH_K8S_PORT") or "8080")
        try:
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", port))
        except OSError as error:
            fail(f"Cannot bind 127.0.0.1:{port}: {error}")
        config = STATE / "kind.yaml"
        config.write_text(
            "kind: Cluster\napiVersion: kind.x-k8s.io/v1alpha4\nnodes:\n"
            "  - role: control-plane\n    extraPortMappings:\n"
            f"      - containerPort: 30080\n        hostPort: {port}\n"
            "        listenAddress: 127.0.0.1\n        protocol: TCP\n"
        )
        run("kind", "create", "cluster", "--name", CLUSTER,
            "--kubeconfig", str(KUBECONFIG), "--config", str(config), "--wait", "180s")
        (STATE / "port").write_text(f"{port}\n")

    kubectl("cluster-info", capture=True)
    if not kubectl("get", "namespace", NAMESPACE, "-o", "name",
                   "--ignore-not-found", capture=True).strip():
        kubectl("create", "namespace", NAMESPACE)

    pvc = kubectl("-n", NAMESPACE, "get", "pvc", "data-postgres-0", "-o", "name",
                  "--ignore-not-found", capture=True)
    if pvc.strip():
        stored = json.loads(kubectl("-n", NAMESPACE, "get", "secret",
                                    "aidash-local-infra", "-o", "json", capture=True))
        old_password = base64.b64decode(stored["data"]["POSTGRES_PASSWORD"]).decode()
        if old_password != password:
            fail("PostgreSQL password differs from the persisted database; restore the original "
                 "AIDASH_LOCAL_POSTGRES_PASSWORD or run k8s-down to discard local data")

    app_values = application_values(values, password, token, qdrant_key, port)
    apply_secret("aidash-local-infra", {"POSTGRES_PASSWORD": password,
                                        "QDRANT_API_KEY": qdrant_key})
    apply_secret("aidash-local-app", app_values)


def health() -> None:
    port = port_number((STATE / "port").read_text().strip())
    url = f"http://127.0.0.1:{port}/health"
    for _ in range(30):
        try:
            with urlopen(url, timeout=2) as response:
                if response.status == 200:
                    print(f"Aidash: http://127.0.0.1:{port}")
                    print("Configure Keycloak OIDC to sign in to the dashboard; AIDASH_API_TOKEN remains available for API recovery.")
                    return
        except (OSError, URLError):
            pass
        time.sleep(1)
    fail(f"Aidash health check failed at {url}")


def status() -> None:
    if not cluster_exists() or not KUBECONFIG.is_file() or not KUBECONFIG.stat().st_size:
        fail("Aidash local kind cluster is not running")
    kubectl("-n", NAMESPACE, "get", "pods,services")
    port = port_number((STATE / "port").read_text().strip())
    print(f"Local access: http://127.0.0.1:{port}")


def down() -> None:
    if cluster_exists():
        run("kind", "delete", "cluster", "--name", CLUSTER)
    for name in ("kubeconfig", "port", "kind.yaml", "forward.pid", "forward.port", "forward.log"):
        (STATE / name).unlink(missing_ok=True)


if __name__ == "__main__":
    actions = {"prepare": prepare, "health": health, "status": status, "down": down}
    if len(sys.argv) != 2 or sys.argv[1] not in actions:
        fail("Usage: local_k8s.py prepare|health|status|down")
    actions[sys.argv[1]]()
