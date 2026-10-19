"""Exercise the Community bundle check against generated bundles.

Each bundle is written to a temporary directory, so that no tracked fixture
holds a rejected name.
"""

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("check-community-bundle.py")
SPEC = importlib.util.spec_from_file_location("community_bundle", SCRIPT)
BUNDLE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUNDLE)

ACCEPTED = {
    "index.html": '<script src="/assets/index.js"></script>',
    "assets/index.js": "export const app = () => 'Aidash';\n",
    "assets/index.js.map": json.dumps(
        {"version": 3, "sources": ["../../src/main.tsx", "../../node_modules/react/index.js"]}
    ),
    "assets/index.css": "body{margin:0}",
}


def write(directory: Path, files: dict[str, str]) -> Path:
    for name, content in files.items():
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    return directory


class CommunityBundleTests(unittest.TestCase):
    def check(self, files: dict[str, str], location: str = "web/dist") -> list[str]:
        """Check a bundle at `location` of a repository that holds enterprise/."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle = write(root / location, files)
            return BUNDLE.violations(bundle, root / "enterprise")

    def test_accepts_a_bundle_without_enterprise_code(self):
        self.assertEqual(self.check(ACCEPTED), [])

    def test_rejects_a_source_map_that_lists_an_enterprise_file(self):
        enterprise = json.dumps(
            {"version": 3, "sources": ["../../../enterprise/web/src/slots/sign-in.tsx"]}
        )
        problems = self.check({**ACCEPTED, "assets/index.js.map": enterprise})
        self.assertEqual(len(problems), 1)
        self.assertIn("under enterprise/", problems[0])

    def test_rejects_the_sources_of_a_bundle_built_inside_enterprise(self):
        own = json.dumps({"version": 3, "sources": ["../../src/slots/sign-in.tsx"]})
        files = {**ACCEPTED, "assets/index.js.map": own}
        problems = self.check(files, location="enterprise/web/dist")
        self.assertEqual(len(problems), 1)
        self.assertIn("under enterprise/", problems[0])

    def test_rejects_an_enterprise_source_written_with_backslashes(self):
        enterprise = json.dumps({"version": 3, "sources": ["..\\..\\..\\enterprise\\web\\src\\x.ts"]})
        self.assertEqual(len(self.check({**ACCEPTED, "assets/index.js.map": enterprise})), 1)

    def test_accepts_a_source_that_only_contains_the_word(self):
        sources = ["../../src/enterprise-notice.tsx", "../../node_modules/enterprises/index.js"]
        files = {**ACCEPTED, "assets/index.js.map": json.dumps({"version": 3, "sources": sources})}
        self.assertEqual(self.check(files), [])

    def test_rejects_a_bundle_that_contains_a_gcip_library(self):
        for name in ("firebase", "Firebase", "gcip", "identitytoolkit.googleapis.com"):
            with self.subTest(name=name):
                problems = self.check({**ACCEPTED, "assets/chunk.js": f"fetch('{name}')"})
                self.assertEqual(len(problems), 1)
                self.assertIn("GCIP name", problems[0])

    def test_rejects_a_gcip_name_in_any_file_type(self):
        for suffix in ("css", "html", "json", "mjs"):
            with self.subTest(suffix=suffix):
                problems = self.check({**ACCEPTED, f"assets/other.{suffix}": "gcip"})
                self.assertEqual(len(problems), 1)

    def test_refuses_a_bundle_without_source_maps(self):
        problems = self.check({name: text for name, text in ACCEPTED.items() if name != "assets/index.js.map"})
        self.assertEqual(len(problems), 1)
        self.assertIn("no source maps", problems[0])

    def test_refuses_an_unreadable_source_map(self):
        problems = self.check({**ACCEPTED, "assets/index.js.map": "not json"})
        self.assertEqual(len(problems), 1)
        self.assertIn("unreadable source map", problems[0])

    def test_refuses_a_missing_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            problems = BUNDLE.violations(Path(directory) / "dist", Path(directory) / "enterprise")
        self.assertEqual(len(problems), 1)
        self.assertIn("not a directory", problems[0])

    def test_command_exit_codes(self):
        def run(files: dict[str, str]) -> int:
            with tempfile.TemporaryDirectory() as directory:
                write(Path(directory), files)
                return subprocess.run(
                    [sys.executable, str(SCRIPT), directory], capture_output=True
                ).returncode

        self.assertEqual(run(ACCEPTED), 0)
        self.assertEqual(run({**ACCEPTED, "assets/chunk.js": "firebase"}), 1)
        self.assertEqual(subprocess.run([sys.executable, str(SCRIPT)], capture_output=True).returncode, 2)


if __name__ == "__main__":
    unittest.main()
