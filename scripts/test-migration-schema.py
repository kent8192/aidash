#!/usr/bin/env python3
"""Compare an actual empty-database migration with the frozen legacy PostgreSQL schema.

The reference database is read-only and must have exactly the migration history
at the immutable pre-workspace revision. The target is owned by an RAII context.
Only the known dropped runs column slot is normalized; every definition and
constraint property is otherwise compared verbatim on the same database server.
"""

import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import tempfile
from urllib.parse import quote
import uuid

ROOT = Path(__file__).resolve().parents[1]
LEGACY_REVISION = "d75d1c0453a6e8bd267e428cf2303dd7c6ff8639"

QUERIES = {
    "tables": "SELECT c.relname AS name, c.relkind, c.relpersistence, c.relreplident, c.relrowsecurity, c.relforcerowsecurity FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p') AND c.relname NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY c.relname",
    "columns": "SELECT c.relname AS table_name, a.attname, a.attnum, format_type(a.atttypid,a.atttypmod) AS type, a.attnotnull, a.attidentity, a.attgenerated, pg_get_expr(d.adbin,d.adrelid) AS default_expr, col.collname AS collation FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum LEFT JOIN pg_collation col ON col.oid=a.attcollation WHERE n.nspname='public' AND c.relkind IN ('r','p') AND a.attnum>0 AND NOT a.attisdropped AND c.relname NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY c.relname,a.attnum",
    "constraints": "SELECT c.relname AS table_name, x.conname AS name, x.contype, pg_get_constraintdef(x.oid,true) AS definition, x.condeferrable, x.condeferred, x.convalidated FROM pg_constraint x JOIN pg_class c ON c.oid=x.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY c.relname,x.conname",
    "indexes": "SELECT t.relname AS table_name, i.relname AS name, pg_get_indexdef(i.oid) AS definition, x.indisvalid, x.indisready, x.indisreplident FROM pg_index x JOIN pg_class i ON i.oid=x.indexrelid JOIN pg_class t ON t.oid=x.indrelid JOIN pg_namespace n ON n.oid=t.relnamespace WHERE n.nspname='public' AND t.relname NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY t.relname,i.relname",
    "triggers": "SELECT c.relname AS table_name, t.tgname AS name, pg_get_triggerdef(t.oid,true) AS definition, t.tgenabled FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND NOT t.tgisinternal AND c.relname NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY c.relname,t.tgname",
    "functions": "SELECT p.proname AS name, pg_get_function_identity_arguments(p.oid) AS arguments, pg_get_functiondef(p.oid) AS definition FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' ORDER BY p.proname,pg_get_function_identity_arguments(p.oid)",
    "sequences": "SELECT s.relname AS name, format_type(q.seqtypid,NULL) AS type, q.seqstart, q.seqincrement, q.seqmax, q.seqmin, q.seqcache, q.seqcycle, t.relname AS owned_table, a.attname AS owned_column, d.deptype FROM pg_sequence q JOIN pg_class s ON s.oid=q.seqrelid JOIN pg_namespace n ON n.oid=s.relnamespace LEFT JOIN pg_depend d ON d.classid='pg_class'::regclass AND d.objid=s.oid AND d.refclassid='pg_class'::regclass AND d.deptype IN ('a','i') LEFT JOIN pg_class t ON t.oid=d.refobjid LEFT JOIN pg_attribute a ON a.attrelid=t.oid AND a.attnum=d.refobjsubid WHERE n.nspname='public' AND coalesce(t.relname,'') NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY s.relname",
    "extensions": "SELECT e.extname AS name, e.extversion, e.extrelocatable, n.nspname AS schema FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname='pg_jsonschema' ORDER BY e.extname",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aidash", type=Path, required=True)
    parser.add_argument("--manage", type=Path, required=True)
    parser.add_argument("--postgres-container", required=True)
    parser.add_argument("--reference-database", required=True)
    parser.add_argument("--postgres-port", type=int, required=True)
    parser.add_argument(
        "--evidence-dir", type=Path, default=ROOT / ".ignore/schema-parity"
    )
    args = parser.parse_args()
    binaries = {
        name: getattr(args, name).resolve(strict=True) for name in ("aidash", "manage")
    }
    subprocess.run(["docker", "ps"], check=True, capture_output=True)
    info = json.loads(
        subprocess.check_output(["docker", "inspect", args.postgres_container])
    )[0]
    config = dict(
        value.split("=", 1) for value in info["Config"]["Env"] if "=" in value
    )
    user = config.get("POSTGRES_USER", "postgres")
    target = "aidash_schema_" + uuid.uuid4().hex
    evidence = args.evidence_dir.resolve() / target
    evidence.mkdir(parents=True, mode=0o700)
    legacy_revision = subprocess.check_output(
        ["git", "rev-parse", LEGACY_REVISION], cwd=ROOT, text=True
    ).strip()
    legacy_source = subprocess.check_output(
        ["git", "show", legacy_revision + ":migration/src/lib.rs"], cwd=ROOT, text=True
    )
    legacy_versions = set(
        re.findall(r"Box::new\((m[0-9_a-z]+)::Migration\)", legacy_source)
    )
    if len(legacy_versions) != 55:
        raise RuntimeError(
            "the independent legacy revision must register exactly 55 migrations"
        )

    def psql(database, sql):
        result = subprocess.run(
            [
                "docker",
                "exec",
                "-i",
                args.postgres_container,
                "psql",
                "-U",
                user,
                "-d",
                database,
                "-qAt",
                "-v",
                "ON_ERROR_STOP=1",
            ],
            input=sql,
            text=True,
            capture_output=True,
            check=True,
        )
        return result.stdout.strip()

    def rows(database, query):
        return json.loads(
            psql(
                database,
                "SET search_path TO public, pg_catalog;\nSELECT coalesce(jsonb_agg(to_jsonb(snapshot)), '[]'::jsonb) FROM ("
                + query
                + ") snapshot;",
            )
        )

    @contextlib.contextmanager
    def database():
        psql("postgres", 'CREATE DATABASE "' + target + '" TEMPLATE template0;')
        try:
            yield
        finally:
            psql("postgres", 'DROP DATABASE "' + target + '" WITH (FORCE);')

    versions = set(
        psql(
            args.reference_database,
            "SELECT version FROM seaql_migrations ORDER BY version;",
        ).splitlines()
    )
    if versions != legacy_versions:
        raise RuntimeError(
            "the reference database does not match the frozen legacy migration history"
        )
    expected = {
        name: rows(args.reference_database, query) for name, query in QUERIES.items()
    }
    report = {
        "head": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "dirty": bool(
            subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT)
        ),
        "legacy_revision": legacy_revision,
        "reference_legacy_migrations": len(versions),
        "binary_sha256": {
            name: hashlib.sha256(binary.read_bytes()).hexdigest()
            for name, binary in binaries.items()
        },
        "commands": [],
        "catalog": {},
        "passed": False,
    }
    with (
        tempfile.TemporaryDirectory(prefix="aidash-schema-", dir="/tmp") as private,
        database(),
    ):
        settings = Path(private) / "settings"
        settings.mkdir()
        (settings / "base.toml").write_text(
            (ROOT / "server/settings/base.example.toml").read_text()
        )
        (settings / "local.toml").write_text("")
        environment = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("AIDASH_", "REINHARDT_"))
            and key not in ("DATABASE_URL", "NATS_URL")
        }
        environment.update(
            REINHARDT_ENV="local",
            REINHARDT_SETTINGS_DIR=str(settings),
            DATABASE_URL="postgres://"
            + quote(user, safe="")
            + ":"
            + quote(config["POSTGRES_PASSWORD"], safe="")
            + "@127.0.0.1:"
            + str(args.postgres_port)
            + "/"
            + target,
            AIDASH_API_TOKEN=secrets.token_urlsafe(32),
            AIDASH_NODE_ID="aidash://schema-parity",
        )
        try:
            for name, arguments in [
                ("aidash", ["migrate"]),
                ("manage", ["migrate"]),
                (
                    "manage",
                    [
                        "makemigrations",
                        "--state-source",
                        "files",
                        "--dry-run",
                        "--check",
                    ],
                ),
            ]:
                result = subprocess.run(
                    [str(binaries[name]), *arguments],
                    cwd=ROOT / "server",
                    env=environment,
                    text=True,
                    capture_output=True,
                    timeout=120,
                )
                report["commands"].append(
                    {"binary": name, "args": arguments, "exit_code": result.returncode}
                )
                if result.returncode:
                    raise RuntimeError(name + " " + arguments[0] + " failed")
            report["native_ledger_records"] = int(
                psql(target, "SELECT count(*) FROM reinhardt_migrations;")
            )
            report["legacy_ledger_absent"] = (
                psql(target, "SELECT to_regclass('public.seaql_migrations') IS NULL;")
                == "t"
            )
            for name, query in QUERIES.items():
                actual = rows(target, query)
                (evidence / (name + "-reference.json")).write_text(
                    json.dumps(expected[name], indent=2) + "\n"
                )
                (evidence / (name + "-native.json")).write_text(
                    json.dumps(actual, indent=2) + "\n"
                )
                normalized = actual
                if name == "columns" and actual != expected[name]:
                    known = {
                        "observed_input_seq": (18, 17),
                        "ledger_worker_ready": (19, 18),
                        "pending_human_request_id": (20, 19),
                    }
                    normalized = []
                    offsets = []
                    for left, right in zip(expected[name], actual):
                        value = right.copy()
                        if left["attnum"] != right["attnum"]:
                            if (
                                left["table_name"] != "runs"
                                or right["table_name"] != "runs"
                                or left["attname"] != right["attname"]
                                or known.get(left["attname"])
                                != (left["attnum"], right["attnum"])
                            ):
                                raise RuntimeError(
                                    "unexpected physical column ordering difference"
                                )
                            value["attnum"] = left["attnum"]
                            offsets.append(left["attname"])
                        normalized.append(value)
                    report["legacy_dropped_column_slot"] = {
                        "table": "runs",
                        "slot": 17,
                        "following_columns": offsets,
                    }
                report["catalog"][name] = {
                    "expected": len(expected[name]),
                    "actual": len(actual),
                    "equal": len(actual) == len(expected[name])
                    and normalized == expected[name],
                }
            report["passed"] = (
                report["native_ledger_records"] == 46
                and report["legacy_ledger_absent"]
                and all(value["equal"] for value in report["catalog"].values())
            )
        finally:
            (evidence / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    report["temporary_database_removed"] = True
    (evidence / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    raise SystemExit(0 if report["passed"] else 1)


if __name__ == "__main__":
    main()
