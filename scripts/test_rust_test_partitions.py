"""Verify partitioned execution and coverage of shared production dependencies."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "rust_test_partitions", Path(__file__).with_name("rust-test-partitions.py")
)
PARTITIONS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PARTITIONS)


class RustPartitionCoverageTests(unittest.TestCase):
    def test_server_partition_records_dependency_hits_without_running_its_tests(self):
        # Arrange: the server exercises both branches of a production dependency.
        with tempfile.TemporaryDirectory(prefix="aidash-coverage-contract-", dir="/tmp") as temporary:
            root = Path(temporary).resolve()
            sources = {
                "Cargo.toml": '[workspace]\nmembers = ["business", "server"]\nresolver = "3"\n',
                "business/Cargo.toml": '[package]\nname = "aidash-domain"\nversion = "0.0.0"\nedition = "2024"\n',
                "business/src/lib.rs": """pub fn permitted(value: u8) -> bool {
    if value == 7 {
        true
    } else {
        false
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn must_remain_in_the_foundation_partition() {
        panic!("a server partition must not execute dependency unit tests");
    }
}
""",
                "server/Cargo.toml": """[package]
name = "aidash-server"
version = "0.0.0"
edition = "2024"
[dependencies]
aidash-domain = { path = "../business" }
[[test]]
name = "authorization"
path = "src/apps/identity/tests/authorization.rs"
""",
                "server/src/lib.rs": "pub fn permitted(value: u8) -> bool { aidash_domain::permitted(value) }\n",
                "server/src/apps/identity/tests/authorization.rs": """#[test]
fn authorization_accepts_current_authority_and_rejects_other_values() {
    assert!(aidash_server::permitted(7));
    assert!(!aidash_server::permitted(0));
}
""",
            }
            # Keep the real inventory contract: all eight partitions are nonempty.
            # These targets are not selected by the identity partition under test.
            for app, target in (
                ("execution", "worker"),
                ("execution", "migrations"),
                ("workspaces", "collaboration"),
                ("federation", "federation"),
                ("knowledge", "knowledge"),
            ):
                source = f"src/apps/{app}/tests/{target}.rs"
                sources["server/Cargo.toml"] += (
                    f'\n[[test]]\nname = "{target}"\npath = "{source}"\n'
                )
                sources[f"server/{source}"] = (
                    '#[test]\nfn must_remain_in_its_own_partition() {\n'
                    '    panic!("an identity partition must not execute unrelated targets");\n'
                    '}\n'
                )
            for name, contents in sources.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(contents)
            env = os.environ.copy()
            env.update(
                CARGO_TARGET_DIR=str(root / "target"),
                CARGO_BUILD_BUILD_DIR=str(root / "build"),
                CARGO_INCREMENTAL="0",
                RUSTC_WRAPPER="",
            )
            subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=root, env=env, check=True, capture_output=True)
            metadata = json.loads(subprocess.check_output(
                ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=root, env=env,
            ))
            inventory = PARTITIONS.inventory(metadata, root)
            reports = {}
            # Act: compare identical source and test inputs with the old and new selection.
            for mode in (False, True):
                report = root / ("workspace.lcov" if mode else "package.lcov")
                arguments = PARTITIONS.command_arguments(metadata, inventory, "identity", coverage=mode)
                completed = subprocess.run(
                    ["cargo", "llvm-cov", "--locked", *arguments, "--lcov", "--output-path", str(report)],
                    cwd=root, env=env, capture_output=True, text=True, timeout=120,
                )
                self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
                reports[mode] = report.read_text()
            # Assert: the dependency was omitted before and now has actual execution hits.
            dependency = "business/src/lib.rs"
            old_records = [record for record in reports[False].split("end_of_record") if dependency in record]
            new_records = [record for record in reports[True].split("end_of_record") if dependency in record]
            self.assertEqual(old_records, [])
            self.assertEqual(len(new_records), 1)
            hits = [int(line.split(",")[1]) for line in new_records[0].splitlines() if line.startswith("DA:")]
            self.assertTrue(hits)
            self.assertTrue(all(count > 0 for count in hits))


if __name__ == "__main__":
    unittest.main()
