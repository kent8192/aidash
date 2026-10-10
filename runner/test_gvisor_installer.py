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
        drain = Mock()
        for text in ('version = 3\n', 'version = 2\n[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc]\nruntime_type="other"\n'):
            config.write_text(text)
            with self.assertRaises(ValueError):
                installer.install(self.root, installer.GVISOR_VERSION, self.sha, 'x86_64',
                                  self.download, self.restart, drain=drain)
            self.assertEqual(config.read_text(), text)
            # Refused before anything is fenced, drained or written.
            self.assertFalse((self.root / 'usr/local/bin' / installer.SHIM).exists())
        drain.assert_not_called()
        self.restart.assert_not_called()

    def test_runsc_root_change_fences_and_drains_before_reconfiguring(self):
        self.install()
        shim = self.root / 'usr/local/bin' / installer.SHIM
        config = self.root / 'etc/containerd/runsc.toml'
        before = config.read_text()

        def blocked(root):
            self.assertEqual(shim.stat().st_mode & 0o777, 0o644)
            raise RuntimeError('live gVisor sandboxes')

        with self.assertRaisesRegex(RuntimeError, 'live gVisor'):
            installer.install(self.root, installer.GVISOR_VERSION, self.sha, 'x86_64',
                              self.download, self.restart, '/run/other', drain=blocked)
        # Live sandboxes keep the root the guard reaches them through.
        self.assertEqual(config.read_text(), before)
        self.assertEqual(self.restart.call_count, 1)
        self.assertEqual(shim.stat().st_mode & 0o777, 0o644)
        drained = Mock()
        installer.install(self.root, installer.GVISOR_VERSION, self.sha, 'x86_64',
                          self.download, self.restart, '/run/other', drain=drained)
        drained.assert_called_once()
        self.assertIn('root = "/run/other"', config.read_text())
        self.assertEqual(self.restart.call_count, 2)
        self.assertEqual(shim.stat().st_mode & 0o777, 0o755)
        # An unchanged installation neither fences nor drains.
        unchanged = Mock()
        installer.install(self.root, installer.GVISOR_VERSION, self.sha, 'x86_64',
                          self.download, self.restart, '/run/other', drain=unchanged)
        unchanged.assert_not_called()

    def test_shim_stays_fenced_until_every_file_and_containerd_restart_complete(self):
        shim = self.root / 'usr/local/bin' / installer.SHIM
        observed, real = [], installer.write

        def executable():
            return shim.exists() and bool(shim.stat().st_mode & 0o111)

        def write(path, data, mode=0o644):
            changed = real(path, data, mode)
            observed.append((path.name, executable()))
            return changed

        restart = Mock(side_effect=lambda: observed.append(('restart', executable())))
        with patch.object(installer, 'write', write):
            installer.install(self.root, installer.GVISOR_VERSION, self.sha, 'x86_64',
                              self.download, restart)
        # The archive lists the shim first; it is still never executable before the end.
        self.assertEqual(observed[0], (installer.SHIM, False))
        self.assertIn(('restart', False), observed)
        self.assertFalse(any(state for _, state in observed))
        self.assertEqual(shim.stat().st_mode & 0o777, 0o755)
        self.assertTrue(installer.installed(self.root, installer.GVISOR_VERSION, self.sha))

    def test_temporary_files_are_unique_and_never_left_behind(self):
        binaries = self.root / 'usr/local/bin'
        binaries.mkdir(parents=True)
        # Another writer's in-progress file must be neither replaced nor reused.
        (binaries / 'runsc.new').write_bytes(b'other')
        self.install()
        self.assertEqual((binaries / 'runsc.new').read_bytes(), b'other')
        leftovers = [path.name for path in self.root.rglob('*.new') if path != binaries / 'runsc.new']
        self.assertEqual(leftovers, [])

    def assert_locked(self):
        fd = os.open(self.root / installer.LOCK, os.O_RDWR)
        try:
            with self.assertRaises(BlockingIOError):
                installer.fcntl.flock(fd, installer.fcntl.LOCK_EX | installer.fcntl.LOCK_NB)
        finally:
            os.close(fd)

    def test_node_lock_excludes_other_installers_until_released(self):
        with installer.node_lock(self.root):
            self.assert_locked()
        with installer.node_lock(self.root):
            pass

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
        shim = self.root / 'usr/local/bin/containerd-shim-runsc-v1'
        replacement = archive(content=b'newer!')
        sha = hashlib.sha256(replacement).hexdigest()
        modes = []

        def blocked(root):
            modes.append(shim.stat().st_mode & 0o777)
            raise RuntimeError('live gVisor sandboxes')

        with self.assertRaisesRegex(RuntimeError, 'live gVisor'):
            installer.install(self.root, 'next', sha, 'x86_64', Mock(return_value=replacement),
                              self.restart, drain=blocked)
        # Already-bound Pods cannot start a sandbox while the drain observes.
        self.assertEqual(modes, [0o644])
        self.assertEqual(sentry.read_bytes(), b'binary')
        self.assertEqual(sentry.stat().st_mode & 0o777, 0o755)
        self.assertEqual(shim.stat().st_mode & 0o777, 0o644)
        # A failed drain stays fenced and is not mistaken for an installed runtime.
        self.assertFalse(installer.installed(self.root, installer.GVISOR_VERSION, self.sha))
        installer.install(self.root, 'next', sha, 'x86_64', Mock(return_value=replacement),
                          self.restart, drain=lambda root: None)
        self.assertEqual(sentry.read_bytes(), b'newer!')
        self.assertEqual(shim.stat().st_mode & 0o777, 0o755)

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
        # The drain also waits for shims: one started before the fence can still launch a Sentry.
        self.assertEqual(sorted(installer.live_sandboxes(self.root, proc)), [101, 102, 103])

    def test_drain_waits_confirms_then_fails_closed_after_its_deadline(self):
        now, sleeps = [0.0], []

        def sleep(seconds):
            sleeps.append(seconds)
            now[0] += seconds

        # An empty scan is confirmed by a second one; a late launch restarts the wait.
        waiting = iter([[7], [], [8], [], []])
        installer.drain(self.root, 60, sleep=sleep, clock=lambda: now[0], live=lambda root: next(waiting))
        self.assertEqual(sleeps, [5, 5, 5, 5])
        with self.assertRaises(StopIteration):
            next(waiting)
        with self.assertRaisesRegex(RuntimeError, r'\[7\]'):
            installer.drain(self.root, 12, sleep=sleep, clock=lambda: now[0], live=lambda root: [7])

    def run_main(self, install):
        calls = []
        environment = {'AIDASH_HOST_ROOT': str(self.root), 'AIDASH_GVISOR_LABEL': 'x/admit',
                       'AIDASH_GVISOR_INSTALLED_LABEL': 'x/installed',
                       'AIDASH_GVISOR_READY': str(self.root / 'ready')}

        def idle(seconds):
            # Once admission is published the lock is free for other installers.
            fd = os.open(self.root / installer.LOCK, os.O_RDWR)
            try:
                installer.fcntl.flock(fd, installer.fcntl.LOCK_EX | installer.fcntl.LOCK_NB)
            finally:
                os.close(fd)
            raise StopIteration

        with (patch.object(installer, 'label', side_effect=calls.append),
              patch.object(installer, 'install', install),
              patch.object(installer.time, 'sleep', side_effect=idle),
              patch.object(installer.os, 'uname', return_value=Mock(machine='x86_64')),
              patch.dict(os.environ, environment)):
            try:
                installer.main()
            except (StopIteration, RuntimeError) as error:
                return calls, error
        return calls, None

    def test_replacement_withdraws_only_admission_and_keeps_the_guard_scheduled(self):
        calls, error = self.run_main(Mock(side_effect=lambda *args, **kwargs: self.assert_locked()))
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
