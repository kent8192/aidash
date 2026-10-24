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
            root = Path(temporary).resolve() / "fixture with spaces"
            root.mkdir()
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
                "server/src/apps/identity/tests/authorization.rs": """fn record(name: &str) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create(true).append(true)
        .open(std::env::var("AIDASH_SHARD_TRACE").unwrap()).unwrap();
    file.write_all(format!("{name}\\n").as_bytes()).unwrap();
}
#[test]
fn authorization() {
    record("authorization");
    assert!(aidash_server::permitted(7));
}
#[test]
fn authorization_extended() {
    record("authorization_extended");
    assert!(!aidash_server::permitted(0));
}
mod parameterized {
    #[test]
    fn case_1() {
        super::record("parameterized::case_1");
        assert!(aidash_server::permitted(7));
    }
    #[test]
    fn case_2() {
        super::record("parameterized::case_2");
        assert!(!aidash_server::permitted(1));
    }
}
#[test]
#[ignore]
fn subprocess_helper() { panic!("ignored helpers must remain ignored"); }
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
                AIDASH_SHARD_TRACE=str(root / "unsharded.log"),
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

            # Act: use the real Cargo runner and coverage collector for both shards.
            # The runner path has no spaces; the script and fixture paths may have them.
            runner = Path(temporary) / "runner"
            runner.write_text('#!/bin/sh\nexec python3 "$AIDASH_RUST_TEST_SHARD_RUNNER" "$@"\n')
            runner.chmod(0o755)
            host = subprocess.check_output(["rustc", "-vV"], text=True)
            host = next(line.removeprefix("host: ") for line in host.splitlines() if line.startswith("host: "))
            key = f"CARGO_TARGET_{host.upper().replace('-', '_')}_RUNNER"
            env[key] = str(runner)
            env["AIDASH_RUST_TEST_SHARD_RUNNER"] = str(ROOT / "scripts/rust-test-shard.py")
            executed = []
            shard_hits = []
            for shard in PARTITIONS.SHARDS:
                env["AIDASH_RUST_TEST_SHARD"] = str(shard)
                trace = root / f"trace-{shard}.log"
                env["AIDASH_SHARD_TRACE"] = str(trace)
                report = root / f"shard-{shard}.lcov"
                completed = subprocess.run(
                    ["cargo", "llvm-cov", "--locked", *arguments, "--lcov", "--output-path", str(report)],
                    cwd=root, env=env, capture_output=True, text=True, timeout=120,
                )
                self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
                executed.append(trace.read_text().splitlines())
                records = [record for record in report.read_text().split("end_of_record") if dependency in record]
                self.assertEqual(len(records), 1)
                shard_hits.append({line.split(",")[0]: int(line.split(",")[1])
                                   for line in records[0].splitlines() if line.startswith("DA:")})
            # Assert: actual execution covers every ordinary case once, including
            # prefix-overlapping names and parameterized identities. Both reports
            # retain the exercised dependency; its panic unit test stays excluded.
            self.assertEqual(sorted(executed[0] + executed[1]), [
                "authorization", "authorization_extended", "parameterized::case_1", "parameterized::case_2",
            ])
            self.assertTrue(set(executed[0]).isdisjoint(executed[1]))
            self.assertTrue(all(shard_hits[0][line] + shard_hits[1][line] > 0 for line in shard_hits[0]))


if __name__ == "__main__":
    unittest.main()
