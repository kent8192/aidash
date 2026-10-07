"""The schema verifier must reject incomplete or substituted native ledgers."""

import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "migration_schema", Path(__file__).with_name("test-migration-schema.py")
)
assert spec is not None and spec.loader is not None
schema = importlib.util.module_from_spec(spec)
spec.loader.exec_module(schema)


class MigrationSchemaTest(unittest.TestCase):
    def test_new_sources_extend_the_required_ledger_without_a_fixed_count(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app = root / "server/migrations/knowledge"
            app.mkdir(parents=True)
            with patch.object(schema, "ROOT", root):
                for name in ["0001_initial", "0002_native_memory", "0003_embeddings"]:
                    (app / (name + ".rs")).write_text(
                        '// reinhardt-migration-source: 1\nMigration::new("'
                        + name
                        + '", "knowledge")'
                    )
                expected = schema.migration_identities()
                applied = [
                    {"app": "knowledge", "name": "0001_initial"},
                    {"app": "knowledge", "name": "0002_native_memory"},
                ]
                self.assertFalse(schema.validate_ledger(applied, expected))
                applied.append({"app": "knowledge", "name": "0003_embeddings"})
                self.assertTrue(schema.validate_ledger(applied, expected))
                # Equal counts cannot conceal an unapplied migration.
                applied[-1]["name"] = "unrelated_migration"
                self.assertFalse(schema.validate_ledger(applied, expected))

    def test_duplicate_applied_rows_are_not_an_exact_history(self):
        record = {"app": "knowledge", "name": "0001_initial"}
        self.assertFalse(
            schema.validate_ledger([record, record], {("knowledge", "0001_initial")})
        )

    def test_source_registration_is_checked_before_database_comparison(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app = root / "server/migrations/knowledge"
            app.mkdir(parents=True)
            with patch.object(schema, "ROOT", root):
                first = app / "0001_initial.rs"
                first.write_text(
                    '// reinhardt-migration-source: 1\nMigration::new("0001_initial", "knowledge")'
                )
                second = app / "0002_next.rs"
                second.write_text(first.read_text())
                with self.assertRaisesRegex(RuntimeError, "duplicate native migration"):
                    schema.migration_identities()
                second.write_text("// reinhardt-migration-source: 1\n")
                with self.assertRaisesRegex(RuntimeError, "invalid native migration"):
                    schema.migration_identities()


if __name__ == "__main__":
    unittest.main()
