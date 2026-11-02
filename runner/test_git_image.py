"""Exercise Git in the built sandbox image as the execution user."""
import subprocess
import unittest
import os


@unittest.skipUnless(os.environ.get("AIDASH_TEST_GIT_IMAGE"), "requires a locally built sandbox image")
class GitImageTest(unittest.TestCase):
    def test_unprivileged_git_without_credential_helper(self):
        result = subprocess.run(
            ["docker", "run", "--rm", "--entrypoint", "sh", "-e", "HOME=/tmp/home",
             os.environ.get("AIDASH_TEST_GIT_IMAGE", ""), "-c",
             'set -eu; test "$(id -u)" = 10000; mkdir -p "$HOME"; '
             'git init -q /tmp/source; git clone -q /tmp/source /tmp/clone; '
             'test -z "$(git config --get credential.helper || true)"'],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
