#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 kent8192
# SPDX-License-Identifier: BUSL-1.1
# See enterprise/LICENSE.

"""Run controller SQL and observer checks against a fresh local-only PostgreSQL container."""

import os
from pathlib import Path
import secrets
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "enterprise/infra/gcp/control"))
from kube import LEGACY_OWNERSHIP_SQL, OBSERVER_ROLE, OWNER_ROLE, observer_sql, MEMORY_ROLE, memory_role_sql  # noqa: E402


def check_legacy_ownership(container):
    """The pre-role check must also refuse a database whose owner role does not exist yet."""
    def psql(sql, database="aidash_legacy_test"):
        return subprocess.check_output(
            ["docker", "exec", container, "psql", "-U", "postgres", "-d", database,
             "--tuples-only", "--no-align", "-v", "ON_ERROR_STOP=1", "-c", sql],
            text=True,
        ).strip()

    psql("CREATE DATABASE aidash_legacy_test", "postgres")
    expectations = [
        (None, "f"),
        ("CREATE TABLE legacy_probe (id integer)", "t"),
        (f"CREATE ROLE {OWNER_ROLE}", "t"),
        (f"ALTER TABLE legacy_probe OWNER TO {OWNER_ROLE}", "f"),
    ]
    for statement, expected in expectations:
        if statement:
            psql(statement)
        actual = psql(LEGACY_OWNERSHIP_SQL)
        if actual != expected:
            raise AssertionError(f"legacy ownership after {statement!r}: {actual!r} != {expected!r}")


def check_memory_role(container):
    """Exercise the deployed login and guards, including its required row locks."""
    database = "aidash_observer_test"
    def psql(sql, role="postgres", success=True):
        result = subprocess.run(["docker", "exec", "-i", container, "psql", "-h", "127.0.0.1", "-U", role,
                                 "-d", database, "-v", "ON_ERROR_STOP=1", "--no-psqlrc", "--tuples-only", "--no-align"],
                                input=sql, text=True, capture_output=True)
        if (result.returncode == 0) != success:
            raise AssertionError(f"memory login boundary: {sql!r}: {result.stderr}")
        return result.stdout.strip()
    psql("CREATE TABLE memory_role_fixture (id integer PRIMARY KEY, body text); INSERT INTO memory_role_fixture VALUES (1, 'retained');")
    psql(memory_role_sql(secrets.token_hex(32), database))
    psql(memory_role_sql(secrets.token_hex(32), database))  # idempotent grants and guards
    assert psql("BEGIN; SELECT body FROM memory_role_fixture FOR SHARE; COMMIT;", MEMORY_ROLE).splitlines()[1] == "retained"
    for statement in ("UPDATE memory_role_fixture SET id = id", "UPDATE memory_role_fixture SET body = 'rewritten'",
                      "DELETE FROM memory_role_fixture", "INSERT INTO memory_role_fixture VALUES (2, 'added')",
                      "ALTER TABLE memory_role_fixture DISABLE TRIGGER ALL", "SET ROLE postgres"):
        psql(statement, MEMORY_ROLE, success=False)
    assert psql("SELECT body FROM memory_role_fixture") == "retained"
    psql("DROP TABLE memory_role_fixture")


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
        check_legacy_ownership(container)
        check_memory_role(container)
        if "--memory-role-only" in sys.argv:
            print("memory recovery login: row locks allowed, mutations and privilege escalation refused")
            return
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
                str(ROOT / "enterprise/infra/gcp/observer/Cargo.toml"),
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
