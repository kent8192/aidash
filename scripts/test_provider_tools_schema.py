#!/usr/bin/env python3
"""Exercise Provider tool migration contracts in a disposable PostgreSQL instance."""

import copy
import json
import os
from pathlib import Path
import re
import subprocess
import time
import unittest
import uuid

ROOT = Path(__file__).resolve().parents[1]
SQL = ROOT / "server/migrations/registry/sql"


def function(path, name):
    source = path.read_text()
    match = re.search(
        r"CREATE (?:OR REPLACE )?FUNCTION public\."
        + re.escape(name)
        + r"\([^;]+?\bAS\s+(\$[a-zA-Z0-9_]*\$)",
        source,
        re.S,
    )
    if not match:
        raise AssertionError(f"function {name} missing in {path}")
    end = source.index(match[1] + ";", match.end()) + len(match[1]) + 1
    return source[match.start() : end]


def literal(value):
    return "'" + json.dumps(value).replace("'", "''") + "'::jsonb"


def descriptor(tool):
    return {
        "registry_deployment": "aidash://provider-tools-test",
        "provider": f"provider.openrouter.{tool}@1",
        "operation": f"provider_{tool}",
        "default_alias": f"provider_{tool}",
        "tier": "provider",
    }


PINS = [
    {
        "max_tool_calls": 3,
        "parameters": {"engine": "exa", "max_results": 5, "max_total_results": 15},
    },
    {
        "max_tool_calls": 3,
        "parameters": {"engine": "openrouter", "max_content_tokens": 8192},
    },
    {"max_tool_calls": 3, "parameters": {"timezone": "UTC"}},
    {
        "max_tool_calls": 3,
        "parameters": {
            "analysis_models": ["vendor/panel"],
            "model": "vendor/analyst",
            "max_tool_calls": 2,
            "max_completion_tokens": 4096,
        },
    },
]


def agent(pin):
    return {
        "schema_version": 1,
        "model": {"id": "model", "version": "1.0.0"},
        "instructions": "Answer the request.",
        "agent_plugins": [],
        "agent_plugin_settings": {},
        "exposure": {"version": "deferred@1"},
        "bindings": [
            {
                "kind": "tool",
                "target": {
                    "registry_deployment": "aidash://provider-tools-test",
                    "id": "provider.openrouter.datetime",
                    "version": "1.0.0",
                },
                "exposure": "deferred",
                "narrow": {"provider_tool": pin},
            }
        ],
    }


class ProviderToolSchema(unittest.TestCase):
    container = None

    @classmethod
    def sql(cls, source, *, check=True):
        result = subprocess.run(
            [
                "docker", "exec", "-i", cls.container,
                "psql", "-U", "postgres", "-X", "-A", "-t", "-q",
                "-v", "ON_ERROR_STOP=1",
            ],
            input="BEGIN;\n" + source + "\nCOMMIT;\n",
            capture_output=True,
            text=True,
        )
        if check and result.returncode:
            raise AssertionError(result.stderr)
        return result

    @classmethod
    def setUpClass(cls):
        image = os.environ.get(
            "AIDASH_TEST_POSTGRES_IMAGE",
            "aidash-orm-test-postgres:17-pg-jsonschema-0.3.4",
        )
        result = subprocess.run(
            [
                "docker", "run", "--rm", "-d", "--name",
                "aidash-w5-provider-schema-" + uuid.uuid4().hex[:12],
                "-e", "POSTGRES_PASSWORD=provider-schema-test-only", image,
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode:
            raise RuntimeError(f"Disposable PostgreSQL creation failed: {result.stderr}")
        cls.container = result.stdout.strip()
        if not re.fullmatch("[0-9a-f]{64}", cls.container):
            raise AssertionError("Docker did not return an owned container id")
        cls.addClassCleanup(
            subprocess.run,
            ["docker", "rm", "-f", cls.container],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        for _ in range(100):
            ready = subprocess.run(
                ["docker", "exec", cls.container, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
                capture_output=True,
            )
            if ready.returncode == 0:
                break
            time.sleep(0.1)
        else:
            raise AssertionError("disposable PostgreSQL did not become ready")
        cls.sql("CREATE EXTENSION pg_jsonschema WITH SCHEMA public;")
        # Load real pre-migration contracts; no fake validators or weakened checks.
        for path, name in [
            ("0011_binding_contract.sql", "aidash_qualified_ref_is_valid"),
            ("0034_research_view.sql", "aidash_descriptor_is_valid"),
            ("0045_agent_plugin_settings.sql", "aidash_agent_bindings_is_valid"),
        ]:
            cls.sql(function(SQL / "forward" / path, name))
        cls.sql("""
            CREATE TABLE registry(kind text, metadata jsonb NOT NULL,
              CONSTRAINT registry_tool_config CHECK (kind <> 'tool' OR
                COALESCE(public.aidash_descriptor_is_valid(metadata->'config'),false)),
              CONSTRAINT registry_agent_config CHECK (kind <> 'agent' OR
                COALESCE(public.aidash_agent_bindings_is_valid(metadata->'config'),false)));
            CREATE TABLE runs(context jsonb NOT NULL);
        """)
        cls.before = cls.signature()
        cls.sql((SQL / "forward/0050_provider_tools.sql").read_text())
        cls.sql((SQL / "forward/0051_provider_warning_tools.sql").read_text())

    @classmethod
    def signature(cls):
        result = cls.sql("""
            SELECT jsonb_object_agg(proname, jsonb_build_object('oid',oid,'body',prosrc))
            FROM pg_proc WHERE oid IN (
              'public.aidash_agent_bindings_is_valid(jsonb)'::regprocedure,
              'public.aidash_descriptor_is_valid(jsonb)'::regprocedure);
        """)
        return json.loads(result.stdout)

    def setUp(self):
        self.sql("TRUNCATE registry, runs;")

    def valid(self, function_name, value):
        return self.sql(f"SELECT public.{function_name}({literal(value)});").stdout.strip() == "t"


    def test_warning_tools_are_closed_at_database_constraints(self):
        for tool in ["advisor", "subagent", "experimental__search_models", "tool_search", "shell"]:
            value = descriptor(tool)
            if tool == "experimental__search_models":
                value["operation"] = value["default_alias"] = "provider_search_models"
            self.assertTrue(self.valid("aidash_descriptor_is_valid", value))
            self.sql("INSERT INTO registry VALUES ('tool'," + literal({"config": value}) + ");")
        pins = [
            {"model":"vendor/model","max_completion_tokens":4096,"forward_transcript":False,"stream":False},
            {"model":"vendor/model","max_completion_tokens":4096,"max_tool_calls":2,"tools":[],"inherit_functions":False},
            {"max_results":5},
            {"engine":"openrouter","environment":{"type":"container_reference","network_policy":{"type":"disabled"}}},
        ]
        for parameters in pins:
            pin = {"max_tool_calls":3,"parameters":parameters}
            self.assertTrue(self.valid("aidash_provider_tool_binding_is_valid", pin))
            self.sql("INSERT INTO registry VALUES ('agent'," + literal({"config":agent(pin)}) + ");")
            for key, value in [("max_tool_calls",9),("parameters",{**parameters,"inherit_functions":True})]:
                rejected = self.sql("INSERT INTO registry VALUES ('agent'," + literal({"config":agent({**pin,key:value})}) + ");",check=False)
                self.assertNotEqual(rejected.returncode,0)
        for change in [{"container_id":"another_run"},{"network_policy":{"type":"allowlist","allowed_domains":["*"]}}]:
            bad = copy.deepcopy(pins[-1])
            bad["environment"].update(change)
            self.assertFalse(self.valid("aidash_provider_tool_binding_is_valid",{"max_tool_calls":3,"parameters":bad}))

    def test_closed_descriptors_and_actual_check_constraints(self):
        for tool in ["web_search", "web_fetch", "datetime", "fusion"]:
            value = descriptor(tool)
            with self.subTest(tool=tool):
                self.assertTrue(self.valid("aidash_descriptor_is_valid", value))
                self.sql(
                    "INSERT INTO registry VALUES ('tool',"
                    + literal({"config": value}) + ");"
                )
                for key, replacement in [
                    ("provider", "provider.openrouter.unknown@1"),
                    ("operation", "shell"),
                    ("default_alias", "apply_patch"),
                    ("tier", "builtin"),
                    ("transport", {"transport": "http"}),
                    ("narrow", {"provider_tool": PINS[2]}),
                ]:
                    invalid = {**value, key: replacement}
                    self.assertFalse(self.valid("aidash_descriptor_is_valid", invalid))
                    rejected = self.sql(
                        "INSERT INTO registry VALUES ('tool',"
                        + literal({"config": invalid}) + ");", check=False,
                    )
                    self.assertNotEqual(rejected.returncode, 0)
                    self.assertIn("registry_tool_config", rejected.stderr)

    def test_parameters_have_explicit_bounded_spending_and_closed_engines(self):
        for value in PINS:
            self.assertTrue(self.valid("aidash_provider_tool_binding_is_valid", value))
        invalid = []
        for cap in [0, 9, 3.5, "3", None]:
            invalid.append({**PINS[2], "max_tool_calls": cap})
        invalid.append({"parameters": {"timezone": "UTC"}})
        for base, key, value in [
            (0, "engine", "firecrawl"), (0, "max_results", 11),
            (0, "max_total_results", 31), (0, "max_total_results", 1),
            (1, "max_content_tokens", 16385), (1, "engine", "auto"),
            (2, "timezone", "UTC'; SELECT 1"),
            (3, "analysis_models", ["vendor/model"] * 5),
            (3, "analysis_models", ["openrouter/auto"]),
            (3, "model", "vendor/model:online"),
            (3, "max_tool_calls", 5), (3, "max_completion_tokens", 8193),
        ]:
            candidate = copy.deepcopy(PINS[base])
            candidate["parameters"][key] = value
            invalid.append(candidate)
        candidate = copy.deepcopy(PINS[0])
        candidate["parameters"].update(allowed_domains=["example.com"], excluded_domains=["x.com"])
        invalid.append(candidate)
        candidate = copy.deepcopy(PINS[2])
        candidate["parameters"]["unapproved"] = True
        invalid.append(candidate)
        for value in invalid:
            with self.subTest(value=value):
                self.assertFalse(self.valid("aidash_provider_tool_binding_is_valid", value))

    def test_agent_pins_keep_all_existing_contract_checks(self):
        value = agent(PINS[2])
        self.assertTrue(self.valid("aidash_agent_bindings_is_valid", value))
        self.sql("INSERT INTO registry VALUES ('agent'," + literal({"config": value}) + ");")
        for mutation in ["unbounded", "unknown_field", "wrong_kind", "old_narrow_check"]:
            invalid = copy.deepcopy(value)
            if mutation == "unbounded":
                invalid["bindings"][0]["narrow"]["provider_tool"]["max_tool_calls"] = 9
            elif mutation == "unknown_field":
                invalid["unapproved"] = True
            elif mutation == "wrong_kind":
                invalid["bindings"][0]["kind"] = "memory"
            else:
                invalid["bindings"][0]["narrow"]["unapproved"] = True
            self.assertFalse(self.valid("aidash_agent_bindings_is_valid", invalid), mutation)
        value["bindings"][0]["narrow"]["provider_tool"] = None
        self.assertTrue(self.valid("aidash_agent_bindings_is_valid", value))

    def test_legacy_function_identity_and_exact_bodies_are_preserved(self):
        current = self.signature()
        for public_name, backup_name in [
            ("aidash_agent_bindings_is_valid", "aidash_agent_before_provider_tools_0050"),
            ("aidash_descriptor_is_valid", "aidash_descriptor_before_provider_tools_0050"),
        ]:
            self.assertEqual(current[public_name]["oid"], self.before[public_name]["oid"])
            result = self.sql(f"SELECT to_json(prosrc) FROM pg_proc WHERE oid = 'public.{backup_name}(jsonb)'::regprocedure;")
            self.assertEqual(json.loads(result.stdout), self.before[public_name]["body"])
        legacy = {
            "registry_deployment": "aidash://provider-tools-test", "provider": "core.files@1",
            "operation": "file_read", "default_alias": "file_read", "tier": "builtin",
        }
        self.assertTrue(self.valid("aidash_descriptor_is_valid", legacy))

    def test_rollback_refuses_history_then_restores_original_contracts(self):
        reverse = (SQL / "backward/0051_provider_warning_tools.sql").read_text() + (SQL / "backward/0050_provider_tools.sql").read_text()
        self.sql("INSERT INTO registry VALUES ('tool'," + literal({"config": descriptor("datetime")}) + ");")
        blocked = self.sql(reverse, check=False)
        self.assertNotEqual(blocked.returncode, 0)
        self.assertIn("definitions require Provider tools", blocked.stderr)
        self.sql("TRUNCATE registry; INSERT INTO runs VALUES (" + literal({"binding_snapshot": {"bindings": [{"narrow": {"provider_tool": PINS[2]}}]}}) + ");")
        blocked = self.sql(reverse, check=False)
        self.assertNotEqual(blocked.returncode, 0)
        self.assertIn("Runs pin Provider tools", blocked.stderr)
        self.sql("TRUNCATE runs;")
        self.sql(reverse)
        self.assertEqual(self.signature(), self.before)
        self.assertFalse(self.valid("aidash_descriptor_is_valid", descriptor("datetime")))
        self.assertFalse(self.valid("aidash_agent_bindings_is_valid", agent(PINS[2])))
        self.sql((SQL / "forward/0050_provider_tools.sql").read_text())
        self.sql((SQL / "forward/0051_provider_warning_tools.sql").read_text())


if __name__ == "__main__":
    unittest.main()
