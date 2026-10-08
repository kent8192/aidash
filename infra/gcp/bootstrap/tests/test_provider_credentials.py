"""Human-run bootstrap owns BYOK roles; broker identity grants remain deferred."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[1]
class ByokBootstrapTests(unittest.TestCase):
    def test_explicit_project_and_audited_write_only_boundary(self):
        variables = (ROOT / "variables.tf").read_text()
        self.assertIn('variable "byok_project_id"', variables)
        self.assertIn('var.byok_project_id != var.project_id', variables)
        source = (ROOT / "provider_credentials.tf").read_text()
        self.assertNotIn('roles/owner', source)
        self.assertNotIn('resource "google_project"', source)
        self.assertIn('"DATA_READ"', source)
        self.assertIn('"DATA_WRITE"', source)
