"""The sandbox passes only allowlisted variables through, including what uv needs."""
import ast
import unittest
from pathlib import Path

SANDBOX = Path(__file__).with_name("sandbox.py")


def environment_allowlist():
    # sandbox.py serves a request at import time, so read its allowlist statically.
    tree = ast.parse(SANDBOX.read_text(encoding="utf-8"))
    for node in ast.walk(tree):
        if isinstance(node, ast.Set):
            names = {item.value for item in node.elts if isinstance(item, ast.Constant)}
            if "PATH" in names:
                return names
    raise AssertionError("environment allowlist not found in sandbox.py")


class SandboxEnvironmentTest(unittest.TestCase):
    def test_allowlist_passes_uv_python_selection(self):
        allowlist = environment_allowlist()
        self.assertTrue({"UV_PYTHON_DOWNLOADS", "UV_PYTHON"} <= allowlist)

    def test_allowlist_excludes_credentials(self):
        for name in environment_allowlist():
            for marker in ("TOKEN", "SECRET", "PASSWORD", "KEY", "CREDENTIAL"):
                self.assertNotIn(marker, name)


if __name__ == "__main__":
    unittest.main()
