"""Contracts for stable, opaque Scratch Space resource identities."""
import unittest

from control import scratch_resource_name


class ScratchResourceNameTest(unittest.TestCase):
    def test_arbitrary_ids_map_to_stable_dns_labels_without_disclosure(self):
        for scratch_id in ('customer/name', '日本語 🐈', 'x' * 10000):
            with self.subTest(scratch_id=scratch_id[:20]):
                name = scratch_resource_name(scratch_id)
                self.assertRegex(name, r'^scratch-[0-9a-f]{48}$')
                self.assertEqual(name, scratch_resource_name(scratch_id))
                self.assertNotIn(scratch_id, name)
        self.assertNotEqual(scratch_resource_name('A'), scratch_resource_name('a'))

    def test_empty_or_non_string_id_is_rejected(self):
        for scratch_id in ('', None, 1):
            with self.subTest(scratch_id=scratch_id):
                with self.assertRaises(ValueError):
                    scratch_resource_name(scratch_id)
