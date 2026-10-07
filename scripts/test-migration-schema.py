#!/usr/bin/env python3
"""Compare a fresh native bootstrap with an independently frozen native schema.

Both ledgers must match their complete source identities. Every catalog definition
is compared verbatim on the same PostgreSQL server; the reference stays read-only
and the disposable target belongs exclusively to this invocation.
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
REFERENCE_REVISION = "cd29635a9937133d2e81cca44857bea331446cc5"

QUERIES = {
    "tables": "SELECT c.relname AS name, c.relkind, c.relpersistence, c.relreplident, c.relrowsecurity, c.relforcerowsecurity FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p') AND c.relname NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY c.relname",
    "columns": "SELECT c.relname AS table_name, a.attname, a.attnum, format_type(a.atttypid,a.atttypmod) AS type, a.attnotnull, a.attidentity, a.attgenerated, pg_get_expr(d.adbin,d.adrelid) AS default_expr, col.collname AS collation FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum LEFT JOIN pg_collation col ON col.oid=a.attcollation WHERE n.nspname='public' AND c.relkind IN ('r','p') AND a.attnum>0 AND NOT a.attisdropped AND c.relname NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY c.relname,a.attnum",
    "constraints": "SELECT c.relname AS table_name, x.conname AS name, x.contype, pg_get_constraintdef(x.oid,true) AS definition, x.condeferrable, x.condeferred, x.convalidated FROM pg_constraint x JOIN pg_class c ON c.oid=x.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY c.relname,x.conname",
    "indexes": "SELECT t.relname AS table_name, i.relname AS name, pg_get_indexdef(i.oid) AS definition, x.indisvalid, x.indisready, x.indisreplident FROM pg_index x JOIN pg_class i ON i.oid=x.indexrelid JOIN pg_class t ON t.oid=x.indrelid JOIN pg_namespace n ON n.oid=t.relnamespace WHERE n.nspname='public' AND t.relname NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY t.relname,i.relname",
    "triggers": "SELECT c.relname AS table_name, t.tgname AS name, pg_get_triggerdef(t.oid,true) AS definition, t.tgenabled FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND NOT t.tgisinternal AND c.relname NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY c.relname,t.tgname",
    "functions": "SELECT p.proname AS name, pg_get_function_identity_arguments(p.oid) AS arguments, pg_get_functiondef(p.oid) AS definition FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' AND p.prokind <> 'a' ORDER BY p.proname,pg_get_function_identity_arguments(p.oid)",
    "aggregates": "SELECT p.proname AS name, pg_get_function_identity_arguments(p.oid) AS arguments, format_type(p.prorettype,NULL) AS return_type, p.proparallel, a.aggkind, a.aggnumdirectargs, a.aggtransfn::regprocedure::text AS transition, a.aggfinalfn::regprocedure::text AS final, a.aggcombinefn::regprocedure::text AS combine, a.aggserialfn::regprocedure::text AS serial, a.aggdeserialfn::regprocedure::text AS deserial, a.aggmtransfn::regprocedure::text AS moving_transition, a.aggminvtransfn::regprocedure::text AS moving_inverse, a.aggmfinalfn::regprocedure::text AS moving_final, a.aggfinalextra, a.aggmfinalextra, a.aggfinalmodify, a.aggmfinalmodify, a.aggsortop::regoperator::text AS sort_operator, format_type(a.aggtranstype,NULL) AS transition_type, a.aggtransspace, format_type(a.aggmtranstype,NULL) AS moving_transition_type, a.aggmtransspace, a.agginitval, a.aggminitval FROM pg_aggregate a JOIN pg_proc p ON p.oid=a.aggfnoid JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' ORDER BY p.proname,pg_get_function_identity_arguments(p.oid)",
    "sequences": "SELECT s.relname AS name, format_type(q.seqtypid,NULL) AS type, q.seqstart, q.seqincrement, q.seqmax, q.seqmin, q.seqcache, q.seqcycle, t.relname AS owned_table, a.attname AS owned_column, d.deptype FROM pg_sequence q JOIN pg_class s ON s.oid=q.seqrelid JOIN pg_namespace n ON n.oid=s.relnamespace LEFT JOIN pg_depend d ON d.classid='pg_class'::regclass AND d.objid=s.oid AND d.refclassid='pg_class'::regclass AND d.deptype IN ('a','i') LEFT JOIN pg_class t ON t.oid=d.refobjid LEFT JOIN pg_attribute a ON a.attrelid=t.oid AND a.attnum=d.refobjsubid WHERE n.nspname='public' AND coalesce(t.relname,'') NOT IN ('seaql_migrations','reinhardt_migrations') ORDER BY s.relname",
    "extensions": "SELECT e.extname AS name, e.extversion, e.extrelocatable, n.nspname AS schema FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE e.extname IN ('pg_jsonschema', 'vector', 'pgroonga') ORDER BY e.extname",
}


def migration_identities(revision=None):
    """Read every native source; refuse missing or duplicate migration headers."""
    if revision is None:
        sources = {
            str(path): path.read_text()
            for path in sorted((ROOT / "server/migrations").glob("*/*.rs"))
        }
    else:
        paths = subprocess.check_output(
            ["git", "ls-tree", "-r", "--name-only", revision, "server/migrations"],
            cwd=ROOT,
            text=True,
        ).splitlines()
        sources = {
            path: subprocess.check_output(
                ["git", "show", revision + ":" + path], cwd=ROOT, text=True
            )
            for path in paths
            if re.fullmatch(r"server/migrations/[^/]+/[^/]+\.rs", path)
        }
    identities = set()
    for path, source in sources.items():
        headers = re.findall(
            r'Migration::new\(\s*"([^"\n]+)"\s*,\s*"([^"\n]+)"\s*\)', source
        )
        if len(headers) != 1 or "// reinhardt-migration-source: 1" not in source:
            raise RuntimeError("invalid native migration header: " + path)
        name, app = headers[0]
        identity = (app, name)
        if identity in identities:
            raise RuntimeError("duplicate native migration identity: " + repr(identity))
        identities.add(identity)
    if not identities:
        raise RuntimeError("native migration history is empty")
    return identities


def validate_ledger(records, expected):
    actual = [(record["app"], record["name"]) for record in records]
    return len(actual) == len(set(actual)) and set(actual) == expected


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
    reference_revision = subprocess.check_output(
        ["git", "rev-parse", REFERENCE_REVISION], cwd=ROOT, text=True
    ).strip()
    reference_identities = migration_identities(reference_revision)
    target_identities = migration_identities()

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

    ledger_query = "SELECT app, name FROM reinhardt_migrations ORDER BY app, name"
    reference_records = rows(args.reference_database, ledger_query)
    if not validate_ledger(reference_records, reference_identities):
        raise RuntimeError(
            "reference ledger differs from the frozen native source identities"
        )
    if (
        psql(
            args.reference_database,
            "SELECT to_regclass('public.seaql_migrations') IS NULL;",
        )
        != "t"
    ):
        raise RuntimeError("reference retains the retired migration ledger")
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
        "reference_revision": reference_revision,
        "reference_native_migrations": len(reference_records),
        "expected_native_migrations": len(target_identities),
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
            (ROOT / "server/settings/base.example.toml")
            .read_text()
            .replace(
                "[core]", "[core]\nbase_dir = " + json.dumps(str(ROOT / "server")), 1
            )
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
            target_records = rows(target, ledger_query)
            report["native_ledger_records"] = len(target_records)
            report["native_ledger_matches_sources"] = validate_ledger(
                target_records, target_identities
            )
            (evidence / "ledger-native.json").write_text(
                json.dumps(target_records, indent=2) + "\n"
            )
            (evidence / "ledger-reference.json").write_text(
                json.dumps(reference_records, indent=2) + "\n"
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
                report["catalog"][name] = {
                    "expected": len(expected[name]),
                    "actual": len(actual),
                    "equal": len(actual) == len(expected[name])
                    and actual == expected[name],
                }
            report["passed"] = (
                report["native_ledger_matches_sources"]
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
