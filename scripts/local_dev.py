"""Start and health-check the Compose development stack before watching it."""

from __future__ import annotations

import os
from pathlib import Path
import shlex
import signal
import shutil
import socket
import subprocess
import sys
import time
from urllib.parse import urlsplit, urlunsplit


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BACKEND_PORT = 18080
DEFAULT_FRONTEND_PORT = 5173
WATCH_STATE_DIR = ROOT / ".ignore" / "local-dev"
WATCH_PID_PATH = WATCH_STATE_DIR / "watch.pid"
WATCH_LOG_PATH = WATCH_STATE_DIR / "watch.log"


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


def watch_process_command(pid: int) -> str | None:
    result = subprocess.run(
        ("ps", "-p", str(pid), "-o", "command="),
        check=False,
        capture_output=True,
        text=True,
    )
    command = result.stdout.strip()
    return command or None


def is_watch_process(pid: int) -> bool:
    command = watch_process_command(pid)
    if command is None:
        return False
    return command.endswith(f"{Path(__file__).resolve()} watch")


def stop_watch() -> None:
    if not WATCH_PID_PATH.is_file():
        return

    try:
        pid = int(WATCH_PID_PATH.read_text().strip())
    except (OSError, ValueError):
        WATCH_PID_PATH.unlink(missing_ok=True)
        return

    if not is_watch_process(pid):
        WATCH_PID_PATH.unlink(missing_ok=True)
        return

    try:
        os.killpg(pid, signal.SIGTERM)
    except ProcessLookupError:
        WATCH_PID_PATH.unlink(missing_ok=True)
        return

    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and is_watch_process(pid):
        time.sleep(0.1)

    if is_watch_process(pid):
        try:
            os.killpg(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    WATCH_PID_PATH.unlink(missing_ok=True)


def start_watch(environment: dict[str, str]) -> None:
    stop_watch()
    WATCH_STATE_DIR.mkdir(parents=True, exist_ok=True)

    with WATCH_LOG_PATH.open("wb") as log_file:
        process = subprocess.Popen(
            (sys.executable, str(Path(__file__).resolve()), "watch"),
            cwd=ROOT,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=log_file,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )

    time.sleep(0.2)
    if process.poll() is not None:
        raise SystemExit(
            f"Compose Watch failed to start; see {WATCH_LOG_PATH}"
        )

    WATCH_PID_PATH.write_text(f"{process.pid}\n")


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
    start_watch(environment)
    print(f"Frontend: http://127.0.0.1:{frontend_port}", flush=True)
    print(f"Backend:  http://127.0.0.1:{backend_port}", flush=True)
    print("Container logs: cargo make dev-logs", flush=True)
    print(f"Compose Watch output: tail -f {WATCH_LOG_PATH}", flush=True)


def watch() -> None:
    run("docker", "compose", "--profile", "dev", "watch", "--no-up")


def main() -> None:
    if len(sys.argv) != 2 or sys.argv[1] not in {"up", "down", "watch"}:
        raise SystemExit("Usage: local_dev.py up|down|watch")
    if sys.argv[1] == "down":
        stop_watch()
        if shutil.which("docker") is None:
            raise SystemExit("Missing required tool: docker")
        run("docker", "compose", "--profile", "dev", "down")
    elif sys.argv[1] == "watch":
        watch()
    else:
        start()


if __name__ == "__main__":
    main()
