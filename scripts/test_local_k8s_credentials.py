"""The kind bootstrap keeps encryption keys across repeated startup."""

import base64
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("local_credentials", Path(__file__).with_name("local-k8s-credentials.py"))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class LocalCredentialsTest(unittest.TestCase):
    def test_initialization_retains_unrelated_data_and_reuses_both_keys(self):
        secret = {"metadata": {"resourceVersion": "11"}, "data": {"DATABASE_URL": "Zml4dHVyZQ=="}}
        with patch.object(MODULE.subprocess, "run", side_effect=[
            subprocess.CompletedProcess([], 0, json.dumps(secret).encode()),
            subprocess.CompletedProcess([], 0),
        ]) as run:
            MODULE.initialize(["kubectl"])
        operations = json.loads(run.call_args.kwargs["input"])
        self.assertEqual(operations[0], {"op": "test", "path": "/metadata/resourceVersion", "value": "11"})
        self.assertEqual(len(operations), 3)
        for operation in operations[1:]:
            self.assertEqual(operation["op"], "add")
            key = operation["path"].removeprefix("/data/")
            self.assertIn(key, MODULE.KEYS)
            self.assertRegex(base64.b64decode(operation["value"]).decode(), r"^[0-9a-f]{64}$")
            secret["data"][key] = operation["value"]
        self.assertNotEqual(*(secret["data"][key] for key in MODULE.KEYS))
        self.assertEqual(secret["data"]["DATABASE_URL"], "Zml4dHVyZQ==")
        with patch.object(MODULE.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, json.dumps(secret).encode())) as run:
            MODULE.initialize(["kubectl"])
        self.assertEqual(run.call_count, 1, "existing keys must never be rewritten")

    def test_partial_or_invalid_keys_are_never_rotated(self):
        for data in ({MODULE.KEYS[0]: "Zml4dHVyZQ=="}, dict.fromkeys(MODULE.KEYS, "malformed!")):
            with self.subTest(data=data), patch.object(MODULE.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, json.dumps({"data": data}).encode())) as run:
                with self.assertRaisesRegex(RuntimeError, "incomplete|invalid"):
                    MODULE.initialize(["kubectl"])
                self.assertEqual(run.call_count, 1)


if __name__ == "__main__":
    unittest.main()
