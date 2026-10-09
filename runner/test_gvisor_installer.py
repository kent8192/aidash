import hashlib
import io
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch

import gvisor_installer as installer


def archive(extra=None, content=b'binary'):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode='w:bz2') as tar:
        for name in sorted(installer.REQUIRED):
            member = tarfile.TarInfo(name)
            member.size = len(content)
            tar.addfile(member, io.BytesIO(content))
        if extra:
            tar.addfile(extra, io.BytesIO(b'x') if extra.isfile() else None)
    return output.getvalue()


class InstallerTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / 'etc/containerd').mkdir(parents=True)
        (self.root / 'etc/containerd/config.toml').write_text('version = 2\n')
        self.data = archive()
        self.sha = hashlib.sha256(self.data).hexdigest()
        self.download, self.restart = Mock(return_value=self.data), Mock()

    def install(self):
        installer.install(self.root, installer.GVISOR_VERSION, self.sha, 'x86_64',
                          self.download, self.restart)

    def test_idempotence_verifies_every_archive_binary_and_repairs_tampering(self):
        self.install()
        self.install()
        self.assertEqual(self.download.call_count, 1)
        self.assertEqual(self.restart.call_count, 1)
        self.assertTrue(installer.installed(self.root, installer.GVISOR_VERSION, self.sha))
        sentry = self.root / 'usr/local/bin/gvisor-bin/gvisor_sentry'
        sentry.write_bytes(b'tampered')
        self.install()
        self.assertEqual(sentry.read_bytes(), b'binary')
        self.assertEqual(self.download.call_count, 2)
        self.assertEqual(self.restart.call_count, 1)

    def test_swap_platform_and_root_configuration_repair_restarts_once(self):
        self.install()
        config = self.root / 'etc/containerd/runsc.toml'
        config.write_text('root = "wrong"\n')
        self.install()
        self.assertIn('platform = "systrap"', config.read_text())
        self.assertEqual(self.restart.call_count, 2)

    def test_incompatible_runtime_and_containerd_version_are_refused(self):
        config = self.root / 'etc/containerd/config.toml'
        for text in ('version = 3\n', 'version = 2\n[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc]\nruntime_type="other"\n'):
            config.write_text(text)
            with self.assertRaises(ValueError):
                self.install()
            self.assertEqual(config.read_text(), text)
        self.restart.assert_not_called()

    def test_checksum_and_unsafe_archive_members_fail_before_install(self):
        with self.assertRaisesRegex(ValueError, 'checksum'):
            installer.archive_files(self.data, '0' * 64)
        for name, kind in (('../escape', tarfile.REGTYPE), ('/absolute', tarfile.REGTYPE),
                           ('link', tarfile.SYMTYPE), ('hard', tarfile.LNKTYPE),
                           ('fifo', tarfile.FIFOTYPE), ('runsc', tarfile.REGTYPE)):
            with self.subTest(name=name):
                member = tarfile.TarInfo(name)
                member.type, member.size = kind, 1 if kind == tarfile.REGTYPE else 0
                data = archive(member)
                with self.assertRaises(ValueError):
                    installer.archive_files(data, hashlib.sha256(data).hexdigest())

    def test_symlink_destination_is_refused(self):
        (self.root / 'usr/local/bin').mkdir(parents=True)
        (self.root / 'usr/local/bin/gvisor-bin').symlink_to(self.root / 'etc')
        with self.assertRaisesRegex(ValueError, 'symlink'):
            self.install()
        self.assertFalse((self.root / 'usr/local/bin/runsc').exists())

    def test_replacement_is_refused_while_a_sandbox_uses_the_installed_runtime(self):
        self.install()
        sentry = self.root / 'usr/local/bin/gvisor-bin/gvisor_sentry'
        replacement = archive(content=b'newer!')
        sha = hashlib.sha256(replacement).hexdigest()

        def blocked(root):
            raise RuntimeError('live gVisor sandboxes')

        with self.assertRaisesRegex(RuntimeError, 'live gVisor'):
            installer.install(self.root, 'next', sha, 'x86_64', Mock(return_value=replacement),
                              self.restart, drain=blocked)
        self.assertEqual(sentry.read_bytes(), b'binary')
        installer.install(self.root, 'next', sha, 'x86_64', Mock(return_value=replacement),
                          self.restart, drain=lambda root: None)
        self.assertEqual(sentry.read_bytes(), b'newer!')

    def test_live_sentries_use_the_guards_executable_identity(self):
        self.install()
        proc = self.root / 'proc'
        for pid, target in (('101', 'usr/local/bin/gvisor-bin/gvisor_sentry'),
                            ('102', 'usr/local/bin/runsc'),
                            ('103', 'usr/local/bin/containerd-shim-runsc-v1'),
                            ('not-a-pid', 'usr/local/bin/runsc')):
            (proc / pid).mkdir(parents=True)
            (proc / pid / 'exe').symlink_to(self.root / target)
        (proc / '104').mkdir()  # exited: no readable executable
        self.assertEqual(sorted(installer.live_sentries(self.root, proc)), [101, 102])

    def test_drain_waits_then_fails_closed_after_its_deadline(self):
        now, sleeps = [0.0], []

        def sleep(seconds):
            sleeps.append(seconds)
            now[0] += seconds

        waiting = iter([[7], [7], []])
        installer.drain(self.root, 60, sleep=sleep, clock=lambda: now[0], live=lambda root: next(waiting))
        self.assertEqual(sleeps, [5, 5])
        with self.assertRaisesRegex(RuntimeError, r'\[7\]'):
            installer.drain(self.root, 12, sleep=sleep, clock=lambda: now[0], live=lambda root: [7])

    def run_main(self, install):
        calls = []
        environment = {'AIDASH_HOST_ROOT': str(self.root), 'AIDASH_GVISOR_LABEL': 'x/admit',
                       'AIDASH_GVISOR_INSTALLED_LABEL': 'x/installed',
                       'AIDASH_GVISOR_READY': str(self.root / 'ready')}
        with (patch.object(installer, 'label', side_effect=calls.append),
              patch.object(installer, 'install', install),
              patch.object(installer.time, 'sleep', side_effect=StopIteration),
              patch.object(installer.os, 'uname', return_value=Mock(machine='x86_64')),
              patch.dict(os.environ, environment)):
            try:
                installer.main()
            except (StopIteration, RuntimeError) as error:
                return calls, error
        return calls, None

    def test_replacement_withdraws_only_admission_and_keeps_the_guard_scheduled(self):
        calls, error = self.run_main(Mock())
        self.assertIsInstance(error, StopIteration)
        self.assertEqual(calls, [{'x/admit': None},
                                 {'x/admit': installer.GVISOR_VERSION, 'x/installed': 'true'}])
        self.assertTrue((self.root / 'ready').exists())

    def test_failed_replacement_stays_unadmitted_without_withdrawing_the_guard(self):
        calls, error = self.run_main(Mock(side_effect=RuntimeError('live sandboxes')))
        self.assertIsInstance(error, RuntimeError)
        self.assertEqual(calls, [{'x/admit': None}, {'x/admit': None}])
        self.assertFalse((self.root / 'ready').exists())


if __name__ == '__main__':
    unittest.main()
