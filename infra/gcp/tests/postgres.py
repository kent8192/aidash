#!/usr/bin/env python3
"""Run observer integration checks against a fresh local-only PostgreSQL container."""

import os
from pathlib import Path
import secrets
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "infra/gcp/control"))
from kube import OBSERVER_ROLE, observer_sql  # noqa: E402


def main():
    container = subprocess.check_output(
        [
            "docker",
            "run",
            "--detach",
            "--rm",
            "--label",
            "aidash-purpose=infra-observer-test",
            "--env",
            "POSTGRES_HOST_AUTH_METHOD=trust",
            "--env",
            "POSTGRES_DB=aidash_observer_test",
            "--publish",
            "127.0.0.1::5432",
            "postgres:17-bookworm",
        ],
        text=True,
    ).strip()
    try:
        for _ in range(50):
            # The image's temporary initialization server only accepts Unix
            # sockets. Wait for the TCP listener used by the observer instead.
            probe = subprocess.run(
                [
                    "docker",
                    "exec",
                    container,
                    "pg_isready",
                    "-h",
                    "127.0.0.1",
                    "-U",
                    "postgres",
                    "-d",
                    "aidash_observer_test",
                ],
                capture_output=True,
            )
            if probe.returncode == 0:
                break
            time.sleep(1)
        else:
            raise RuntimeError("disposable PostgreSQL did not become ready")
        port = (
            subprocess.check_output(
                ["docker", "port", container, "5432/tcp"], text=True
            )
            .strip()
            .rsplit(":", 1)[1]
        )
        password = secrets.token_hex(32)
        environment = dict(
            os.environ,
            AIDASH_OBSERVER_TEST_DATABASE_URL=f"postgres://postgres@127.0.0.1:{port}/aidash_observer_test",
            # The deployed observer connects as its own role; the test applies
            # the controller's exact grants and observes through that role.
            AIDASH_OBSERVER_TEST_ROLE_SQL=observer_sql(password, "aidash_observer_test"),
            AIDASH_OBSERVER_TEST_ROLE_URL=f"postgres://{OBSERVER_ROLE}:{password}@127.0.0.1:{port}/aidash_observer_test",
        )
        subprocess.run(
            [
                "cargo",
                "test",
                "--locked",
                "--manifest-path",
                str(ROOT / "infra/gcp/observer/Cargo.toml"),
                "--",
                "--ignored",
            ],
            env=environment,
            check=True,
        )
    finally:
        subprocess.run(
            ["docker", "rm", "--force", container],
            stdout=subprocess.DEVNULL,
            check=False,
        )


if __name__ == "__main__":
    main()
