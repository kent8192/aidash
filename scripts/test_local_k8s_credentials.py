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
        operations = []
        patch_files = []

        def kubectl(command, **kwargs):
            if command[1] == "get":
                return subprocess.CompletedProcess(command, 0, json.dumps(secret).encode())
            patch_file = Path(command[command.index("--patch-file") + 1])
            patch_files.append(patch_file)
            self.assertEqual(patch_file.stat().st_mode & 0o777, 0o600)
            operations.extend(json.loads(patch_file.read_text()))
            return subprocess.CompletedProcess(command, 0)

        with patch.object(MODULE.subprocess, "run", side_effect=kubectl):
            MODULE.initialize(["kubectl"])
        self.assertFalse(patch_files[0].exists(), "the private patch file must be removed")
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

    def test_application_valid_key_formats_are_preserved(self):
        for fingerprint in ("raw-fingerprint-key-with-32-bytes!", "界" * 11, "  " + "x" * 32 + "\n"):
            with self.subTest(fingerprint=fingerprint):
                data = {
                    MODULE.KEYS[0]: base64.b64encode(fingerprint.encode()).decode(),
                    MODULE.KEYS[1]: base64.b64encode(("  " + "AB" * 32 + "\n").encode()).decode(),
                }
                with patch.object(MODULE.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, json.dumps({"data": data}).encode())) as run:
                    MODULE.initialize(["kubectl"])
                self.assertEqual(run.call_count, 1, "valid recovered keys must never be rewritten")

    def test_partial_or_invalid_keys_are_never_rotated(self):
        valid = {
            MODULE.KEYS[0]: base64.b64encode(b"x" * 32).decode(),
            MODULE.KEYS[1]: base64.b64encode(b"ab" * 32).decode(),
        }
        invalid_pairs = [{MODULE.KEYS[0]: "Zml4dHVyZQ=="}, dict.fromkeys(MODULE.KEYS, "malformed!")]
        for key, value in (
            (MODULE.KEYS[0], b"x" * 31),
            (MODULE.KEYS[0], ("界" * 10).encode()),
            (MODULE.KEYS[0], b" " + b"x" * 31 + b" "),
            (MODULE.KEYS[0], b"\xff" * 32),
            (MODULE.KEYS[1], b"g" * 64),
            (MODULE.KEYS[1], b"a" * 63),
        ):
            invalid_pairs.append({**valid, key: base64.b64encode(value).decode()})
        for data in invalid_pairs:
            with self.subTest(data=data), patch.object(MODULE.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, json.dumps({"data": data}).encode())) as run:
                with self.assertRaisesRegex(RuntimeError, "incomplete|invalid"):
                    MODULE.initialize(["kubectl"])
                self.assertEqual(run.call_count, 1)


if __name__ == "__main__":
    unittest.main()
