"""Start and health-check the Compose development stack before watching it."""

from __future__ import annotations

import os
from pathlib import Path
import shlex
import shutil
import socket
import subprocess
import sys
from urllib.parse import urlsplit, urlunsplit


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BACKEND_PORT = 18080
DEFAULT_FRONTEND_PORT = 5173


def dotenv_values() -> dict[str, str]:
    values: dict[str, str] = {}
    path = ROOT / ".env"
    if not path.is_file():
        return values

    for line_number, line in enumerate(path.read_text().splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("export "):
            line = line[7:].strip()
        key, separator, raw = line.partition("=")
        key = key.strip()
        if not separator or not key.isidentifier():
            raise SystemExit(f"Invalid .env entry at line {line_number}")
        try:
            parsed = shlex.split(raw, comments=True)
        except ValueError as error:
            raise SystemExit(f"Invalid .env value at line {line_number}: {error}") from error
        if len(parsed) > 1:
            raise SystemExit(
                f"Invalid .env value at line {line_number}; quote values containing spaces"
            )
        values[key] = parsed[0] if parsed else ""
    return values


def selected_port(
    name: str,
    default: int,
    inherited: dict[str, str],
    dotenv: dict[str, str],
) -> int:
    configured = inherited[name] if name in inherited else dotenv.get(name, "")
    explicit = bool(configured)
    try:
        port = int(configured) if explicit else default
    except ValueError as error:
        raise SystemExit(f"{name} must be a port number from 1 to 65535") from error
    if not 1 <= port <= 65535:
        raise SystemExit(f"{name} must be a port number from 1 to 65535")

    while True:
        try:
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", port))
            return port
        except OSError as error:
            if explicit:
                raise SystemExit(
                    f"127.0.0.1:{port} is already in use; choose another {name}"
                ) from error
            if port == 65535:
                raise SystemExit(f"No available host port found for {name}") from error
            port += 1


def local_endpoint(endpoint: str, backend_port: int) -> str:
    if not endpoint:
        return f"http://127.0.0.1:{backend_port}"
    try:
        parsed = urlsplit(endpoint)
        if parsed.hostname not in {"127.0.0.1", "localhost", "::1"}:
            return endpoint
        return urlunsplit(
            (
                parsed.scheme or "http",
                f"127.0.0.1:{backend_port}",
                parsed.path,
                parsed.query,
                parsed.fragment,
            )
        )
    except ValueError:
        return endpoint


def run(*args: str, env: dict[str, str] | None = None) -> None:
    subprocess.run(args, cwd=ROOT, env=env, check=True)


def start() -> None:
    inherited = dict(os.environ)
    dotenv = dotenv_values()
    environment = inherited.copy()
    backend_port = selected_port(
        "AIDASH_BACKEND_PORT", DEFAULT_BACKEND_PORT, inherited, dotenv
    )
    frontend_port = selected_port(
        "AIDASH_FRONTEND_PORT", DEFAULT_FRONTEND_PORT, inherited, dotenv
    )
    environment["AIDASH_BACKEND_PORT"] = str(backend_port)
    environment["AIDASH_FRONTEND_PORT"] = str(frontend_port)
    endpoint = (
        inherited["AIDASH_ENDPOINT"]
        if "AIDASH_ENDPOINT" in inherited
        else dotenv.get("AIDASH_ENDPOINT", "")
    )
    environment["AIDASH_ENDPOINT"] = local_endpoint(endpoint, backend_port)

    for executable in ("docker", "curl"):
        if shutil.which(executable) is None:
            raise SystemExit(f"Missing required tool: {executable}")
    run("docker", "compose", "version")

    compose = ("docker", "compose", "--profile", "dev")
    run(
        *compose, "up", "--build", "--detach", "--wait", "--wait-timeout", "300",
        env=environment,
    )
    run(
        "curl", "--fail", "--silent", "--show-error", "--max-time", "5",
        "--output", os.devnull,
        f"http://127.0.0.1:{backend_port}/health", env=environment,
    )
    run(
        "curl", "--fail", "--silent", "--show-error", "--max-time", "5",
        "--output", os.devnull,
        f"http://127.0.0.1:{frontend_port}/src/main.tsx", env=environment,
    )
    print(f"Frontend: http://127.0.0.1:{frontend_port}", flush=True)
    print(f"Backend:  http://127.0.0.1:{backend_port}", flush=True)
    os.execvpe("docker", [*compose, "up", "--watch"], environment)


def main() -> None:
    if len(sys.argv) != 2 or sys.argv[1] not in {"up", "down"}:
        raise SystemExit("Usage: local_dev.py up|down")
    if sys.argv[1] == "down":
        if shutil.which("docker") is None:
            raise SystemExit("Missing required tool: docker")
        run("docker", "compose", "--profile", "dev", "down")
    else:
        start()


if __name__ == "__main__":
    main()
