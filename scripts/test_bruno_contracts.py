"""Regression checks for completeness and privacy of real-API evidence."""

import json
from collections import Counter
from contextlib import redirect_stderr, redirect_stdout
from copy import deepcopy
from io import StringIO
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from bruno_contracts import inventory

# The executable script uses a hyphen, so load its public evidence reducer explicitly.
import importlib.util

SPEC = importlib.util.spec_from_file_location(
    "bruno_api", Path(__file__).with_name("test-bruno-api.py")
)
API = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(API)

EXPORT_SPEC = importlib.util.spec_from_file_location(
    "bruno_export", Path(__file__).with_name("export-bruno-catalog.py")
)
EXPORT = importlib.util.module_from_spec(EXPORT_SPEC)
EXPORT_SPEC.loader.exec_module(EXPORT)


class NativeCatalogProjectionTests(unittest.TestCase):
    def test_registered_multi_method_wildcards_survive_without_public_operations(self):
        saved = "[INFO] /{<path:asset>} GET, HEAD frontend-asset -\n[INFO] /federation/internal POST internal -\n"
        metadata = [
            {"method": "GET", "path": "/{<path:asset>}", "name": "frontend-asset", "source": None,
             "json": False, "uuid_parameters": [], "query_parameters": []},
            {"method": "HEAD", "path": "/{<path:asset>}", "name": "frontend-asset-head", "source": None,
             "json": False, "uuid_parameters": [], "query_parameters": []},
            {"method": "POST", "path": "/federation/internal", "name": "internal", "source": "native-owner",
             "json": True, "uuid_parameters": [], "query_parameters": ["after"],
             "response": {"schema": {"type": "string"}, "definitions": {}}},
        ]
        projected = EXPORT.project(saved, {"paths": {}}, metadata)
        self.assertEqual(
            [(route["method"], route["path"], route["name"]) for route in projected],
            [("GET", "/{<path:asset>}", "frontend-asset"),
             ("HEAD", "/{<path:asset>}", "frontend-asset-head"),
             ("POST", "/federation/internal", "internal")],
        )
        self.assertTrue(projected[2]["json"])
        self.assertEqual(projected[2]["query_parameters"], ["after"])
        self.assertNotIn("response", projected[2])
        self.assertNotIn("responses", projected[2])
        reordered = EXPORT.project(saved, {"paths": {}}, list(reversed(metadata)))
        self.assertEqual(
            [(route["method"], route["path"], route["name"]) for route in reordered],
            [("POST", "/federation/internal", "internal"),
             ("HEAD", "/{<path:asset>}", "frontend-asset-head"),
             ("GET", "/{<path:asset>}", "frontend-asset")],
        )

    def typed_inputs(self):
        # Small saved-native-shaped documents. Response and request definitions
        # intentionally differ; no expected value is produced by the projector.
        document = {
            "paths": {"/api/records/{id}": {"post": {
                "operationId": "create-record",
                "parameters": [
                    {"in": "path", "name": "id", "schema": {"type": "string", "format": "uuid"}},
                    {"in": "query", "name": "include_deleted", "schema": {"type": "boolean"}},
                ],
                "requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Request_Record"}}}},
                "responses": {
                    "200": {"content": {"application/json": {"schema": {"type": "string"}}}},
                    "201": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Record"}}}},
                },
            }}},
            "components": {"schemas": {
                "Request_Record": {"type": "object", "properties": {"label": {"type": "string"}}},
                "Record": {"type": "object", "required": ["state", "link"], "properties": {
                    "state": {"type": "string", "enum": ["live", "deleted"]},
                    "link": {"$ref": "#/components/schemas/Link"},
                }},
                "Link": {"type": "object", "properties": {"next": {"$ref": "#/components/schemas/Record"}}},
                "Unused": {"type": "string"},
            }},
        }
        metadata = [{"method": "POST", "path": "/api/records/{id}", "name": "create-record",
                     "source": "reviewed-owner", "json": False, "uuid_parameters": [], "query_parameters": [],
                     "response": {"schema": {"type": "boolean"}, "definitions": {}}}]
        return "[INFO] /api/records/{id} POST create-record -", document, metadata

    def test_native_response_status_and_recursive_reference_closure_are_preserved(self):
        saved, document, metadata = self.typed_inputs()
        original = deepcopy(document)
        route, = EXPORT.project(saved, document, metadata)
        self.assertTrue(route["json"])
        self.assertEqual(route["uuid_parameters"], ["id"])
        self.assertEqual(route["query_parameters"], ["include_deleted"])
        self.assertEqual(route["responses"]["200"], {"schema": {"type": "string"}, "definitions": {}})
        response = route["responses"]["201"]
        self.assertEqual(response["schema"], {"$ref": "#/components/schemas/Record"})
        self.assertEqual(set(response["definitions"]), {"Record", "Link"})
        self.assertEqual(response["definitions"]["Record"]["required"], ["state", "link"])
        self.assertEqual(response["definitions"]["Link"]["properties"]["next"], {"$ref": "#/components/schemas/Record"})
        self.assertEqual(document, original)
        self.assertNotIn("response", route)

    def test_invalid_or_ambiguous_native_inputs_fail_explicitly(self):
        saved, document, metadata = self.typed_inputs()
        for bad in ("[INFO] no routes", saved + "\n" + saved,
                    "[INFO] /api/records/{id} ALL create-record -",
                    "[INFO] /api/records/{id} POST, POST create-record -"):
            with self.subTest(inventory=bad), self.assertRaises(ValueError):
                EXPORT.project(bad, document, metadata)
        with self.assertRaisesRegex(ValueError, "unannotated"):
            EXPORT.project(saved, document, [])
        with self.assertRaisesRegex(ValueError, "duplicate reviewed"):
            EXPORT.project(saved, document, metadata + metadata)
        unknown_ref = deepcopy(document)
        unknown_ref["components"]["schemas"].pop("Link")
        with self.assertRaisesRegex(ValueError, "unresolved native reference"):
            EXPORT.project(saved, unknown_ref, metadata)
        unknown_input = deepcopy(document)
        unknown_input["components"]["schemas"].pop("Request_Record")
        with self.assertRaisesRegex(ValueError, "unresolved native reference"):
            EXPORT.project(saved, unknown_input, metadata)
        duplicate_operation = deepcopy(document)
        duplicate_operation["paths"]["/api/records/{id}"]["POST"] = {"operationId": "create-record"}
        with self.assertRaisesRegex(ValueError, "ambiguous native operation"):
            EXPORT.project(saved, duplicate_operation, metadata)
        wrong_name = deepcopy(document)
        wrong_name["paths"]["/api/records/{id}"]["post"]["operationId"] = "different-handler"
        with self.assertRaisesRegex(ValueError, "operationId differs"):
            EXPORT.project(saved, wrong_name, metadata)
        extra_operation = deepcopy(document)
        extra_operation["paths"]["/api/extra"] = {"get": {"operationId": "extra"}}
        with self.assertRaisesRegex(ValueError, "absent from native inventory"):
            EXPORT.project(saved, extra_operation, metadata)
        with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
            json.loads('{"paths":{},"paths":{}}', object_pairs_hook=EXPORT.object_without_duplicates)

    def test_known_addition_still_requires_the_exact_native_identity(self):
        path = "/api/tenants/{tenant}/nodes/{node}/default"
        restore = "/api/tenants/{tenant}/nodes/{node}/restore"
        saved = ("[INFO] " + restore + " POST tenant-node-restore -\n"
                 "[INFO] " + path + " POST tenant-node-default -\n"
                 "[INFO] /health GET health -\n")
        document = {"paths": {
            path: {"post": {"operationId": "tenant-node-default", "responses": {}}},
            restore: {"post": {"operationId": "tenant-node-restore", "responses": {}}},
        }}
        metadata = [
            {"method": "GET", "path": "/health", "name": "health", "source": "reviewed-owner",
             "json": False, "uuid_parameters": [], "query_parameters": []},
            {"method": "DELETE", "path": "/removed", "name": "removed", "source": "old-owner",
             "json": False, "uuid_parameters": [], "query_parameters": []},
        ]
        projected = EXPORT.project(saved, document, metadata)
        self.assertEqual([(route["method"], route["path"]) for route in projected],
                         [("GET", "/health"), ("POST", path), ("POST", restore)])
        route = projected[1]
        self.assertEqual((route["method"], route["path"], route["name"]),
                         ("POST", path, "tenant-node-default"))
        self.assertEqual(route["source"], "server/src/apps/identity/views/nodes.rs")
        self.assertNotIn("expected_status", route)
        with self.assertRaisesRegex(ValueError, "unannotated"):
            EXPORT.project(saved.replace("tenant-node-default", "unknown-default"), document, metadata)

    def test_legacy_name_migration_requires_both_native_exports_to_agree(self):
        path = "/api/tasks/{task}/remote-grants/{grant}/human-requests/answer"
        metadata = [{"method": "POST", "path": path, "name": "remote-human-answer", "source": "reviewed-owner"}]
        saved = "[INFO] " + path + " POST remote_human_answer -"
        document = {"paths": {path: {"post": {"operationId": "remote_human_answer"}}}}
        route, = EXPORT.project(saved, document, metadata)
        self.assertEqual(route["name"], "remote_human_answer")
        self.assertEqual(route["source"], "reviewed-owner")
        with self.assertRaisesRegex(ValueError, "operationId differs"):
            EXPORT.project(saved, {"paths": {path: {"post": {"operationId": "remote-human-answer"}}}}, metadata)
        with self.assertRaisesRegex(ValueError, "operation name differs"):
            EXPORT.project(saved.replace("remote_human_answer", "unknown_answer"), document, metadata)

    def test_cli_check_allows_formatting_but_rejects_changed_native_structure(self):
        route = {"method": "GET", "path": "/health", "name": "health", "source": "reviewed-owner",
                 "json": False, "uuid_parameters": [], "query_parameters": []}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "showurls").write_text("[INFO] /health GET health -\n")
            (root / "openapi.json").write_text('{"paths":{}}')
            (root / "metadata.json").write_text(json.dumps([route]))
            expected = root / "catalog.json"
            expected.write_text(json.dumps([route], indent=4) + "\n")
            argv = ["export-bruno-catalog.py", "--showurls", str(root / "showurls"),
                    "--openapi", str(root / "openapi.json"), "--metadata", str(root / "metadata.json"),
                    "--check", str(expected)]
            before = expected.read_bytes()
            with patch("sys.argv", argv), redirect_stdout(StringIO()):
                EXPORT.main()
            self.assertEqual(expected.read_bytes(), before, "check mode never writes")
            for changed in (True, 0):
                expected.write_text(json.dumps([{**route, "json": changed}]))
                with self.subTest(changed=changed), patch("sys.argv", argv), redirect_stderr(StringIO()):
                    with self.assertRaises(SystemExit) as failure:
                        EXPORT.main()
                    self.assertEqual(failure.exception.code, 1)


class ContractEvidenceTests(unittest.TestCase):
    def test_inventory_failure_reports_exact_missing_and_extra_multiplicities(self):
        self.assertEqual(
            API.inventory_difference(
                Counter({("GET", "/retained"): 1, ("HEAD", "/asset"): 1}),
                Counter({("GET", "/retained"): 1, ("POST", "/new"): 2}),
            ),
            {"missing_from_native": [{"method": "HEAD", "path": "/asset", "count": 1}],
             "extra_in_native": [{"method": "POST", "path": "/new", "count": 2}]},
        )

    def test_native_inventory_retains_methods_and_wildcards(self):
        actual = inventory(
            "[INFO] /api/tasks GET task-list -\n[INFO] /{<path:asset>} GET, HEAD frontend -\n[INFO] Total: 3"
        )
        self.assertEqual(
            dict(actual),
            {
                ("GET", "/api/tasks"): 1,
                ("GET", "/{<path:asset>}"): 1,
                ("HEAD", "/{<path:asset>}"): 1,
            },
        )

    def evidence(self, duplicate=False, skipped=False, missing_check=False):
        scenarios = [
            {
                "name": f"case-{endpoint}-{scenario}",
                "endpoint": f"GET /endpoint-{endpoint}",
                "required_checks": ["response contract"],
            }
            for endpoint in range(273)
            for scenario in range(3)
        ]
        results = [
            {
                "name": case["name"],
                "response": {
                    "status": 200,
                    "headers": {"Authorization": "private-value"},
                    "data": "private-value",
                },
                "testResults": [{"description": "response contract", "status": "pass"}],
            }
            for case in scenarios
        ]
        if duplicate:
            results[-1] = results[0].copy()
        if skipped:
            results[0]["skipped"] = True
        if missing_check:
            scenarios[0]["required_checks"].append("mutation preserved state")
        with tempfile.TemporaryDirectory(dir="/tmp") as directory:
            path = Path(directory) / "report.json"
            path.write_text(json.dumps([{"results": results}]))
            return API.summarize(path, [case["name"] for case in scenarios], scenarios)

    def test_complete_evidence_requires_every_scenario_exactly_once(self):
        self.assertTrue(self.evidence()["passed"])
        self.assertFalse(self.evidence(duplicate=True)["complete"])
        self.assertFalse(self.evidence(skipped=True)["passed"])
        self.assertFalse(self.evidence(missing_check=True)["passed"])

    def test_published_evidence_never_retains_response_values(self):
        self.assertNotIn("private-value", json.dumps(self.evidence()))


if __name__ == "__main__":
    unittest.main()
