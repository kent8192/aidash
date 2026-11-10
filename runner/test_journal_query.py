"""Journal Query runs without a writable Working Area."""
import tempfile
import unittest

import test_serve


class JournalManifestTest(unittest.TestCase):
    def test_journal_query_cannot_write_the_working_or_reference_mounts(self):
        fixture = test_serve.ServeManifestTest()
        with tempfile.TemporaryDirectory() as root:
            manifest = fixture.runner(root).manifest(fixture.record("journal"))
        pod = manifest["spec"]
        worker = pod["containers"][0]
        mounts = {item["mountPath"]: item for item in worker["volumeMounts"]}
        self.assertTrue(mounts["/work"]["readOnly"])
        self.assertTrue(mounts["/references"]["readOnly"])
        self.assertFalse(mounts["/tmp"].get("readOnly", False))
        self.assertFalse(pod["hostNetwork"])
        self.assertEqual(pod["dnsConfig"]["nameservers"], ["127.0.0.1"])


if __name__ == "__main__":
    unittest.main()
