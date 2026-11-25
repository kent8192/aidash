"""Verify the E2E-only coverage gate counts only E2E hits over the full denominator."""

import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "e2e_coverage_gate", Path(__file__).with_name("e2e-coverage-gate.py")
)
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)

CONFIG = {
    "threshold": 50.0,
    "target": 90.0,
    "unit_partitions": ["foundation", "server-unit"],
    "exclude": [{"glob": "crates/**/test_support.rs", "reason": "fakes"}],
}


def record(source, lines):
    body = "".join(f"DA:{number},{count}\n" for number, count in lines)
    return f"SF:{source}\n{body}end_of_record\n"


class E2ECoverageGateTests(unittest.TestCase):
    def measure(self, files, config=CONFIG):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, text in files.items():
                (root / name).write_text(text)
            return GATE.measure(root, config, root=Path("/repo"))

    def test_unit_hits_count_in_the_denominator_but_not_as_covered(self):
        # Arrange: line 1 only a unit test reaches, line 2 only an E2E test.
        files = {
            "rust-foundation-1.lcov": record("/repo/crates/aidash-domain/src/a.rs", [(1, 3), (2, 0)]),
            "rust-identity-1.lcov": record("/repo/crates/aidash-domain/src/a.rs", [(1, 0), (2, 5)]),
        }

        # Act
        covered, total, _ = self.measure(files)

        # Assert
        self.assertEqual((covered, total), (1, 2))

    def test_files_only_unit_tests_load_still_count_as_uncovered(self):
        # Arrange
        files = {
            "rust-server-unit-1.lcov": record("/repo/server/src/apps/x/y.rs", [(1, 1), (2, 1)]),
            "rust-execution-2.lcov": record("/repo/crates/aidash-harness/src/b.rs", [(7, 1)]),
        }

        # Act
        covered, total, _ = self.measure(files)

        # Assert
        self.assertEqual((covered, total), (1, 3))

    def test_tests_migrations_and_excluded_globs_leave_the_denominator(self):
        # Arrange
        files = {
            "rust-identity-1.lcov": "".join([
                record("/repo/crates/aidash-application/src/test_support.rs", [(1, 0)]),
                record("/repo/server/src/apps/identity/tests/flow.rs", [(1, 0)]),
                record("/repo/server/migrations/0001.rs", [(1, 0)]),
                record("/repo/server/src/apps/identity/services/tests.rs", [(1, 0)]),
                record("/repo/server/src/http.rs", [(1, 1)]),
            ]),
        }

        # Act
        covered, total, _ = self.measure(files)

        # Assert
        self.assertEqual((covered, total), (1, 1))

    def test_cfg_test_items_leave_the_denominator(self):
        # Arrange: a production file with an inline test module and a test-only fn.
        source = "\n".join([
            "pub fn live() -> u8 {",          # 1
            "    7",                           # 2
            "}",                               # 3
            "#[cfg(any(test, feature = \"test-support\"))]",  # 4
            "pub fn fake() -> &'static str {",  # 5
            "    \"{\"",                       # 6: a brace inside a string
            "}",                               # 7
            "#[cfg(test)]",                    # 8
            "mod tests {",                     # 9
            "    #[test]",                     # 10
            "    fn t() { assert!(true); }",   # 11
            "}",                               # 12
            "#[cfg(test)]",                    # 13
            "mod outline;",                    # 14
            "pub fn after() {}",               # 15
        ])

        # Act
        skipped = GATE.test_only_lines(source)

        # Assert
        self.assertEqual(skipped, {4, 5, 6, 7, 8, 9, 10, 11, 12})

    def test_a_run_without_e2e_partitions_is_refused(self):
        # Arrange
        files = {"rust-foundation-1.lcov": record("/repo/crates/aidash-domain/src/a.rs", [(1, 1)])}

        # Act / Assert
        with self.assertRaisesRegex(ValueError, "No E2E partition"):
            self.measure(files)

    def test_the_gate_fails_below_the_threshold(self):
        # Arrange: 1 of 4 lines (25%) against a 50% floor.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "rust-identity-1.lcov").write_text(
                record(f"{GATE.ROOT}/crates/aidash-domain/src/a.rs", [(1, 1), (2, 0), (3, 0), (4, 0)])
            )
            config = root / "gate.toml"
            config.write_text('threshold = 50.0\ntarget = 90.0\nunit_partitions = ["foundation"]\n')

            # Act
            status = GATE.main([str(root), "--config", str(config)])

        # Assert
        self.assertEqual(status, 1)


if __name__ == "__main__":
    unittest.main()
