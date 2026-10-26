#!/usr/bin/env python3
"""Unit tests for the Deployment Administrator terminology guard."""

import importlib.util
import pathlib
import unittest

SPEC = importlib.util.spec_from_file_location(
    "check_role_terminology", pathlib.Path(__file__).with_name("check-role-terminology.py")
)
guard = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(guard)


class ViolationsTest(unittest.TestCase):
    def test_role_uses_fail(self):
        lines = [
            "server/src/lib.rs:1:// An operator approves the request.",
            "web/src/main.tsx:2:  const isOperator = kind === 'operator';",
            "docs/a.md:3:Use the operator token.",
        ]
        self.assertEqual(list(guard.violations(lines)), lines)

    def test_unrelated_meanings_pass(self):
        lines = [
            "server/migrations/a.rs:1:operator_class: Some(\"vector_l2_ops\".into()),",
            "deploy/values.yaml:2:  - { key: aidash.run/metadata-server, operator: DoesNotExist }",
            "README.md:3:Provision using NATS operator credentials.",
            "enterprise/infra/delivery/README.md:4:mkdir -p secrets/delivery/operator",
            "crates/x/brave.rs:5:\t\t\t\"operators\": true,",
        ]
        self.assertEqual(list(guard.violations(lines)), [])

    def test_an_allowed_use_does_not_hide_a_role_use_on_the_same_line(self):
        line = "docs/a.md:1:The operator lists operator_class values."
        self.assertEqual(list(guard.violations([line])), [line])

    def test_allowed_files_are_skipped(self):
        line = "scripts/check-role-terminology.py:1:An operator approves."
        self.assertEqual(list(guard.violations([line])), [])

    def test_paths_naming_the_role_fail(self):
        self.assertEqual(
            list(guard.paths(["server/src/operator.rs", "server/src/deployment_administrator.rs"])),
            ["server/src/operator.rs"],
        )


if __name__ == "__main__":
    unittest.main()
