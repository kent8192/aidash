"""Remote-controller sizing must match the execution node's physical ceiling."""
import unittest
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
import uuid

from control import Runner, Rejected
from node_guard import filesystem_capacity
import node_guard


class ResourceLimitsTest(unittest.TestCase):
    def test_manifest_and_node_observation_agree_across_page_sizes(self):
        runner = object.__new__(Runner)
        runner.config = dict(cpu=2, memory_bytes=2 << 30, processes=128,
                             temporary_bytes=256 << 20, working_bytes=1 << 30,
                             image='fixture-image', output_bytes=8 << 20,
                             runtime_class='fixture-runtime')
        # These nonaligned read-only inputs failed real gVisor admission when
        # the macOS controller rounded to 16 KiB and the Linux node used 4 KiB.
        for mounted in (0, 24637, 130000, 6299709, (1 << 30) - 1):
            with self.subTest(mounted=mounted):
                record = dict(operation_id='fixture', area_id='fixture-area', epoch=1,
                              wire_digest='fixture-digest', request=dict(kind='shell', files=[
                                  dict(scope='references', size=mounted),
                                  dict(scope='working', size=1024)]))
                with patch('control.os.sysconf', side_effect=AssertionError('host page size')):
                    manifest = runner.manifest(record)
                    limits = runner.execution_limits(record)
                logical = (1 << 30) - mounted
                volume = next(item for item in manifest['spec']['volumes'] if item['name'] == 'work')
                self.assertEqual(volume['emptyDir']['sizeLimit'], str(logical))
                self.assertEqual(limits['working_bytes'], logical)
                observed = filesystem_capacity(logical, 4096)
                self.assertGreaterEqual(observed, logical)
                self.assertLess(observed - logical, 4096)
                self.assertEqual(observed % 4096, 0)
                self.assertEqual(limits['cpu'], 2)
                self.assertEqual(limits['memory_bytes'], 2 << 30)
                self.assertEqual(limits['processes'], 128)
                self.assertEqual(limits['host_tasks'], 512)

    def test_guard_writes_separate_host_ceiling_and_refuses_lower_ancestor_limit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cgroups = root / 'cgroup'
            current = cgroups / 'sentry'
            current.mkdir(parents=True)
            for file, value in {'cpu.max': '200000 100000', 'memory.max': str(2 << 30),
                                'pids.max': 'max', 'memory.swap.max': 'max'}.items():
                (current / file).write_text(value)
            request = dict(action='limits', area_id=str(uuid.uuid4()), pod_uid=str(uuid.uuid4()),
                           container_id='a' * 64, epoch=1, image='fixture', cpu=2,
                           memory_bytes=2 << 30, host_tasks=512, processes=128,
                           working_bytes=4096, temporary_bytes=4096)
            with patch.object(node_guard, 'ROOT', root / 'guard'), \
                 patch.object(node_guard, 'CGROUP_ROOT', cgroups), \
                 patch.object(node_guard, 'sentry_cgroup', return_value=current), \
                 patch.object(node_guard, 'validate', return_value=('a' * 64, request['pod_uid'], request['area_id'], {'pid': 42})), \
                 patch.object(node_guard, 'binding', return_value={}), \
                 patch.object(node_guard.os, 'statvfs', return_value=SimpleNamespace(f_blocks=1, f_frsize=4096)), \
                 patch.object(node_guard.os, 'sysconf', return_value=4096):
                evidence = node_guard.handle(request)
                self.assertEqual(evidence['host_tasks'], 512)
                self.assertEqual(evidence['processes'], 128)
                self.assertEqual((current / 'pids.max').read_text(), '512')
                self.assertEqual((current / 'memory.swap.max').read_text(), '0')
                (cgroups / 'pids.max').write_text('256')
                with self.assertRaisesRegex(ValueError, 'limits differ'):
                    node_guard.handle(request)
                for invalid in (0, -1, True, '512'):
                    with self.assertRaisesRegex(ValueError, 'positive host task'):
                        node_guard.handle(dict(request, host_tasks=invalid))

    def test_node_rounds_only_to_its_verified_page_boundary(self):
        for size, page, expected in ((1, 4096, 4096), (4096, 4096, 4096),
                                     (4097, 4096, 8192), (4097, 16384, 16384),
                                     (1073717187, 4096, 1073717248)):
            with self.subTest(size=size, page=page):
                self.assertEqual(filesystem_capacity(size, page), expected)

    def test_invalid_budgets_are_rejected_before_rounding(self):
        for size, page in ((0, 4096), (-1, 4096), (True, 4096), ('1', 4096),
                           (1.5, 4096), (1, 0), (1, -1), (1, True)):
            with self.subTest(size=size, page=page):
                with self.assertRaisesRegex(ValueError, 'positive filesystem byte budget'):
                    filesystem_capacity(size, page)

    def test_readonly_inputs_cannot_exhaust_the_writable_budget(self):
        runner = object.__new__(Runner)
        runner.config = dict(cpu=2, memory_bytes=2 << 30, processes=128,
                             temporary_bytes=256 << 20, working_bytes=4096)
        for size in (4096, 4097):
            with self.subTest(size=size):
                record = dict(request=dict(files=[dict(scope='references', size=size)]))
                with self.assertRaises(Rejected) as failure:
                    runner.execution_limits(record)
                self.assertEqual(failure.exception.code, 413)


if __name__ == '__main__':
    unittest.main()
