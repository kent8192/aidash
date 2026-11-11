#!/usr/bin/env python3
"""Regenerate and verify the native migration baselines.

Build ``manage`` from the current tree, then run::

    python3 scripts/rebaseline-migrations.py --manage /path/to/manage --old-ref HEAD --generate
    python3 scripts/rebaseline-migrations.py --manage /path/to/manage --old-ref OLD_REF --verify

Use an old-history ref (before the baseline commit) for both commands. Generation
refuses to overwrite a migration tree that differs from that ref, builds a
candidate in ``private/rebaseline/``, and installs it only after native replay,
model-state checking, and schema/data parity succeed. Verification replays the
ref and current tree in separate disposable PostgreSQL databases. It preserves
raw dumps and canonicalizes both schemas through PostgreSQL before diffing, since
restoring a dump may flatten associative CHECK expressions. Data dumps retain all
seed rows and sequence states but exclude the native ledger and its sequence.
The Docker image defaults to the local runtime or test PostgreSQL image;
``AIDASH_POSTGRES_IMAGE`` overrides it. The tool creates no persistent container.
Table ownership comes from the classification, with unclassified tables inferred
from their creating migration; ambiguous ownership aborts generation. Physical
baseline reversal raises an error: recreate the database instead.
"""

import argparse
import collections
import contextlib
import difflib
import json
import os
import shutil
import socket
import subprocess
import tarfile
import tempfile
import time
import uuid
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
APPS = (
    "execution",
    "federation",
    "identity",
    "knowledge",
    "marketplace",
    "operations",
    "registry",
    "workspaces",
)
HEADER = re.compile(
    r"^-- (?:Data for )?Name: (.*?); Type: (.*?); Schema: (.*?); Owner: .*$", re.M
)


def table_owners(source):
    result = {}
    for app, name in re.findall(
        r'\b(?:table|audit|named|tombstone)\(\s*"(\w+)",\s*"(\w+)"', source
    ):
        if name in result:
            raise ValueError(f"duplicate table classification: {name}")
        result[name] = app
    return result


def resolve_table_owners(root):
    owners = table_owners((root / "server/src/database/classification.rs").read_text())
    inferred = collections.defaultdict(set)
    for app in APPS:
        appdir = root / "server/migrations" / app
        for path in appdir.glob("*.rs"):
            for name in re.findall(
                r'Operation::CreateTable\s*\{\s*name:\s*"([a-zA-Z0-9_]+)"',
                path.read_text(),
            ):
                inferred[name].add(app)
        for path in (appdir / "sql/forward").glob("*.sql"):
            for name in re.findall(
                r'\bCREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:public\.)?"?([a-zA-Z0-9_]+)',
                path.read_text(),
                re.I,
            ):
                inferred[name].add(app)
    for name, candidates in inferred.items():
        if name in owners:
            continue
        if len(candidates) != 1:
            raise ValueError(f"ambiguous table owner for {name}: {sorted(candidates)}")
        owners[name] = next(iter(candidates))
    return owners


def dump_entries(text):
    matches = list(HEADER.finditer(text))
    for index, match in enumerate(matches):
        end = matches[index + 1].start() - 3 if index + 1 < len(matches) else len(text)
        body = text[match.end() : end]
        # The dump trailer contains a random psql restrict token, not SQL.
        body = re.sub(r"^\\(?:un)?restrict\s+\S+\s*$", "", body, flags=re.M)
        yield match.group(1), match.group(2), match.group(3), body


def sequence_owners(entries, owners):
    seq_owners = {}
    for name, kind, _, body in entries:
        if kind == "SEQUENCE OWNED BY":
            match = re.search(r"OWNED BY public\.([\w]+)\.", body)
            if match and match.group(1) in owners:
                seq_owners[name] = owners[match.group(1)]
        elif kind == "SEQUENCE":
            # Identity sequences are declared by ALTER TABLE on their owner.
            match = re.search(r"ALTER TABLE public\.([\w]+) ALTER COLUMN", body)
            if match and match.group(1) in owners:
                seq_owners[name] = owners[match.group(1)]
    return seq_owners


def escape_whitespace_literals(sql):
    """Keep literal control characters intact without trailing-whitespace lines."""

    def escape(match):
        return (
            "E'" + "".join(f"\\u{ord(char):04x}" for char in match.group(1)) + "'::text"
        )

    return re.sub(r"'([\s]+)'::text", escape, sql)


def assign_schema(dump, owners, function_owners):
    """Partition pg_dump objects; fail closed when ownership is unknown."""
    entries = list(dump_entries(dump))
    seq_owners = sequence_owners(entries, owners)
    index_owners = {}
    for name, kind, _, body in entries:
        if kind == "INDEX":
            match = re.search(r" ON (?:ONLY )?public\.([\w]+)", body)
            if match and match.group(1) in owners:
                index_owners[name] = owners[match.group(1)]
    out = {app: [[], []] for app in APPS}
    for name, kind, schema, body in entries:
        if (
            name in ("reinhardt_migrations", "reinhardt_migrations_id_seq")
            or kind in ("EXTENSION", "COMMENT")
            and schema == "-"
        ):
            continue
        if (
            kind == "ACL"
            and re.search(
                r"^GRANT (?:SELECT,INSERT,DELETE,UPDATE ON TABLE|SELECT,USAGE ON SEQUENCE) public\.[a-z0-9_]+ TO aidash_server;$",
                body,
                re.M,
            )
            and len(re.findall(r"(?m)^(?:GRANT|REVOKE) ", body)) == 1
        ):
            # Supplied by each database's owner default privileges, not by
            # the application migration; avoid binding to a specific role.
            continue
        if kind == "DEFAULT ACL":
            # Database provisioning installs these independently of migrations.
            continue
        if schema != "public":
            raise ValueError(f"unsupported dump schema for {kind} {name}: {schema}")
        early = kind in (
            "FUNCTION",
            "TABLE",
            "SEQUENCE",
            "SEQUENCE OWNED BY",
            "DEFAULT",
            "CONSTRAINT",
            "CHECK CONSTRAINT",
        )
        if kind == "FUNCTION":
            app = function_owners.get(name.split("(", 1)[0])
        elif kind.startswith("SEQUENCE"):
            app = seq_owners.get(name)
        elif kind == "ACL":
            words = name.split(maxsplit=1)
            if len(words) != 2:
                app = None
            elif words[0] == "SEQUENCE":
                app = seq_owners.get(words[1])
            elif words[0] == "FUNCTION":
                app = function_owners.get(words[1].split("(", 1)[0])
            else:
                app = owners.get(words[1])
        elif kind == "COMMENT":
            match = re.search(
                r"COMMENT ON (TABLE|COLUMN|SEQUENCE|INDEX|FUNCTION) public\.([\w]+)",
                body,
            )
            constraint = re.search(
                r"COMMENT ON CONSTRAINT [\w]+ ON public\.([\w]+)", body
            )
            if match:
                category, target = match.groups()
                mapping = {
                    "TABLE": owners,
                    "COLUMN": owners,
                    "SEQUENCE": seq_owners,
                    "INDEX": index_owners,
                    "FUNCTION": function_owners,
                }
                app = mapping[category].get(target)
            else:
                app = owners.get(constraint.group(1)) if constraint else None
        elif kind == "INDEX":
            match = re.search(r" ON (?:ONLY )?public\.([\w]+)", body)
            app = owners.get(match.group(1)) if match else None
        else:
            app = owners.get(name.split()[0])
        if kind == "INDEX" and re.search(r"CREATE UNIQUE INDEX", body):
            early = True
        if kind == "FK CONSTRAINT":
            match = re.search(r"REFERENCES public\.([\w]+)", body)
            if not match or match.group(1) not in owners:
                raise ValueError(f"unknown foreign key target: {name}")
            early = app == owners[match.group(1)]
        if app not in out:
            raise ValueError(f"unknown owner for {kind} {name}")
        # Objects belong to the login executing the migration, not the
        # disposable database's fixed owner name. Verification retains owners.
        body = re.sub(
            r"(?m)^ALTER (?:TABLE|SEQUENCE|FUNCTION) .* OWNER TO aidash_owner;\s*$",
            "",
            body,
        )
        if kind in ("TABLE", "CONSTRAINT", "CHECK CONSTRAINT"):
            body = escape_whitespace_literals(body)
        out[app][0 if early else 1].append(body.strip() + "\n")
    return out


def function_owners(root):
    # A filename is a migration identity; only forward assets represent applied
    # history. Duplicate definitions in the same app retain their original app.
    found = collections.defaultdict(set)
    for app in APPS:
        paths = list((root / "server/migrations" / app).glob("*.rs"))
        paths += list((root / "server/migrations" / app / "sql/forward").glob("*.sql"))
        for path in paths:
            for name in re.findall(
                r'(?:CREATE\s+(?:OR\s+REPLACE\s+)?|[\'"])FUNCTION\s+(?:public\.)?([\w]+)\s*\(',
                path.read_text(),
                re.I,
            ):
                found[name].add(app)
    ambiguous = {name: apps for name, apps in found.items() if len(apps) > 1}
    if ambiguous:
        raise ValueError(f"function ownership is ambiguous: {ambiguous}")
    result = {name: next(iter(apps)) for name, apps in found.items()}
    for app in APPS:
        for path in (root / "server/migrations" / app / "sql/forward").glob("*.sql"):
            for name in re.findall(
                r"public\.([a-z_][a-z_0-9]*)\s*\(", path.read_text()
            ):
                if name not in result:
                    found[name].add(app)
    result.update(
        {name: next(iter(apps)) for name, apps in found.items() if len(apps) == 1}
    )
    return result


def run(args, **kwargs):
    print("+", " ".join(map(str, args)), flush=True)
    return subprocess.run(args, check=True, text=True, **kwargs)


@contextlib.contextmanager
def postgres():
    name = "aidash-rebaseline-" + uuid.uuid4().hex[:12]
    image = os.environ.get("AIDASH_POSTGRES_IMAGE")
    if image is None:
        images = (
            "aidash-postgres:17-pg-jsonschema-0.3.4",
            "aidash-orm-test-postgres:17-pg-jsonschema-0.3.4",
        )
        image = next(
            (
                candidate
                for candidate in images
                if subprocess.run(
                    ["docker", "image", "inspect", candidate],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                ).returncode
                == 0
            ),
            None,
        )
        if image is None:
            raise RuntimeError(
                "neither the Aidash runtime nor test PostgreSQL image exists locally"
            )
    run(
        [
            "docker",
            "run",
            "-d",
            "--rm",
            "--name",
            name,
            "-e",
            "POSTGRES_USER=aidash",
            "-e",
            "POSTGRES_PASSWORD=aidash-local",
            "-e",
            "POSTGRES_DB=aidash",
            "-p",
            "127.0.0.1::5432",
            image,
        ],
        stdout=subprocess.DEVNULL,
    )
    try:
        for _ in range(120):
            logs = subprocess.run(
                ["docker", "logs", name], capture_output=True, text=True
            )
            if (
                "PostgreSQL init process complete" in logs.stdout
                and subprocess.run(
                    ["docker", "exec", name, "pg_isready", "-U", "aidash"],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                ).returncode
                == 0
            ):
                break
            time.sleep(1)
        else:
            raise RuntimeError("PostgreSQL did not become ready")
        port = (
            run(["docker", "port", name, "5432"], capture_output=True)
            .stdout.strip()
            .rsplit(":", 1)[1]
        )
        for _ in range(120):
            try:
                with socket.create_connection(("127.0.0.1", int(port)), timeout=1):
                    break
            except OSError:
                time.sleep(1)
        else:
            raise RuntimeError("PostgreSQL TCP endpoint did not become ready")
        psql(
            name,
            "postgres",
            "CREATE ROLE aidash_owner LOGIN PASSWORD 'aidash-owner-local'; CREATE ROLE aidash_server LOGIN PASSWORD 'aidash-server-local';",
        )
        yield name, port
    finally:
        run(["docker", "stop", name], stdout=subprocess.DEVNULL)


def psql(container, database, sql):
    return run(
        [
            "docker",
            "exec",
            "-i",
            container,
            "psql",
            "-X",
            "-v",
            "ON_ERROR_STOP=1",
            "-U",
            "aidash",
            "-d",
            database,
        ],
        input=sql,
        capture_output=True,
    ).stdout


def make_database(container, database):
    psql(
        container,
        "postgres",
        f'CREATE DATABASE "{database}" OWNER aidash_owner TEMPLATE template0;',
    )
    psql(
        container,
        database,
        "CREATE EXTENSION pg_jsonschema; CREATE EXTENSION vector; ALTER DEFAULT PRIVILEGES FOR ROLE aidash_owner GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO aidash_server; ALTER DEFAULT PRIVILEGES FOR ROLE aidash_owner GRANT USAGE, SELECT ON SEQUENCES TO aidash_server;",
    )
    for attempt in range(300):
        try:
            psql(
                container,
                database,
                "CREATE EXTENSION IF NOT EXISTS pgroonga; SELECT pgroonga_command('io_flush');",
            )
            break
        except subprocess.CalledProcessError as error:
            if (
                "pgroonga_crash_safer is preparing" not in (error.stderr or "")
                or attempt == 299
            ):
                raise
            time.sleep(0.1)


def settings(project, directory):
    directory.mkdir(parents=True, exist_ok=True)
    source = (ROOT / "server/settings/base.example.toml").read_text()
    source = source.replace(
        "[core]", "[core]\nbase_dir = " + json.dumps(str(project / "server")), 1
    )
    (directory / "base.toml").write_text(source)
    (directory / "local.toml").write_text("")


def manage(binary, project, directory, port, database, arguments):
    settings(project, directory)
    env = {
        k: v
        for k, v in os.environ.items()
        if not k.startswith(("AIDASH_", "REINHARDT_"))
        and k not in ("DATABASE_URL", "NATS_URL")
    }
    env.update(
        REINHARDT_ENV="local",
        REINHARDT_SETTINGS_DIR=str(directory),
        DATABASE_URL=f"postgres://aidash_owner:aidash-owner-local@127.0.0.1:{port}/{database}",
        AIDASH_API_TOKEN="rebaseline-local-test-token",
        AIDASH_DEPLOYMENT_ID="aidash://rebaseline",
    )
    return run([str(binary), *arguments], cwd=project / "server", env=env)


def dump(container, database, mode):
    args = [
        "docker",
        "exec",
        container,
        "pg_dump",
        "-U",
        "aidash",
        "-d",
        database,
        "--exclude-table=public.reinhardt_migrations",
        "--exclude-table=public.reinhardt_migrations_id_seq",
    ]
    args += (
        ["--schema-only"] if mode == "schema" else ["--data-only", "--column-inserts"]
    )
    print("+", " ".join(map(str, args)), flush=True)
    # Text-mode subprocesses translate CR to LF, including inside SQL literals.
    # Decode bytes directly so comparison and replay preserve exact values.
    result = subprocess.run(
        args, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    text = result.stdout.decode("utf-8")
    return re.sub(r"^\\(?:un)?restrict\s+\S+\s*$", "", text, flags=re.M)


def extract_ref(ref, destination):
    # Extract only sources needed for the old migration replay and generator.
    archive = subprocess.Popen(
        [
            "git",
            "archive",
            ref,
            "server/migrations",
            "server/src/database/classification.rs",
        ],
        cwd=ROOT,
        stdout=subprocess.PIPE,
    )
    with tarfile.open(fileobj=archive.stdout, mode="r|") as tar:
        tar.extractall(destination, filter="data")
    if archive.wait() != 0:
        raise RuntimeError(f"cannot archive {ref}")


def install_environment(old, candidate):
    source = old / "server/migrations/operations"
    target = candidate / "server/migrations/operations"
    target.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source / "0000_environment.rs", target)
    for direction in ("forward", "backward"):
        for name in (
            "0000_environment.sql",
            "extension_ownership.sql",
            "baseline_reverse_context.sql",
        ):
            dest = target / "sql" / direction / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source / "sql" / direction / name, dest)


def normalize_constraint(text):
    """Canonicalize Rust token spacing without changing string literals."""
    token = re.compile(
        r'"(?:\\.|[^"\\])*"|[A-Za-z_][A-Za-z_0-9]*|[0-9][A-Za-z_0-9]*|[^\s]', re.S
    )
    result = []
    previous_identifier = False
    position = 0
    for match in token.finditer(text):
        if text[position : match.start()].strip():
            raise ValueError("unsupported state constraint syntax")
        value = match.group()
        if value in ("'", "#") or value in ("/", "*"):
            raise ValueError(
                "unsupported literal, comment or operator in state constraint"
            )
        is_identifier = bool(re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]*", value))
        if previous_identifier and is_identifier:
            result.append(" ")
        result.append(value)
        previous_identifier = is_identifier
        position = match.end()
    if text[position:].strip():
        raise ValueError("unsupported state constraint syntax")
    return "".join(result)


def canonicalize_state_constraints(text):
    """Order named table constraints while preserving their Rust syntax."""
    marker = re.compile(r"\bconstraints\s*:\s*vec!\s*\[")
    position = 0
    while match := marker.search(text, position):
        start = match.end()
        stack = ["]"]
        parts = []
        item = start
        i = start
        quote = False
        while i < len(text) and stack:
            char = text[i]
            if quote:
                if char == "\\":
                    i += 2
                    continue
                if char == '"':
                    quote = False
            elif char == '"':
                quote = True
            elif char in "([{":
                stack.append({"(": ")", "[": "]", "{": "}"}[char])
            elif char in ")]}":
                if not stack or stack.pop() != char:
                    raise ValueError("unbalanced state constraint syntax")
                if not stack:
                    break
            elif char == "," and len(stack) == 1:
                parts.append(text[item:i])
                item = i + 1
            i += 1
        if stack or quote:
            raise ValueError("unterminated state constraint list")
        tail = text[item:i]
        if tail.strip():
            parts.append(tail)
            tail = ""
        if len(parts) > 1:
            keyed = []
            for part in parts:
                name = re.search(r'\bname\s*:\s*"([^"\n]+)"', part)
                if not name:
                    raise ValueError("state constraint lacks a stable name")
                keyed.append((name.group(1), part))
            if len({name for name, _ in keyed}) != len(keyed):
                raise ValueError("duplicate state constraint name")
            line_start = text.rfind("\n", 0, match.start()) + 1
            indentation = re.match(r"[\t ]*", text[line_start : match.start()]).group()
            replacement = (
                "\n"
                + ",\n".join(
                    indentation + "\t" + normalize_constraint(part.strip())
                    for _, part in sorted(keyed)
                )
                + ",\n"
                + indentation
            )
            text = text[:start] + replacement + text[i:]
            i = start + len(replacement)
        position = i + 1
    return text


def use_hard_tabs(text):
    def indent(match):
        width = len(match.group(0))
        return "\t" * (width // 4) + " " * (width % 4)

    return re.sub(r"^ +", indent, text, flags=re.M)


def migration(app, name, dependencies, seed):
    lines = [
        "// reinhardt-migration-source: 1",
        "// Generated baseline: PostgreSQL schema and seed data captured from the old history.",
        "// Raw SQL preserves physical PostgreSQL DDL and data alongside the typed state snapshot.",
        "use reinhardt::db::migrations::prelude::*;",
        "",
        "pub(super) fn migration() -> Migration {",
        f'\tMigration::new("{name}", "{app}")',
        "\t\t.database_only(true)",
    ]
    lines += [f'\t\t.add_dependency("{a}", "{n}")' for a, n in dependencies]
    lines += [
        "\t\t.add_operation(Operation::RunSQL {",
        f'\t\t\tsql: include_str!("sql/forward/{name}.sql").to_owned(),',
        "\t\t\treverse_sql: Some(\"DO $$ BEGIN RAISE EXCEPTION 'Aidash baseline is irreversible; recreate the database'; END $$;\".to_owned()),",
        "\t\t})",
    ]
    if seed:
        lines += [
            "\t\t.add_operation(Operation::RunSQL {",
            '\t\t\tsql: include_str!("sql/forward/baseline_seed.sql").to_owned(),',
            "\t\t\treverse_sql: Some(\"DO $$ BEGIN RAISE EXCEPTION 'Aidash baseline is irreversible; recreate the database'; END $$;\".to_owned()),",
            "\t\t})",
        ]
    lines += ["}", ""]
    return "\n".join(lines)


def generate(binary, old, candidate, temp, container, port):
    project = candidate / "server"
    (project / "src/bin").mkdir(parents=True)
    shutil.copy2(ROOT / "server/src/bin/manage.rs", project / "src/bin/manage.rs")
    (project / "migrations").mkdir()
    manage(
        binary,
        candidate,
        temp / "state-settings",
        port,
        "aidash",
        ["makemigrations", "--state-source", "files", "--name", "state"],
    )
    install_environment(old, candidate)
    owners = resolve_table_owners(old)
    funcs = function_owners(old)
    schema_dump = dump(container, "old", "schema")
    batches = assign_schema(schema_dump, owners, funcs)
    sequences = sequence_owners(list(dump_entries(schema_dump)), owners)
    # Tables refer to validator functions in other apps. Order those app table
    # phases by the actual function names present in each captured table block.
    deps = {app: set() for app in APPS}
    for app, (early, _) in batches.items():
        for block in early:
            if "CREATE TABLE " in block or "CREATE UNIQUE INDEX " in block:
                for function in re.findall(r"public\.([a-z_][a-z_0-9]*)\s*\(", block):
                    owner = funcs.get(function)
                    if owner and owner != app:
                        deps[app].add(owner)
    federation_seed = None
    for name, kind, _schema, body in dump_entries(dump(container, "old", "data")):
        if name in ("reinhardt_migrations", "reinhardt_migrations_id_seq"):
            continue
        if kind == "TABLE DATA":
            app = owners.get(name)
            if not app:
                raise ValueError(f"unknown seed owner {name}")
            if name == "atomic_gate":
                federation_seed = re.sub(r"^\s*--\s*", "", body).strip()
            if body.strip() and name != "atomic_gate":
                batches[app][0].append(body.strip() + "\n")
        elif kind == "SEQUENCE SET":
            app = sequences.get(name)
            if not app:
                raise ValueError(f"unknown sequence seed owner: {name}")
            # Seed values also cover sequences advanced by migration data.
            batches[app][0].append(body.strip() + "\n")

        else:
            raise ValueError(f"unknown data object {kind} {name}")
    for app in APPS:
        appdir = project / "migrations" / app
        appdir.mkdir(parents=True, exist_ok=True)
        state = appdir / "0001_state.rs"
        if state.exists():
            text = state.read_text().replace(".state_only(false)", ".state_only(true)")
            state.write_text(text)
        else:
            state.write_text(
                f'// reinhardt-migration-source: 1\nuse reinhardt::db::migrations::prelude::*;\npub(super) fn migration() -> Migration {{ Migration::new("0001_state", "{app}").state_only(true) }}\n'
            )
        for index, name in enumerate(("0002_tables", "0003_references")):
            dependencies = (
                [(app, "0001_state")]
                if index == 0
                else [(a, "0002_tables") for a in APPS]
            )
            if index == 0:
                dependencies += [("operations", "0000_environment")]
                if app != "operations":
                    dependencies += [("operations", "0002_tables")]
                dependencies += [(a, "0002_tables") for a in sorted(deps[app])]
            (appdir / f"{name}.rs").write_text(
                migration(app, name, dependencies, app == "federation" and index == 0)
            )
            asset = appdir / "sql/forward" / f"{name}.sql"
            asset.parent.mkdir(parents=True, exist_ok=True)
            extension_sql = (
                "CREATE EXTENSION IF NOT EXISTS vector;\nCREATE EXTENSION IF NOT EXISTS pgroonga;\n"
                if app == "operations" and index == 0
                else ""
            )
            asset.write_text(
                "SET LOCAL check_function_bodies = false;\nSET LOCAL search_path = public, pg_catalog;\n"
                + extension_sql
                + "\n".join(batches[app][index])
            )
            if app == "federation" and index == 0:
                if not federation_seed:
                    raise ValueError("missing atomic_gate seed")
                (appdir / "sql/forward/baseline_seed.sql").write_text(
                    "SET LOCAL search_path = public, pg_catalog;\n\n"
                    + federation_seed
                    + "\n"
                )
        run(
            [
                "rustfmt",
                "--edition",
                "2024",
                "--config-path",
                str(ROOT / "rustfmt.toml"),
                "--config",
                "skip_children=true",
                str(state),
                str(appdir / "0002_tables.rs"),
                str(appdir / "0003_references.rs"),
            ]
        )
        state.write_text(
            canonicalize_state_constraints(use_hard_tabs(state.read_text()))
        )
        run(
            [
                "rustfmt",
                "--edition",
                "2024",
                "--config-path",
                str(ROOT / "rustfmt.toml"),
                "--config",
                "skip_children=true",
                str(state),
            ]
        )
        state.write_text(use_hard_tabs(state.read_text()))


def compare(container, evidence):
    equal = True
    for kind in ("schema", "data"):
        old = dump(container, "old", kind)
        new = dump(container, "new", kind)
        (evidence / f"old-{kind}-raw.sql").write_text(old)
        (evidence / f"new-{kind}-raw.sql").write_text(new)
        if kind == "schema":
            # A pg_dump CHECK expression can be flattened by PostgreSQL's SQL
            # parser on restore. Canonicalize *both* sides through the same
            # PostgreSQL version; retain the untouched dumps for inspection.
            for name, schema in (("canonical_old", old), ("canonical_new", new)):
                psql(
                    container,
                    "postgres",
                    f'CREATE DATABASE "{name}" TEMPLATE template0;',
                )
                psql(container, name, schema)
            old = dump(container, "canonical_old", kind)
            new = dump(container, "canonical_new", kind)
        (evidence / f"old-{kind}.sql").write_text(old)
        (evidence / f"new-{kind}.sql").write_text(new)
        diff = "".join(
            difflib.unified_diff(
                old.splitlines(keepends=True),
                new.splitlines(keepends=True),
                fromfile="old",
                tofile="new",
            )
        )
        (evidence / f"{kind}.diff").write_text(diff)
        print(
            f"{kind}: {'equal' if not diff else 'DIFFERENT'} ({len(diff.splitlines())} diff lines)",
            flush=True,
        )
        equal &= not diff
    return equal


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--inspect-dump",
        type=Path,
        help="inspect a schema-only dump without modifying migrations",
    )
    parser.add_argument(
        "--manage", type=Path, help="manage binary built from this checkout"
    )
    parser.add_argument(
        "--old-ref", help="git ref containing the old migration history"
    )
    parser.add_argument(
        "--generate",
        action="store_true",
        help="generate and verify a candidate before installing it",
    )
    parser.add_argument(
        "--verify",
        action="store_true",
        help="compare the old ref with the working tree",
    )
    parser.add_argument(
        "--candidate-only",
        action="store_true",
        help="leave the verified candidate in private/ without installing it",
    )
    args = parser.parse_args()
    if args.inspect_dump:
        owners = resolve_table_owners(ROOT)
        batches = assign_schema(
            args.inspect_dump.read_text(), owners, function_owners(ROOT)
        )
        for app, (early, late) in batches.items():
            print(
                f"{app}: {len(early)} table-phase objects, {len(late)} reference-phase objects"
            )
        return
    if not args.manage or not args.old_ref or args.generate == args.verify:
        parser.error(
            "supply --manage, --old-ref and exactly one of --generate or --verify"
        )
    binary = args.manage.resolve(strict=True)
    evidence_root = ROOT / "private/rebaseline"
    evidence_root.mkdir(parents=True, exist_ok=True)
    temp = Path(tempfile.mkdtemp(prefix="run-", dir=evidence_root))
    old = temp / "old"
    old.mkdir()
    extract_ref(args.old_ref, old)
    (old / "server/src/bin").mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / "server/src/bin/manage.rs", old / "server/src/bin/manage.rs")
    if args.generate and not args.candidate_only:
        for app in APPS:
            reference = old / "server/migrations" / app
            working = ROOT / "server/migrations" / app
            before = {
                p.relative_to(reference): p.read_bytes()
                for p in reference.rglob("*")
                if p.is_file()
            }
            current = {
                p.relative_to(working): p.read_bytes()
                for p in working.rglob("*")
                if p.is_file()
            }
            if before != current:
                raise RuntimeError(
                    f"{working} differs from {args.old_ref}; refusing to replace it"
                )
    print("evidence:", temp, flush=True)
    with postgres() as (container, port):
        make_database(container, "old")
        manage(binary, old, temp / "old-settings", port, "old", ["migrate"])
        if args.generate:
            candidate = temp / "candidate"
            candidate.mkdir()
            generate(binary, old, candidate, temp, container, port)
        else:
            candidate = ROOT
        make_database(container, "new")
        manage(binary, candidate, temp / "new-settings", port, "new", ["migrate"])
        manage(
            binary,
            candidate,
            temp / "new-settings",
            port,
            "new",
            ["makemigrations", "--state-source", "files", "--dry-run", "--check"],
        )
        if not compare(container, temp):
            raise RuntimeError(f"baseline differs; inspect diffs in {temp}")
        if args.generate and not args.candidate_only:
            source = candidate / "server/migrations"
            target = ROOT / "server/migrations"
            for app in APPS:
                shutil.rmtree(target / app)
                shutil.copytree(source / app, target / app)
            print("Installed verified baselines into", target)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr, file=sys.stderr)
        sys.exit(1)
