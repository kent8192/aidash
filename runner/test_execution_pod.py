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


class CommandQueueTest(unittest.TestCase):
    def test_queued_foreground_cancellation_and_concurrency_limit(self):
        import threading
        import time
        from execution_pod import CommandQueue

        started = []
        release = threading.Event()
        queue = CommandQueue()

        def run(command):
            started.append(command)
            release.wait(3)
            return command

        try:
            futures = [queue.submit('run-a', str(i), run) for i in range(8)]
            deadline = time.monotonic() + 2
            while len(started) < 8 and time.monotonic() < deadline:
                time.sleep(.01)
            self.assertEqual(len(started), 8)
            waiting = queue.submit('run-b', 'waiting', run)
            queue.cancel_waiting('run-b')
            self.assertTrue(waiting.cancelled())
            release.set()
            self.assertEqual([f.result(timeout=3) for f in futures], [str(i) for i in range(8)])
            self.assertNotIn('waiting', started)
        finally:
            release.set()
            queue.close()

    def test_cancelling_run_preserves_queued_background_work(self):
        import threading
        from execution_pod import CommandQueue

        release = threading.Event()
        queue = CommandQueue()
        try:
            running = [queue.submit('run-a', i, lambda value: (release.wait(3), value)[1])
                       for i in range(8)]
            background = queue.submit('run-b', 'background', lambda value: value, background=True)
            queue.cancel_waiting('run-b')
            self.assertFalse(background.cancelled())
            release.set()
            self.assertEqual(background.result(timeout=3), 'background')
            for future in running:
                future.result(timeout=3)
        finally:
            release.set()
            queue.close()


class SystemTreeTest(unittest.TestCase):
    def test_records_added_modified_removed_and_does_not_scan_scratch(self):
        import tempfile
        from pathlib import Path
        from execution_pod import system_tree_index, system_tree_diff

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'etc').mkdir()
            (root / 'scratch').mkdir()
            (root / 'etc' / 'edited').write_text('old')
            (root / 'etc' / 'removed').write_text('gone')
            before = system_tree_index(root)
            (root / 'etc' / 'edited').write_text('new contents')
            (root / 'etc' / 'removed').unlink()
            (root / 'etc' / 'added').write_text('new')
            (root / 'scratch' / 'work').write_text('ignored')
            after = system_tree_index(root)
            self.assertEqual(system_tree_diff(before, after), {
                'added': ['etc/added'], 'modified': ['etc', 'etc/edited'],
                'removed': ['etc/removed'],
            })

    def test_symlink_is_recorded_but_never_followed(self):
        import tempfile
        from pathlib import Path
        from execution_pod import system_tree_index

        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryDirectory() as outside:
            root = Path(directory)
            (Path(outside) / 'secret').write_text('hidden')
            (root / 'link').symlink_to(outside, target_is_directory=True)
            index = system_tree_index(root)
            self.assertIn('link', index)
            self.assertNotIn('link/secret', index)

    def test_initial_copy_reuses_system_and_reset_recopies_image(self):
        import tempfile
        from pathlib import Path
        from execution_pod import initialize_system_root

        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            image, store = base / 'image', base / 'store'
            (image / 'usr' / 'bin').mkdir(parents=True)
            (image / 'usr' / 'bin' / 'base').write_text('image')
            (image / 'scratch').mkdir()
            (image / 'scratch' / 'user-data').write_text('not image')
            initialize_system_root(image, store)
            root = store / 'rootfs'
            (root / 'usr' / 'bin' / 'installed').write_text('installed')
            initialize_system_root(image, store)
            self.assertTrue((root / 'usr' / 'bin' / 'installed').exists())
            self.assertFalse((root / 'scratch' / 'user-data').exists())
            initialize_system_root(image, store, reset=True)
            self.assertFalse((root / 'usr' / 'bin' / 'installed').exists())
            self.assertEqual((root / 'usr' / 'bin' / 'base').read_text(), 'image')


class FullDiskRegressionTest(unittest.TestCase):
    def test_operations_journal_has_independent_bounded_storage(self):
        from execution_pod import execution_manifests
        settings = {'scratch_space_id': 's', 'image': 'image@sha256:' + 'a' * 64, 'cpu': 2,
                    'memory_bytes': 2 << 30, 'capacity_bytes': 2 << 30, 'docker_enabled': False}
        config = {'runtime_class': 'gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64, 'namespace': 'sandbox'}
        _, pod = execution_manifests(settings, config)
        mount = next(item for item in pod['spec']['containers'][0]['volumeMounts']
                     if item['mountPath'] == '/scratch-system/operations')
        volume = next(item for item in pod['spec']['volumes'] if item['name'] == mount['name'])
        self.assertEqual(volume['emptyDir']['medium'], 'Memory')
        self.assertGreater(int(volume['emptyDir']['sizeLimit']), 0)

    def test_claim_excludes_a_late_supervisor(self):
        import tempfile
        from pathlib import Path
        from sandbox_v2 import claim_operation
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            self.assertTrue(claim_operation(operation, directory))
            self.assertFalse(claim_operation(operation, directory))
            self.assertEqual([p.name for p in Path(directory).iterdir()], [operation + '.started'])

    def test_late_supervise_after_claim_leaves_no_running_record(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import patch
        import sandbox_v2
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            sandbox_v2.claim_operation(operation, directory)
            with patch.object(sandbox_v2, 'OPERATION_DIRECTORY', directory), \
                 patch.dict('os.environ', {'AIDASH_POD_UID': 'uid'}):
                with self.assertRaises(FileExistsError):
                    sandbox_v2.supervise_command(operation, 'run', '.', 'true', 5, False,
                                                 expected_pod_uid='uid')
            self.assertEqual([path.name for path in Path(directory).iterdir()], [operation + '.started'])

    def test_completed_operation_bookkeeping_is_released(self):
        import tempfile
        from pathlib import Path
        from sandbox_v2 import forget_operation
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for suffix in ('.started', '.json', '.running'):
                (root / (operation + suffix)).write_text('record')
            (root / 'unrelated').write_text('keep')
            output = root / 'output'
            output.mkdir()
            for suffix in ('.stdout', '.stderr'):
                (output / (operation + suffix)).write_text('output')
            forget_operation(operation, root, output)
            self.assertEqual(sorted(path.name for path in root.iterdir()), ['output', 'unrelated'])
            self.assertEqual(list(output.iterdir()), [])

    def test_reset_releases_old_root_before_copying_new_image(self):
        import tempfile
        import shutil
        from pathlib import Path
        from unittest.mock import patch
        from execution_pod import initialize_system_root
        with tempfile.TemporaryDirectory() as directory:
            image, store = Path(directory) / 'image', Path(directory) / 'store'
            image.mkdir()
            (image / 'new').write_text('image')
            (store / 'rootfs').mkdir(parents=True)
            (store / 'rootfs' / 'old').write_text('old')
            original_copy = shutil.copytree
            def copy(*args, **kwargs):
                self.assertFalse((store / 'rootfs').exists())
                return original_copy(*args, **kwargs)
            with patch('shutil.copytree', side_effect=copy):
                initialize_system_root(image, store, reset=True)
            self.assertTrue((store / 'rootfs' / 'new').exists())

    def test_full_disk_releases_reserve_and_retries_tracking(self):
        import errno
        import tempfile
        from pathlib import Path
        from sandbox_v2 import disk_tolerant_track
        with tempfile.TemporaryDirectory() as directory:
            reserve = Path(directory) / 'reserve'
            reserve.write_bytes(b'space')
            calls = []
            def tracker():
                calls.append(1)
                if reserve.exists():
                    raise OSError(errno.ENOSPC, 'full')
            self.assertTrue(disk_tolerant_track(tracker, reserve))
            self.assertEqual(len(calls), 2)
            self.assertFalse(reserve.exists())
            self.assertFalse(disk_tolerant_track(lambda: (_ for _ in ()).throw(OSError(errno.ENOSPC, 'full')), reserve))
            self.assertFalse(disk_tolerant_track(lambda: (_ for _ in ()).throw(OSError(errno.EDQUOT, 'quota')), reserve))


class ImageEnvironmentRegressionTest(unittest.TestCase):
    def test_image_environment_is_applied_only_to_guest_commands(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import patch
        from sandbox_v2 import chroot_command, save_image_environment
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'rootfs'
            root.mkdir()
            save_image_environment(root.parent, {'PATH': '/opt/rust/bin:/usr/bin', 'CARGO_HOME': '/cargo',
                                                 'AIDASH_PROCESS_LIMIT': 'evil'})
            with patch('sandbox_v2.enter_user_root'), patch('os.execvpe') as execute, \
                 patch.dict('os.environ', {'AIDASH_PROCESS_LIMIT': '128', 'HTTPS_PROXY': 'http://proxy'}, clear=True):
                chroot_command(str(root), '.', ['/bin/sh'], False)
            env = execute.call_args.args[2]
            self.assertEqual(env['PATH'], '/opt/rust/bin:/usr/bin')
            self.assertEqual(env['CARGO_HOME'], '/cargo')
            self.assertEqual(env['HTTPS_PROXY'], 'http://proxy')
            self.assertNotIn('AIDASH_PROCESS_LIMIT', env)


class PodProxyUrlTest(unittest.TestCase):
    def test_a_credential_travels_in_the_proxy_url(self):
        from execution_pod import pod_proxy_url
        self.assertEqual(pod_proxy_url('http://10.0.0.8:8888', 'aidash-scope-v1.a-b_c'),
                         'http://aidash:aidash-scope-v1.a-b_c@10.0.0.8:8888')

    def test_no_credential_leaves_the_address_alone(self):
        from execution_pod import pod_proxy_url
        self.assertEqual(pod_proxy_url('http://10.0.0.8:8888', None), 'http://10.0.0.8:8888')

    def test_a_credential_that_could_change_the_url_is_refused_without_repeating_it(self):
        from execution_pod import pod_proxy_url
        for credential in ('', 'a@evil:80', 'a/b', 'a b', 'x' * 513, 7):
            with self.assertRaises(ValueError) as refused:
                pod_proxy_url('http://10.0.0.8:8888', credential)
            self.assertEqual(str(refused.exception), 'invalid egress credential')


    def test_a_pods_proxy_settings_carry_the_credential_the_server_sent(self):
        from execution_pod import execution_manifests
        settings = {'scratch_space_id': 's', 'image': 'image@sha256:' + 'a' * 64, 'cpu': 2,
                    'memory_bytes': 2 << 30, 'capacity_bytes': 2 << 30, 'docker_enabled': False,
                    'egress_credential': 'aidash-scope-v1.a'}
        config = {'runtime_class': 'gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64, 'namespace': 'sandbox',
                  'proxy_endpoint': 'http://10.0.0.8:8888', 'proxy_namespace': 'gateway',
                  'proxy_selector': {'app': 'gateway'}}
        _, pod = execution_manifests(settings, config)
        env = {item['name']: item.get('value') for item in pod['spec']['containers'][0]['env']}
        for name in ('HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy'):
            self.assertEqual(env[name], 'http://aidash:aidash-scope-v1.a@10.0.0.8:8888')
        self.assertEqual(env['NODE_USE_ENV_PROXY'], '1')


class StartupStorageRegressionTest(unittest.TestCase):
    def test_stale_staging_is_removed_before_reusing_existing_root(self):
        import tempfile
        from pathlib import Path
        from execution_pod import initialize_system_root
        with tempfile.TemporaryDirectory() as directory:
            image, store = Path(directory) / 'image', Path(directory) / 'store'
            image.mkdir()
            (store / 'rootfs').mkdir(parents=True)
            stale = store / 'rootfs.old.new'
            stale.mkdir()
            (stale / 'partial').write_text('data')
            self.assertFalse(initialize_system_root(image, store))
            self.assertFalse(stale.exists())

    def test_v2_process_limit_is_independent_of_legacy_limit(self):
        from execution_pod import execution_manifests
        settings = {'scratch_space_id': 's', 'image': 'image@sha256:' + 'a' * 64, 'cpu': 2,
                    'memory_bytes': 2 << 30, 'capacity_bytes': 2 << 30, 'docker_enabled': False}
        config = {'runtime_class': 'gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64,
                  'namespace': 'sandbox', 'processes': 128}
        _, pod = execution_manifests(settings, config)
        env = {item['name']: item.get('value') for item in pod['spec']['containers'][0]['env']}
        self.assertEqual(env['AIDASH_PROCESS_LIMIT'], '512')

    def test_tmp_is_memory_bounded_and_ephemeral_storage_has_a_limit(self):
        from execution_pod import execution_manifests
        settings = {'scratch_space_id': 's', 'image': 'image@sha256:' + 'a' * 64, 'cpu': 2,
                    'memory_bytes': 2 << 30, 'capacity_bytes': 2 << 30, 'docker_enabled': False}
        config = {'runtime_class': 'gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64, 'namespace': 'sandbox'}
        _, pod = execution_manifests(settings, config)
        spec = pod['spec']
        execution = spec['containers'][0]
        tmp = next(m for m in execution['volumeMounts'] if m['mountPath'] == '/tmp')
        volume = next(v for v in spec['volumes'] if v['name'] == tmp['name'])
        self.assertEqual(volume['emptyDir']['medium'], 'Memory')
        self.assertGreater(int(volume['emptyDir']['sizeLimit']), 0)
        self.assertIn('ephemeral-storage', execution['resources']['limits'])


class ProcessCapabilityTest(unittest.TestCase):
    @unittest.skipUnless(__import__('sys').platform.startswith('linux'), 'Linux capabilities required')
    def test_normal_command_cannot_regain_docker_capabilities(self):
        import subprocess
        import sys
        from pathlib import Path

        status = Path('/proc/self/status').read_text()
        effective = int(next(line.split()[1] for line in status.splitlines() if line.startswith('CapEff:')), 16)
        if not effective & (1 << 8):
            self.skipTest('CAP_SETPCAP is required to exercise bounding-set reduction')
        result = subprocess.run([sys.executable, 'sandbox_v2.py', 'normal', '/bin/cat', '/proc/self/status'],
                                capture_output=True, text=True, check=True)
        fields = dict(line.split(':', 1) for line in result.stdout.splitlines() if ':' in line)
        blocked = sum(1 << cap for cap in (12, 13, 18, 21))
        for name in ('CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb'):
            self.assertEqual(int(fields[name], 16) & blocked, 0, name)
        self.assertEqual(fields['NoNewPrivs'].strip(), '1')


class ChangeLogTest(unittest.TestCase):
    def test_records_each_run_and_its_changes(self):
        import tempfile
        from pathlib import Path
        from execution_pod import append_change_log, read_change_log

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'changes.jsonl'
            append_change_log(path, 'run-1', {'added': ['usr/bin/jq'], 'modified': [], 'removed': []})
            append_change_log(path, 'run-2', {'added': [], 'modified': [], 'removed': ['etc/example']})
            records = read_change_log(path)
            self.assertEqual([entry['run_id'] for entry in records], ['run-1', 'run-2'])
            self.assertEqual(records[0]['changes']['added'], ['usr/bin/jq'])
            self.assertEqual(records[1]['changes']['removed'], ['etc/example'])


class PodManifestTest(unittest.TestCase):
    def test_isolated_pod_and_durable_claim_for_scratch(self):
        from execution_pod import execution_manifests

        settings = {'scratch_space_id': 'scratch one', 'image': 'image@sha256:' + 'a' * 64,
                    'cpu': 4, 'memory_bytes': 16 << 30, 'capacity_bytes': 100 << 30,
                    'docker_enabled': False}
        config = {'runtime_class': 'aidash-gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64,
                  'namespace': 'aidash-sandbox', 'storage_class': 'fast'}
        pvc, pod = execution_manifests(settings, config)
        self.assertEqual(pvc['spec']['resources']['requests']['storage'], str(100 << 30))
        self.assertEqual(pvc['spec']['storageClassName'], 'fast')
        self.assertFalse(pod['spec']['automountServiceAccountToken'])
        self.assertEqual(pod['spec']['runtimeClassName'], 'aidash-gvisor')
        self.assertEqual(pod['spec']['initContainers'][1]['image'], settings['image'])
        self.assertTrue(pod['spec']['initContainers'][1]['volumeMounts'][1]['readOnly'])
        self.assertEqual(pod['spec']['containers'][0]['securityContext']['runAsUser'], 0)
        caps = pod['spec']['containers'][0]['securityContext']['capabilities']['add']
        self.assertNotIn('NET_ADMIN', caps)
        self.assertNotIn('NET_RAW', caps)
        self.assertTrue(any(v.get('persistentVolumeClaim', {}).get('claimName') == pvc['metadata']['name']
                            for v in pod['spec']['volumes']))
        self.assertFalse(any('hostPath' in v for v in pod['spec']['volumes']))

    def test_docker_requires_operator_authority_and_dedicated_tenant(self):
        from execution_pod import execution_manifests

        settings = {'scratch_space_id': 'id', 'image': 'image@sha256:' + 'a' * 64, 'cpu': 2,
                    'memory_bytes': 1 << 30, 'capacity_bytes': 1 << 30, 'docker_enabled': True,
                    'tenant_id': 'tenant one'}
        config = {'runtime_class': 'aidash-gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64,
                  'namespace': 'aidash-sandbox', 'allow_docker': False,
                  'docker_runtime_class': 'aidash-gvisor-docker'}
        with self.assertRaises(ValueError):
            execution_manifests(settings, config)
        config['allow_docker'] = True
        _, pod = execution_manifests(settings, config)
        caps = pod['spec']['containers'][0]['securityContext']['capabilities']['add']
        self.assertEqual(pod['spec']['runtimeClassName'], 'aidash-gvisor-docker')
        self.assertIn('NET_ADMIN', caps)
        self.assertIn('NET_RAW', caps)
        self.assertIn('aidash.run/tenant', pod['spec']['nodeSelector'])
        self.assertTrue(pod['spec']['tolerations'])
        self.assertTrue(any(v.get('emptyDir', {}).get('medium') == 'Memory' and v['name'] == 'docker'
                            for v in pod['spec']['volumes']))

    def test_image_hardlinks_are_preserved(self):
        import os
        import tempfile
        from pathlib import Path
        from execution_pod import initialize_system_root

        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            image = base / 'image'
            image.mkdir()
            (image / 'first').write_text('image content')
            os.link(image / 'first', image / 'second')
            initialize_system_root(image, base / 'store')
            root = base / 'store' / 'rootfs'
            self.assertEqual((root / 'first').stat().st_ino, (root / 'second').stat().st_ino)


class ExecutionPodControllerTest(unittest.TestCase):
    def test_v2_admission_is_required_before_creating_resources(self):
        from execution_pod import ExecutionPodController

        class Backend:
            def kube(self, *_args, **_kwargs):
                raise AssertionError('unverified pod must not reach Kubernetes')

        controller = ExecutionPodController(Backend(), {}, admitted=False)
        with self.assertRaisesRegex(RuntimeError, 'admission'):
            controller.ensure({'scratch_space_id': 'example'})

    def test_cancel_run_only_cancels_the_named_scratch_space(self):
        import threading
        from execution_pod import CommandQueue, ExecutionPodController

        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        controller = ExecutionPodController(SimpleNamespace(root=Path(directory.name)), {}, admitted=True)
        release = threading.Event()
        queues = {space: CommandQueue() for space in ('one', 'two')}
        controller.queues.update(queues)
        try:
            for queue in queues.values():
                for _ in range(8):
                    queue.submit('active', None, lambda _: release.wait(5))
            first = queues['one'].submit('same-run', 'one', lambda value: value)
            second = queues['two'].submit('same-run', 'two', lambda value: value)
            controller.cancel_run('one', 'same-run')
            self.assertTrue(first.cancelled())
            self.assertFalse(second.cancelled())
            release.set()
            self.assertEqual(second.result(timeout=5), 'two')
        finally:
            release.set()
            for queue in queues.values():
                queue.close()

    def test_ensure_is_idempotent_and_stop_preserves_claim(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController

        class Backend:
            def __init__(self):
                self.objects = {}
                self.calls = []
            def kube(self, args, data=None, **_kwargs):
                self.calls.append(args)
                if args[0] == 'get':
                    obj = self.objects.get((args[1], args[2]))
                    return SimpleNamespace(stdout=json.dumps(obj).encode() if obj else b'')
                if args[0] == 'create':
                    obj = json.loads(data)
                    kind = 'pvc' if obj['kind'] == 'PersistentVolumeClaim' else 'pod'
                    if kind == 'pod':
                        obj['metadata']['uid'] = 'pod-uid'
                        obj['status'] = {'phase': 'Pending'}
                    self.objects[(kind, obj['metadata']['name'])] = obj
                elif args[0] == 'delete':
                    self.objects.pop((args[1], args[2]), None)
                return SimpleNamespace(stdout=b'')

        backend = Backend()
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        backend.root = Path(temporary.name)
        config = {'runtime_class': 'aidash-gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64,
                  'namespace': 'aidash-sandbox'}
        settings = {'scratch_space_id': 'id', 'image': 'image@sha256:' + 'a' * 64, 'cpu': 2,
                    'memory_bytes': 1 << 30, 'capacity_bytes': 1 << 30, 'docker_enabled': False}
        controller = ExecutionPodController(backend, config, admitted=True)
        controller._admit_pod = Mock()
        import threading
        from concurrent.futures import ThreadPoolExecutor
        started, release = threading.Event(), threading.Event()
        def verify(*args):
            started.set()
            self.assertTrue(release.wait(3))
            return 'pod-uid'
        controller._verify_running_pod = Mock(side_effect=verify)
        with ThreadPoolExecutor(max_workers=2) as pool:
            first = pool.submit(controller.ensure, settings)
            self.assertTrue(started.wait(3))
            second = pool.submit(controller.ensure, settings)
            release.set()
            resource = first.result(timeout=3)
            self.assertEqual(second.result(timeout=3), resource)
        controller._verify_running_pod.assert_called_once()
        waits = [args for args in backend.calls if args[0] == 'wait']
        self.assertEqual(len(waits), 1)
        self.assertIn('--timeout=1800s', waits[0])
        controller.stop('id')
        self.assertIn(('pvc', resource['claim']), backend.objects)
        self.assertNotIn(('pod', resource['pod']), backend.objects)
        controller.delete('id')
        self.assertFalse(backend.objects)


class ExecutionPodHttpTest(unittest.TestCase):
    def test_unverified_ensure_returns_typed_error_without_starting_pod(self):
        import http.client
        import json
        import threading
        from http.server import ThreadingHTTPServer
        from types import SimpleNamespace
        from control import Handler
        from execution_pod import ExecutionPodController

        class Backend:
            def kube(self, *_args, **_kwargs):
                raise AssertionError('unverified pod must not be created')
        runner = SimpleNamespace(token='x' * 32,
                                 execution_pods=ExecutionPodController(Backend(), {}, admitted=False))
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        server.runner = runner
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            conn = http.client.HTTPConnection('127.0.0.1', server.server_port, timeout=2)
            conn.request('POST', '/v2/pods/ensure', json.dumps({'scratch_space_id': 'example'}),
                         {'Authorization': 'Bearer ' + runner.token, 'Content-Type': 'application/json'})
            response = conn.getresponse()
            body = json.loads(response.read())
            self.assertEqual(response.status, 503)
            self.assertEqual(body['code'], 'admission_unverified')
            self.assertTrue(body['message'])
            conn.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def test_pod_updating_is_retryable(self):
        import http.client
        import json
        import threading
        from http.server import ThreadingHTTPServer
        from types import SimpleNamespace
        from unittest.mock import Mock
        from control import Handler
        from execution_pod import ExecutionPodError
        pods = SimpleNamespace(ensure=Mock(side_effect=ExecutionPodError('pod_updating', 'retry shortly')))
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        server.runner = SimpleNamespace(token='x' * 32, execution_pods=pods)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            conn = http.client.HTTPConnection('127.0.0.1', server.server_port, timeout=2)
            conn.request('POST', '/v2/pods/ensure', json.dumps({'scratch_space_id': 's'}),
                         {'Authorization': 'Bearer ' + server.runner.token, 'Content-Type': 'application/json'})
            response = conn.getresponse()
            self.assertEqual(response.status, 503)
            self.assertEqual(json.loads(response.read())['code'], 'pod_updating')
            conn.close()
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def test_startup_failure_contains_typed_reason(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController, ExecutionPodError

        class Backend:
            def kube(self, args, data=None, **_kwargs):
                if args[0] == 'get':
                    return SimpleNamespace(stdout=b'')
                if args[0] == 'wait':
                    raise RuntimeError('image pull failed')
                return SimpleNamespace(stdout=json.dumps({'ok': True}).encode())
        config = {'runtime_class': 'aidash-gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64,
                  'namespace': 'aidash-sandbox'}
        settings = {'scratch_space_id': 'id', 'image': 'image@sha256:' + 'a' * 64, 'cpu': 2,
                    'memory_bytes': 1 << 30, 'capacity_bytes': 1 << 30, 'docker_enabled': False}
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        backend = Backend()
        backend.root = Path(temporary.name)
        controller = ExecutionPodController(backend, config, admitted=True)
        controller._admit_pod = Mock()
        controller._verify_running_pod = Mock()
        with self.assertRaises(ExecutionPodError) as caught:
            controller.ensure(settings)
        self.assertEqual(caught.exception.code, 'startup_failed')
        self.assertIn('image pull failed', caught.exception.message)


class ExecutionRequestTest(unittest.TestCase):
    def test_default_and_maximum_timeouts_are_enforced(self):
        from execution_pod import execution_timeout
        self.assertEqual(execution_timeout({}), 600)
        self.assertEqual(execution_timeout({'timeout_seconds': 3600}), 3600)
        for value in (0, 3601, True, '10'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                execution_timeout({'timeout_seconds': value})

    def test_output_limit_applies_to_both_streams_combined(self):
        from execution_pod import BoundedOutput
        output = BoundedOutput(5)
        output.add('stdout', b'abc')
        output.add('stderr', b'defg')
        result = output.public()
        self.assertEqual(result['stdout'], 'YWJj')
        self.assertEqual(result['stderr'], 'ZGU=')
        self.assertTrue(result['truncated'])


class SupervisorTest(unittest.TestCase):
    def test_supervisor_saves_and_forwards_both_streams(self):
        import os
        import sys
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from sandbox_v2 import run_supervised
        libc = Mock()
        libc.prctl.return_value = 0
        original_write = os.write
        forwarded = {1: bytearray(), 2: bytearray()}
        def capture(fd, data):
            if fd in forwarded:
                forwarded[fd].extend(data)
                return len(data)
            return original_write(fd, data)
        with tempfile.TemporaryDirectory() as directory, patch('sandbox_v2.ctypes.CDLL', return_value=libc), \
             patch('os.write', side_effect=capture):
            result = run_supervised([sys.executable, '-c', 'import os; os.write(1,b"out"); os.write(2,b"err")'],
                                    timeout=2, output_directory=directory, operation_id='op', output_limit=100)
            self.assertEqual(result['status'], 'completed')
            self.assertFalse(result['output_incomplete'])
            self.assertFalse(result['forward_incomplete'])
            self.assertEqual(bytes(forwarded[1]), b'out')
            self.assertEqual(bytes(forwarded[2]), b'err')
            self.assertEqual((Path(directory) / 'op.stdout').read_bytes(), b'out')
            self.assertEqual((Path(directory) / 'op.stderr').read_bytes(), b'err')

    def test_saved_output_is_bounded_and_forward_failure_does_not_stop_child(self):
        import os
        import sys
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from sandbox_v2 import run_supervised
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        libc = Mock()
        libc.prctl.return_value = 0
        real_write = os.write
        def write(fd, data):
            if fd in (1, 2):
                raise BrokenPipeError()
            return real_write(fd, data)
        with tempfile.TemporaryDirectory() as directory, patch('sandbox_v2.ctypes.CDLL', return_value=libc), \
             patch('os.write', side_effect=write):
            result = run_supervised([sys.executable, '-c', 'import os; os.write(1,b"abcdefgh")'],
                                    timeout=2, output_directory=directory, operation_id=operation, output_limit=4)
            self.assertEqual(result['status'], 'completed')
            self.assertTrue(result['output_truncated'])
            self.assertTrue(result['forward_incomplete'])
            self.assertEqual((Path(directory) / (operation + '.stdout')).read_bytes(), b'abcd')

    def test_save_failure_keeps_draining_and_forwarding(self):
        import errno
        import os
        import sys
        import tempfile
        from unittest.mock import Mock, patch
        from sandbox_v2 import run_supervised
        libc = Mock()
        libc.prctl.return_value = 0
        original_open, original_write = os.open, os.write
        forwarded = bytearray()
        def failing_open(path, flags, *args, **kwargs):
            if str(path).endswith('.stdout'):
                raise OSError(errno.ENOSPC, 'full')
            return original_open(path, flags, *args, **kwargs)
        def capture(fd, data):
            if fd in (1, 2):
                forwarded.extend(data)
                return len(data)
            return original_write(fd, data)
        with tempfile.TemporaryDirectory() as directory, patch('sandbox_v2.ctypes.CDLL', return_value=libc), \
             patch('os.open', side_effect=failing_open), patch('os.write', side_effect=capture):
            result = run_supervised([sys.executable, '-c', 'import os; os.write(1,b"x"*200000)'],
                                    timeout=2, output_directory=directory, operation_id='op', output_limit=100)
            self.assertEqual(result['status'], 'completed')
            self.assertTrue(result['output_incomplete'])
            self.assertEqual(len(forwarded), 200000)

    def test_reader_join_is_bounded_when_descendant_holds_pipe(self):
        import builtins
        import os
        import signal
        import sys
        import tempfile
        import time
        from pathlib import Path
        from unittest.mock import Mock, patch
        from sandbox_v2 import run_supervised
        libc = Mock()
        libc.prctl.return_value = 0
        original_open = builtins.open
        original_waitpid = os.waitpid
        def waitpid(pid, flags):
            return (0, 0) if pid == -1 else original_waitpid(pid, flags)
        def unavailable_children(path, *args, **kwargs):
            if str(path).startswith('/proc/self/task/') and str(path).endswith('/children'):
                raise FileNotFoundError(path)
            return original_open(path, *args, **kwargs)
        with tempfile.TemporaryDirectory() as directory, patch('sandbox_v2.ctypes.CDLL', return_value=libc), \
             patch('builtins.open', side_effect=unavailable_children), \
             patch('sandbox_v2.FORWARD_GRACE_SECONDS', .1), patch('os.waitpid', side_effect=waitpid):
            pidfile = Path(directory) / 'pid'
            script = ('import subprocess,sys; p=subprocess.Popen([sys.executable,"-c",'
                      '"import time; time.sleep(30)"]); open(sys.argv[1],"w").write(str(p.pid))')
            started = time.monotonic()
            try:
                result = run_supervised([sys.executable, '-c', script, str(pidfile)], timeout=.2,
                                        output_directory=directory, operation_id='op', output_limit=100)
                self.assertLess(time.monotonic() - started, 3)
                self.assertTrue(result['output_incomplete'])
            finally:
                if pidfile.exists():
                    try:
                        pid = int(pidfile.read_text())
                        os.kill(pid, signal.SIGKILL)
                        try:
                            os.waitpid(pid, 0)
                        except ChildProcessError:
                            pass
                    except ProcessLookupError:
                        pass

    def test_slow_forward_does_not_truncate_saved_output_after_child_exits(self):
        import select
        import sys
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        import sandbox_v2
        libc = Mock()
        libc.prctl.return_value = 0
        original_select = select.select
        def slow_forward(reads, writes, errors, timeout=None):
            if writes:
                __import__('time').sleep(.01)
                return ([], [], [])
            return original_select(reads, writes, errors, timeout)
        with tempfile.TemporaryDirectory() as directory, patch.object(sandbox_v2.ctypes, 'CDLL', return_value=libc), \
             patch.object(sandbox_v2, 'FORWARD_GRACE_SECONDS', .15, create=True), \
             patch('select.select', side_effect=slow_forward):
            result = sandbox_v2.run_supervised([sys.executable, '-c', 'import os,time; os.write(1,b"a"*1000); time.sleep(.1); os.write(1,b"b"*1000)'],
                timeout=4, output_directory=directory, operation_id='op', output_limit=100000)
            self.assertEqual(result['status'], 'completed')
            self.assertTrue(result['forward_incomplete'])
            self.assertFalse(result['output_incomplete'])
            self.assertEqual((Path(directory) / 'op.stdout').read_bytes(), b'a' * 1000 + b'b' * 1000)

    @unittest.skipUnless(__import__('sys').platform.startswith('linux'), 'Linux proc and pidfd required')
    def test_timeout_stops_descendant_that_created_a_new_session(self):
        import os
        import sys
        import tempfile
        from pathlib import Path
        from sandbox_v2 import run_supervised

        with tempfile.TemporaryDirectory() as directory:
            pid_path = Path(directory) / 'child.pid'
            script = ('import os,subprocess,sys,time; '
                      'p=subprocess.Popen([sys.executable,"-c","import os,time; os.setsid(); time.sleep(60)"]); '
                      'open(sys.argv[1],"w").write(str(p.pid)); time.sleep(60)')
            result = run_supervised([sys.executable, '-c', script, str(pid_path)], timeout=1)
            self.assertEqual(result['status'], 'timed_out')
            pid = int(pid_path.read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)

    @unittest.skipUnless(__import__('sys').platform.startswith('linux'), 'Linux subreaper required')
    def test_background_descendant_remains_tracked_after_shell_exits(self):
        import sys
        import time
        from sandbox_v2 import run_supervised
        script = ('import subprocess,sys; '
                  'subprocess.Popen([sys.executable,"-c","import os,time; os.setsid(); time.sleep(.35)"])')
        start = time.monotonic()
        result = run_supervised([sys.executable, '-c', script], timeout=2)
        self.assertEqual(result, {'status': 'completed', 'exit_code': 0})
        self.assertGreaterEqual(time.monotonic() - start, .3)

    @unittest.skipUnless(__import__('sys').platform.startswith('linux'), 'Linux subreaper required')
    def test_background_process_can_be_stopped(self):
        import sys
        import threading
        from sandbox_v2 import run_supervised
        stopped = threading.Event()
        timer = threading.Timer(.3, stopped.set)
        timer.start()
        try:
            result = run_supervised([sys.executable, '-c', 'import time; time.sleep(60)'],
                                    timeout=None, stop_requested=stopped)
            self.assertEqual(result['status'], 'stopped')
        finally:
            timer.cancel()


class CommandTransportTest(unittest.TestCase):
    def test_drains_large_output_without_exceeding_budget(self):
        import os
        import sys
        from execution_pod import drain_process
        script = 'import os; os.write(1,b"a"*100000); os.write(2,b"b"*100000)'
        result = drain_process([sys.executable, '-c', script], os.environ.copy(), 100, 10)
        self.assertEqual(result['exit_code'], 0)
        self.assertTrue(result['output']['truncated'])
        import base64
        self.assertEqual(sum(len(base64.b64decode(result['output'][key]))
                             for key in ('stdout', 'stderr')), 100)

    def test_transport_timeout_stops_its_local_process(self):
        import os
        import sys
        from execution_pod import drain_process
        result = drain_process([sys.executable, '-c', 'import time; time.sleep(30)'],
                               os.environ.copy(), 100, .2)
        self.assertTrue(result['transport_timed_out'])


class ReconnectedCommandRegressionTest(unittest.TestCase):
    def test_late_result_cannot_be_overwritten_by_the_transport_worker(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            backend = Mock(root=Path(directory), command=['kubectl'], environment={}, config={})
            controller = ExecutionPodController(backend, {}, admitted=True)
            record = {'operation_id':operation, 'scratch_space_id':'s', 'run_id':'r', 'pod':'pod',
                      'pod_uid':'uid', 'docker_enabled':False, 'background':False, 'timeout_seconds':5,
                      'cwd':'.', 'code':'true', 'status':'queued'}
            controller._command_path(operation).write_text(json.dumps(record))
            def result_arrived(*_):
                controller._update_command(operation, status='interrupted', outcome='unknown')
                controller._update_command(operation, allow_late=True, status='completed', outcome='known', exit_code=0)
                return None
            controller._await_normal_result = Mock(side_effect=result_arrived)
            controller._stop_matching_pod = Mock()
            transport = {'exit_code':1, 'transport_timed_out':False,
                         'output':{'stdout':'', 'stderr':'', 'truncated':False}}
            with patch('execution_pod.drain_process', return_value=transport):
                controller._execute_command(operation)
            self.assertEqual(controller._read_command(operation)['status'], 'completed')
            controller._stop_matching_pod.assert_not_called()

    def test_lost_transport_leaves_unreachable_pod_running(self):
        import json
        import subprocess
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            backend = Mock(root=Path(directory), command=['kubectl'], environment={}, config={})
            controller = ExecutionPodController(backend, {}, admitted=True)
            record = {'operation_id':operation, 'scratch_space_id':'s', 'run_id':'r', 'pod':'pod',
                      'pod_uid':'uid', 'docker_enabled':False, 'background':False, 'timeout_seconds':5,
                      'cwd':'.', 'code':'true', 'status':'queued'}
            controller._command_path(operation).write_text(json.dumps(record))
            controller._await_normal_result = Mock(return_value=None)
            controller._get = Mock(side_effect=subprocess.TimeoutExpired('kubectl', 45))
            controller._stop_matching_pod = Mock()
            transport = {'exit_code':1, 'transport_timed_out':True,
                         'output':{'stdout':'', 'stderr':'', 'truncated':False}}
            with patch('execution_pod.drain_process', return_value=transport):
                controller._execute_command(operation)
            self.assertEqual(controller._read_command(operation)['status'], 'interrupted')
            self.assertEqual(controller._read_command(operation)['outcome'], 'unknown')
            controller._stop_matching_pod.assert_not_called()
            from types import SimpleNamespace
            controller._get = Mock(return_value={'metadata':{'uid':'uid'}})
            backend.kube = Mock(side_effect=lambda args, **kwargs: SimpleNamespace(
                returncode=1 if 'result' in args else 0, stdout=b''))
            controller.command_result(operation)
            self.assertTrue(any('stop' in call.args[0] for call in backend.kube.call_args_list))
            self.assertFalse(controller._read_command(operation)['stop_pending'])
            controller._stop_matching_pod.assert_not_called()

    def test_reachable_pod_is_stopped_only_after_command_stop_fails(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController, STOP_RETRY_SECONDS
        with tempfile.TemporaryDirectory() as directory:
            backend = Mock(root=Path(directory))
            controller = ExecutionPodController(backend, {}, admitted=True)
            record = {'operation_id':'e6ded09d-f783-4ce8-98e9-dbfca68884f1',
                      'scratch_space_id':'s', 'pod':'pod', 'pod_uid':'uid',
                      'status':'interrupted', 'stop_pending':True, 'terminal_at':1}
            controller._command_path(record['operation_id']).write_text(json.dumps(record))
            controller._get = Mock(return_value={'metadata':{'uid':'uid'}})
            controller.command_result = Mock(return_value={'status':'interrupted'})
            controller._stop_matching_pod = Mock()
            backend.kube = Mock(side_effect=RuntimeError('stop rejected'))
            with patch('time.time', return_value=100):
                controller._stop_untracked_command(record)
            controller._stop_matching_pod.assert_not_called()
            with patch('time.time', return_value=100 + STOP_RETRY_SECONDS):
                controller._stop_untracked_command(record)
            controller._stop_matching_pod.assert_called_once_with('s', 'uid')

    def test_outage_during_stop_does_not_fence_the_whole_space(self):
        import json
        import subprocess
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            backend = Mock(root=Path(directory))
            controller = ExecutionPodController(backend, {}, admitted=True)
            record = {'operation_id':'e6ded09d-f783-4ce8-98e9-dbfca68884f1',
                      'scratch_space_id':'s', 'pod':'pod', 'pod_uid':'uid',
                      'status':'interrupted', 'stop_pending':True, 'terminal_at':1}
            controller._command_path(record['operation_id']).write_text(json.dumps(record))
            controller._get = Mock(side_effect=[{'metadata':{'uid':'uid'}}, subprocess.TimeoutExpired('kubectl', 45)])
            controller.command_result = Mock(return_value={'status':'interrupted'})
            controller._stop_matching_pod = Mock()
            controller._mark_space_unconfirmed = Mock()
            backend.kube = Mock(side_effect=RuntimeError('transport failed'))
            controller._stop_untracked_command(record)
            controller._stop_matching_pod.assert_not_called()
            controller._mark_space_unconfirmed.assert_not_called()

    def test_concurrent_polls_send_only_one_stop_request(self):
        import json
        import tempfile
        import threading
        import time
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            backend = Mock(root=Path(directory))
            controller = ExecutionPodController(backend, {}, admitted=True)
            record = {'operation_id':'e6ded09d-f783-4ce8-98e9-dbfca68884f1', 'run_id':'r',
                      'scratch_space_id':'s', 'pod':'pod', 'pod_uid':'uid',
                      'status':'interrupted', 'stop_pending':True, 'terminal_at':time.time()}
            controller._command_path(record['operation_id']).write_text(json.dumps(record))
            controller._get = Mock(return_value={'metadata':{'uid':'uid'}})
            entered, release = threading.Event(), threading.Event()
            def stop(*args, **kwargs):
                entered.set()
                self.assertTrue(release.wait(2))
            backend.kube = Mock(side_effect=stop)
            first = threading.Thread(target=controller._stop_untracked_command, args=(record,))
            second = threading.Thread(target=controller._stop_untracked_command, args=(record,))
            first.start()
            try:
                self.assertTrue(entered.wait(2))
                second.start()
            finally:
                release.set()
                first.join(2)
                second.join(2)
            self.assertFalse(first.is_alive() or second.is_alive())
            backend.kube.assert_called_once()
            self.assertFalse(controller._read_command(record['operation_id'])['stop_pending'])

    def test_result_wait_survives_a_45_second_kubernetes_timeout(self):
        import json
        import subprocess
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch
        from execution_pod import ExecutionPodController
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            clock = [0.0]
            class Backend:
                root = Path(directory)
                calls = 0
                def kube(self, args, **kwargs):
                    self.calls += 1
                    if self.calls == 1:
                        clock[0] += 45
                        raise subprocess.TimeoutExpired('kubectl', 45)
                    return SimpleNamespace(returncode=0, stdout=json.dumps(
                        {'operation_id':operation, 'run_id':'r', 'status':'completed'}).encode())
            controller = ExecutionPodController(Backend(), {}, admitted=True)
            record = {'operation_id':operation, 'run_id':'r', 'status':'running', 'pod':'pod', 'pod_uid':'uid'}
            controller._command_path(operation).write_text(json.dumps(record))
            with patch('time.monotonic', side_effect=lambda: clock[0]), patch('time.sleep'):
                result = controller._await_normal_result(record, 100)
            self.assertEqual(result['status'], 'completed')
            self.assertEqual(controller.backend.calls, 2)

    def test_background_wait_does_not_expire_when_result_is_temporarily_missing(self):
        import json
        import subprocess
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch
        from execution_pod import ExecutionPodController
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            clock = [0.0]
            class Backend:
                root = Path(directory)
                outage = True
                def kube(self, args, **kwargs):
                    if args[0] == 'get':
                        return SimpleNamespace(stdout=b'{"metadata":{"uid":"uid"},"status":{"phase":"Running"}}')
                    if 'running' in args:
                        return SimpleNamespace(returncode=0, stdout=b'{"running":false}')
                    if self.outage:
                        self.outage = False
                        clock[0] += 45
                        raise subprocess.TimeoutExpired('kubectl', 45)
                    if clock[0] < 65:
                        return SimpleNamespace(returncode=1, stdout=b'')
                    return SimpleNamespace(returncode=0, stdout=b'{"status":"completed"}')
            controller = ExecutionPodController(Backend(), {}, admitted=True)
            record = {'operation_id':operation, 'run_id':'r', 'status':'running', 'pod':'pod', 'pod_uid':'uid'}
            controller._command_path(operation).write_text(json.dumps(record))
            with patch('time.monotonic', side_effect=lambda: clock[0]), \
                 patch('time.sleep', side_effect=lambda _: clock.__setitem__(0, clock[0] + 10)):
                result = controller._await_normal_result(record, None)
            self.assertEqual(result['status'], 'completed')
            self.assertGreaterEqual(clock[0], 65)

    def test_background_missing_result_expires_and_terminal_pod_stops_wait(self):
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            backend = Mock(root=Path(directory))
            backend.kube.side_effect = lambda args, **kwargs: SimpleNamespace(
                returncode=0 if 'running' in args else 1, stdout=b'{"running":false}')
            controller = ExecutionPodController(backend, {}, admitted=True)
            record = {'operation_id':operation, 'status':'running', 'pod':'pod', 'pod_uid':'uid'}
            controller._command_path(operation).write_text(__import__('json').dumps(record))
            clock = [0.0]
            controller._get = Mock(return_value={'metadata':{'uid':'uid'}, 'status':{'phase':'Running'}})
            def advance(_):
                clock[0] += 10
                if clock[0] > 50:
                    raise AssertionError('missing background result never timed out')
            with patch('time.monotonic', side_effect=lambda: clock[0]), \
                 patch('time.sleep', side_effect=advance):
                self.assertIsNone(controller._await_normal_result(record, None))
            self.assertGreaterEqual(clock[0], 30)
            for phase in ('Failed', 'Succeeded'):
                controller._get.return_value['status']['phase'] = phase
                with patch('time.sleep', side_effect=AssertionError('terminal Pod must not wait')):
                    self.assertIsNone(controller._await_normal_result(record, None))
            for phase in ('Pending', 'Unknown'):
                controller._get.return_value['status']['phase'] = phase
                calls = [0]
                def later_result(args, calls=calls, **kwargs):
                    if 'result' in args:
                        calls[0] += 1
                        return SimpleNamespace(returncode=1 if calls[0] == 1 else 0,
                                               stdout=b'{"status":"completed"}')
                    return SimpleNamespace(returncode=0, stdout=b'{"running":true}')
                backend.kube.side_effect = later_result
                with patch('time.sleep'):
                    self.assertEqual(controller._await_normal_result(record, None)['status'], 'completed')

    def test_unconfirmed_stop_waits_before_normal_delete_and_requires_node(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController, ExecutionPodError, STOP_RETRY_SECONDS
        from control import scratch_resource_name
        with tempfile.TemporaryDirectory() as directory:
            backend = Mock(root=Path(directory))
            controller = ExecutionPodController(backend, {'namespace':'fixture'}, admitted=True)
            controller.touch_space('s', now=1)
            path = Path(directory) / 'execution-v2-spaces' / (scratch_resource_name('s') + '.json')
            state = json.loads(path.read_text())
            state['guard'] = {'pod_uid': 'uid', 'cluster_node': 'node', 'node_uid': 'node-uid', 'terminated': False}
            path.write_text(json.dumps(state))
            controller._terminate_recorded_pod = Mock(side_effect=ExecutionPodError('termination_unconfirmed', 'guard unavailable'))
            with patch('time.time', return_value=100):
                with self.assertRaises(ExecutionPodError):
                    controller.stop('s')
            backend.kube.assert_not_called()
            controller._get = Mock(return_value=None)
            with patch('time.time', return_value=100 + STOP_RETRY_SECONDS):
                with self.assertRaises(ExecutionPodError):
                    controller.stop('s')
            backend.kube.assert_not_called()
            controller._get = Mock(side_effect=lambda kind, name: {'metadata':{'name':name, 'uid':'node-uid'}} if kind == 'node' else None)
            with patch('time.time', return_value=100 + STOP_RETRY_SECONDS):
                with self.assertRaises(ExecutionPodError):
                    controller.stop('s')
            backend.kube.assert_not_called()
            controller._get = Mock(side_effect=lambda kind, name: {'metadata': {'uid':'replacement'}} if kind == 'node'
                                   else {'metadata': {'uid':'uid'}, 'spec': {'nodeName':'node'}})
            with patch('time.time', return_value=100 + STOP_RETRY_SECONDS):
                with self.assertRaises(ExecutionPodError):
                    controller.stop('s')
            backend.kube.assert_not_called()
            pod_reads = [0]
            def present_node_and_pod(kind, name):
                if kind == 'node':
                    return {'metadata': {'name': name, 'uid': 'node-uid'}}
                pod_reads[0] += 1
                return {'metadata': {'uid':'uid'}, 'spec': {'nodeName':'node'}} if pod_reads[0] == 1 else None
            controller._get = Mock(side_effect=present_node_and_pod)
            with patch('time.time', return_value=100 + STOP_RETRY_SECONDS):
                self.assertTrue(controller.stop('s')['stopped'])
            self.assertEqual(controller._terminate_recorded_pod.call_count, 1)
            args = backend.kube.call_args_list[0].args[0]
            self.assertEqual(args[0], 'delete')
            self.assertNotIn('--force', args)
            self.assertEqual(json.loads(backend.kube.call_args_list[0].args[1])['preconditions']['uid'], 'uid')
            self.assertFalse(json.loads(path.read_text())['termination_unconfirmed'])

    def test_guard_preserves_node_identity_for_the_same_pod(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController, ExecutionPodError
        from control import scratch_resource_name
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {'helper_image': 'helper'}, admitted=True)
            controller.touch_space('s', now=1)
            name = scratch_resource_name('s')
            pod = {'metadata': {'name': name, 'uid': 'pod-uid'}, 'spec': {'nodeName': 'node'}}
            controller._get = Mock(return_value={'metadata': {'uid': 'first-node'}})
            controller._record_guard(pod, 'execution', 'container', 1)
            controller._get.return_value = {'metadata': {'uid': 'replacement-node'}}
            with self.assertRaises(ExecutionPodError):
                controller._record_guard(pod, 'execution', 'container', 1)

    def test_interrupted_command_adopts_a_late_result_without_replay(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            class Backend:
                root = Path(directory)
                def kube(self, args, **kwargs):
                    if args[0] == 'get':
                        return SimpleNamespace(stdout=b'{"metadata":{"uid":"uid"},"status":{"phase":"Running"}}')
                    if 'output' in args:
                        return SimpleNamespace(returncode=0, stdout=b'{"data":"","next_offset":0,"eof":true}')
                    return SimpleNamespace(returncode=0, stdout=json.dumps(
                        {'operation_id': operation, 'run_id': 'r', 'status': 'completed', 'exit_code': 7}).encode())
            controller = ExecutionPodController(Backend(), {}, admitted=True)
            controller._command_path(operation).write_text(json.dumps({'operation_id': operation,
                'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid',
                'status': 'interrupted', 'outcome': 'unknown', 'terminal_at': __import__('time').time()}))
            result = controller.command_result(operation)
            self.assertEqual(result['status'], 'completed')
            self.assertEqual(result['exit_code'], 7)

    def test_lost_stream_waits_for_supervisor_result_without_stopping_pod(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch
        from execution_pod import ExecutionPodController
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            class Backend:
                root, command, environment = Path(directory), ['kubectl'], {}
                attempts = 0
                def kube(self, args, **kwargs):
                    if args[0] == 'get':
                        return SimpleNamespace(stdout=b'{"metadata":{"uid":"uid"},"status":{"phase":"Running"}}', returncode=0)
                    if 'result' in args:
                        self.attempts += 1
                        if self.attempts == 1:
                            return SimpleNamespace(stdout=b'', returncode=1)
                        return SimpleNamespace(stdout=json.dumps({'operation_id': operation, 'run_id': 'r',
                            'status': 'completed', 'exit_code': 127}).encode(), returncode=0)
                    return SimpleNamespace(stdout=b'{"running":true}', returncode=0)
            controller = ExecutionPodController(Backend(), {}, admitted=True)
            controller._command_path(operation).write_text(json.dumps({'operation_id': operation, 'scratch_space_id': 's',
                'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'docker_enabled': False, 'background': False,
                'timeout_seconds': 5, 'cwd': '.', 'code': 'true', 'status': 'queued'}))
            transport = {'exit_code': 1, 'transport_timed_out': False,
                         'output': {'stdout': '', 'stderr': '', 'truncated': False}}
            with patch('execution_pod.drain_process', return_value=transport), patch('time.sleep'), \
                 patch('execution_pod.OUTPUT_RETRY_SECONDS', 0):
                controller._execute_command(operation)
            self.assertEqual(controller.command_result(operation)['exit_code'], 127)
            self.assertEqual(controller.backend.attempts, 2)


class RestartResumeTest(unittest.TestCase):
    operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'

    def test_resume_result_reads_saved_output_and_forgets(self):
        import base64
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        op = self.operation
        with tempfile.TemporaryDirectory() as directory:
            calls = []
            def kube(args, **kwargs):
                calls.append(args)
                if 'claim' in args:
                    body = {'created': False}
                elif 'result' in args:
                    body = {'operation_id': op, 'run_id': 'r', 'status': 'completed', 'exit_code': 7}
                elif 'output' in args:
                    stream = args[-3]
                    data = b'hello' if stream == 'stdout' else b'err'
                    body = {'data': base64.b64encode(data).decode(), 'next_offset': len(data), 'eof': True}
                else:
                    body = {}
                return SimpleNamespace(returncode=0, stdout=json.dumps(body).encode())
            backend = SimpleNamespace(root=Path(directory), kube=kube)
            controller = ExecutionPodController(backend, {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'}})
            controller._command_path(op).write_text(json.dumps({'operation_id': op, 'scratch_space_id': 's',
                'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'status': 'running', 'background': True}))
            controller._resume_command(op)
            result = controller.command_result(op)
            self.assertEqual((result['status'], result['exit_code']), ('completed', 7))
            self.assertEqual(base64.b64decode(result['stdout']), b'hello')
            self.assertEqual(base64.b64decode(result['stderr']), b'err')
            self.assertTrue(any('forget' in args for args in calls))

    def test_resume_keeps_available_output_if_other_stream_is_missing(self):
        import base64
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        op = self.operation
        def kube(args, **kwargs):
            if 'claim' in args:
                body = {'created': False}
            elif 'result' in args:
                body = {'operation_id': op, 'run_id': 'r', 'status': 'completed', 'exit_code': 0}
            elif 'output' in args and args[-3] == 'stdout':
                body = {'data': base64.b64encode(b'kept').decode(), 'next_offset': 4, 'eof': True}
            elif 'output' in args:
                return SimpleNamespace(returncode=1, stdout=b'')
            else:
                body = {}
            return SimpleNamespace(returncode=0, stdout=json.dumps(body).encode())
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._command_path(op).write_text(json.dumps({'operation_id': op, 'scratch_space_id': 's',
                'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'status': 'running', 'background': True}))
            with patch('execution_pod.OUTPUT_RETRY_SECONDS', 0):
                controller._resume_command(op)
            result = controller.command_result(op)
            self.assertEqual(base64.b64decode(result['stdout']), b'kept')
            self.assertTrue(result['truncated'])
            self.assertIn('output', result['message'].lower())

    def test_resume_gone_replaced_or_claimed_never_stops_pod(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        for pod, expected in [(None, ('interrupted', 'unknown')),
                              ({'metadata': {'uid': 'new'}}, ('interrupted', 'unknown')),
                              ({'metadata': {'uid': 'uid'}}, ('cancelled', 'not_started'))]:
            with self.subTest(pod=pod), tempfile.TemporaryDirectory() as directory:
                backend = SimpleNamespace(root=Path(directory), kube=Mock(return_value=SimpleNamespace(
                    returncode=0, stdout=b'{"created":true}')))
                controller = ExecutionPodController(backend, {}, admitted=False)
                controller._get = Mock(return_value=pod)
                controller._command_path(self.operation).write_text(json.dumps({'operation_id': self.operation,
                    'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid',
                    'status': 'running', 'background': True}))
                controller._resume_command(self.operation)
                result = controller._read_command(self.operation)
                self.assertEqual((result['status'], result['outcome']), expected)
                self.assertFalse(any('stop' in args.args[0] for args in backend.kube.call_args_list))

    def test_successful_claim_cancels_without_another_pod_lookup(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            backend = SimpleNamespace(root=Path(directory), kube=Mock(return_value=SimpleNamespace(
                returncode=0, stdout=b'{"created":true}')))
            controller = ExecutionPodController(backend, {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._command_path(self.operation).write_text(json.dumps({'operation_id': self.operation,
                'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid',
                'status': 'running', 'background': True}))
            controller._resume_command(self.operation)
            result = controller._read_command(self.operation)
            self.assertEqual((result['status'], result['outcome']), ('cancelled', 'not_started'))
            controller._get.assert_called_once()

    def test_resume_retries_transient_failure_and_publishes_while_running(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        op = self.operation
        with tempfile.TemporaryDirectory() as directory:
            backend = SimpleNamespace(root=Path(directory), kube=Mock(return_value=SimpleNamespace(
                returncode=0, stdout=b'{"created":false}')))
            controller = ExecutionPodController(backend, {}, admitted=False)
            controller._get = Mock(side_effect=[RuntimeError('offline'), {'metadata': {'uid': 'uid'}}])
            controller._command_path(op).write_text(json.dumps({'operation_id': op, 'scratch_space_id': 's',
                'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'status': 'running', 'background': True}))
            def await_result(record, deadline, *, publish_saved):
                self.assertTrue(publish_saved)
                self.assertEqual(controller._read_command(op)['status'], 'running')
                return {'operation_id': op, 'run_id': 'r', 'status': 'completed', 'exit_code': 0}
            controller._await_normal_result = await_result
            controller._saved_output = Mock(side_effect=RuntimeError('disk full'))
            with patch('time.sleep'):
                controller._resume_command(op)
            result = controller.command_result(op)
            self.assertEqual(result['status'], 'completed')
            self.assertTrue(result['truncated'])
            self.assertIn('output', result['message'].lower())
            self.assertEqual(controller._get.call_count, 2)

    def test_running_resume_publishes_saved_output_until_result(self):
        import json
        import tempfile
        import threading
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        ready, release = threading.Event(), threading.Event()
        with tempfile.TemporaryDirectory() as directory:
            def kube(args, **kwargs):
                if 'claim' in args:
                    return SimpleNamespace(returncode=0, stdout=b'{"created":false}')
                if 'result' in args:
                    ready.set()
                    release.wait(3)
                    return SimpleNamespace(returncode=0, stdout=json.dumps({'operation_id': self.operation,
                        'run_id': 'r', 'status': 'completed', 'exit_code': 0}).encode())
                return SimpleNamespace(returncode=0, stdout=b'{}')
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._preview_saved_output = Mock(return_value={'stdout': 'eA==', 'stderr': '', 'truncated': False})
            controller._saved_output = Mock(return_value={'stdout': 'eA==', 'stderr': '', 'truncated': False})
            controller._command_path(self.operation).write_text(json.dumps({'operation_id': self.operation,
                'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid',
                'status': 'running', 'background': True}))
            worker = threading.Thread(target=controller._resume_command, args=(self.operation,))
            worker.start()
            try:
                self.assertTrue(ready.wait(2))
                result = controller.command_result(self.operation)
                self.assertEqual(result['status'], 'running')
                self.assertEqual(result['stdout'], 'eA==')
                self.assertIn('output_fetched_at', result)
            finally:
                release.set()
                worker.join(3)
            self.assertFalse(worker.is_alive())
            self.assertEqual(controller.command_result(self.operation)['status'], 'completed')

    def test_resume_preview_failure_does_not_end_watch(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            results = [SimpleNamespace(returncode=1, stdout=b''),
                       SimpleNamespace(returncode=0, stdout=b'{"running":true}'),
                       SimpleNamespace(returncode=0, stdout=json.dumps({'status': 'completed'}).encode())]
            backend = SimpleNamespace(root=Path(directory), kube=Mock(side_effect=results))
            controller = ExecutionPodController(backend, {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._preview_saved_output = Mock(side_effect=RuntimeError('output unavailable'))
            record = {'operation_id': self.operation, 'status': 'running', 'pod': 'pod', 'pod_uid': 'uid'}
            controller._command_path(self.operation).write_text(json.dumps(record))
            with patch('time.sleep'):
                self.assertEqual(controller._await_normal_result(record, None, publish_saved=True)['status'], 'completed')

    def test_interrupted_foreground_docker_is_cancelled_via_helper(self):
        import json
        import tempfile
        import time
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            kube = Mock(side_effect=lambda args, **kwargs: SimpleNamespace(
                returncode=0, stdout=b'{"created":false}' if 'claim' in args else b'{"running":true}'))
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._command_path(self.operation).write_text(json.dumps({'operation_id': self.operation,
                'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'background': False,
                'docker_enabled': True, 'status': 'interrupted', 'terminal_at': time.time()}))
            controller.cancel_run('s', 'r')
            self.assertIn('stop', kube.call_args.args[0])

    def test_docker_interrupted_never_adopts_forged_result_and_can_stop(self):
        import json
        import tempfile
        import time
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            kube = Mock(side_effect=lambda args, **kwargs: SimpleNamespace(
                returncode=0, stdout=b'{"created":false}' if 'claim' in args else b'{"running":true}'))
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._command_path(self.operation).write_text(json.dumps({'operation_id': self.operation,
                'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'background': True,
                'docker_enabled': True, 'status': 'interrupted', 'terminal_at': time.time()}))
            self.assertEqual(controller.command_result(self.operation)['status'], 'interrupted')
            kube.assert_not_called()
            self.assertEqual(controller.stop_background(self.operation)['status'], 'stopping')
            self.assertIn('stop', kube.call_args.args[0])

class LeadReviewRegressionTest(unittest.TestCase):
    op = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'

    def test_stream_failure_uses_saved_output(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        for exit_code, forward_lost in [(1, False), (0, True)]:
            with self.subTest(exit_code=exit_code), tempfile.TemporaryDirectory() as directory:
                backend = SimpleNamespace(root=Path(directory), command=['kubectl'], environment={}, kube=Mock())
                controller = ExecutionPodController(backend, {}, admitted=True)
                controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op,
                    'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid',
                    'docker_enabled': False, 'background': False, 'timeout_seconds': 5, 'cwd': '.',
                    'code': 'true', 'status': 'queued'}))
                controller._await_normal_result = Mock(return_value={'operation_id': self.op, 'run_id': 'r',
                    'status': 'completed', 'exit_code': 0, 'forward_incomplete': forward_lost})
                controller._saved_output = Mock(return_value={'stdout': 'c2F2ZWQ=', 'stderr': '', 'truncated': False})
                transport = {'exit_code': exit_code, 'transport_timed_out': False,
                             'output': {'stdout': 'cGFydA==', 'stderr': '', 'truncated': False}}
                with patch('execution_pod.drain_process', return_value=transport):
                    controller._execute_command(self.op)
                result = controller.command_result(self.op)
                self.assertEqual(result['stdout'], 'c2F2ZWQ=')
                self.assertFalse(result['truncated'])

    def test_failed_saved_output_read_keeps_truncated_live_stream(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            backend = SimpleNamespace(root=Path(directory), command=['kubectl'], environment={}, kube=Mock())
            controller = ExecutionPodController(backend, {}, admitted=True)
            controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op,
                'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'docker_enabled': False,
                'background': False, 'timeout_seconds': 5, 'cwd': '.', 'code': 'true', 'status': 'queued'}))
            controller._await_normal_result = Mock(return_value={'operation_id': self.op, 'run_id': 'r',
                'status': 'completed', 'exit_code': 0, 'forward_incomplete': True})
            controller._saved_output = Mock(side_effect=RuntimeError('offline'))
            transport = {'exit_code': 0, 'transport_timed_out': False,
                         'output': {'stdout': 'cGFydA==', 'stderr': '', 'truncated': False}}
            with patch('execution_pod.drain_process', return_value=transport):
                controller._execute_command(self.op)
            result = controller.command_result(self.op)
            self.assertEqual(result['stdout'], 'cGFydA==')
            self.assertTrue(result['truncated'])

    def test_supervisor_and_transport_use_the_same_output_limit(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        class ChangingConfig(dict):
            def __init__(self):
                super().__init__()
                self.reads = 0
            def get(self, key, default=None):
                if key == 'output_bytes':
                    self.reads += 1
                    return 64 if self.reads == 1 else 128
                return super().get(key, default)
        with tempfile.TemporaryDirectory() as directory:
            backend = SimpleNamespace(root=Path(directory), command=['kubectl'], environment={}, kube=Mock())
            controller = ExecutionPodController(backend, ChangingConfig(), admitted=True)
            controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op,
                'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'docker_enabled': False,
                'background': False, 'timeout_seconds': 5, 'cwd': '.', 'code': 'true', 'status': 'queued'}))
            controller._await_normal_result = Mock(return_value={'operation_id': self.op, 'run_id': 'r',
                'status': 'completed', 'exit_code': 0})
            def drain(argv, environment, limit, *args, **kwargs):
                self.assertEqual(int(argv[-1]), limit)
                return {'exit_code': 0, 'transport_timed_out': False,
                        'output': {'stdout': '', 'stderr': '', 'truncated': False}}
            with patch('execution_pod.drain_process', side_effect=drain):
                controller._execute_command(self.op)
            self.assertEqual(controller.command_result(self.op)['status'], 'completed')

    def test_expired_resume_still_checks_result(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        calls = []
        def kube(args, **kwargs):
            calls.append(args)
            if 'claim' in args:
                body = {'created': False}
            elif 'result' in args:
                body = {'operation_id': self.op, 'run_id': 'r', 'status': 'completed', 'exit_code': 0}
            elif 'output' in args:
                body = {'data': '', 'next_offset': 0, 'eof': True}
            else:
                body = {}
            return SimpleNamespace(returncode=0, stdout=json.dumps(body).encode())
        with tempfile.TemporaryDirectory() as directory:
            backend = SimpleNamespace(root=Path(directory), kube=kube)
            controller = ExecutionPodController(backend, {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op,
                'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid',
                'status': 'running', 'background': False, 'running_at': 1, 'timeout_seconds': 5}))
            controller._resume_command(self.op)
            self.assertEqual(controller.command_result(self.op)['status'], 'completed')
            self.assertTrue(any('result' in call for call in calls))

    def test_terminal_pod_does_not_retry_claim(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        for phase in ('Failed', 'Succeeded'):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as directory:
                kube = Mock()
                controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
                controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': phase}})
                controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op,
                    'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid',
                    'status': 'running', 'background': True}))
                controller._resume_command(self.op)
                self.assertEqual(controller._read_command(self.op)['status'], 'interrupted')
                kube.assert_not_called()

    def test_docker_late_poll_retries_pending_stop_without_reading_result(self):
        import json
        import tempfile
        import time
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            backend = Mock(root=Path(directory))
            controller = ExecutionPodController(backend, {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._stop_untracked_command = Mock()
            controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op, 'run_id': 'r',
                'pod': 'pod', 'pod_uid': 'uid', 'status': 'interrupted', 'terminal_at': time.time(),
                'docker_enabled': True, 'stop_pending': True}))
            controller.command_result(self.op)
            controller._stop_untracked_command.assert_called_once()
            backend.kube.assert_not_called()

    def test_interrupted_docker_stop_ignores_gone_pod_or_finished_process(self):
        import itertools
        import json
        import tempfile
        import time
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        for pod in (None, {'metadata': {'uid': 'new'}}, {'metadata': {'uid': 'uid'}}):
            with self.subTest(pod=pod), tempfile.TemporaryDirectory() as directory:
                kube = Mock(side_effect=lambda args, **kwargs: SimpleNamespace(
                    returncode=0, stdout=b'{"created":false}' if 'claim' in args else b'{"running":false}'))
                controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
                controller._get = Mock(return_value=pod)
                controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op,
                    'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'background': True, 'docker_enabled': True,
                    'status': 'interrupted', 'terminal_at': time.time()}))
                with patch('time.sleep'):
                    with patch('time.monotonic', side_effect=itertools.count(step=.5).__next__):
                        self.assertEqual(controller.stop_background(self.op)['status'], 'interrupted')
                self.assertFalse(any('stop' in call.args[0] for call in kube.call_args_list))

    def test_interrupted_docker_waits_out_the_window_once_and_still_looks_afterwards(self):
        import itertools
        import json
        import tempfile
        import time
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            running = [False]
            def answer(args, **kwargs):
                if 'claim' in args:
                    return SimpleNamespace(returncode=0, stdout=b'{"created":false}')
                return SimpleNamespace(returncode=0, stdout=json.dumps({'running': running[0]}).encode())
            kube = Mock(side_effect=answer)
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op,
                'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'background': True, 'docker_enabled': True,
                'status': 'interrupted', 'terminal_at': time.time()}))
            with patch('time.sleep') as sleep:
                with patch('time.monotonic', side_effect=itertools.count(step=.5).__next__):
                    controller.stop_background(self.op)
                    self.assertTrue(sleep.called)
                    sleep.reset_mock()
                    self.assertEqual(controller.stop_background(self.op)['status'], 'interrupted')
                    self.assertFalse(sleep.called)
                    self.assertFalse(any('stop' in call.args[0] for call in kube.call_args_list))
                    # A supervisor that published after the window still gets its stop.
                    running[0] = True
                    self.assertEqual(controller.stop_background(self.op)['status'], 'stopping')
            self.assertTrue(any('stop' in call.args[0] for call in kube.call_args_list))

    def test_interrupted_docker_retries_stop_and_cancel_continues_after_error(self):
        import json
        import tempfile
        import time
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController, ExecutionPodError
        with tempfile.TemporaryDirectory() as directory:
            attempts = []
            def kube(args, **kwargs):
                if 'claim' in args:
                    return SimpleNamespace(returncode=0, stdout=b'{"created":false}')
                if 'running' in args:
                    return SimpleNamespace(returncode=0, stdout=b'{"running":true}')
                attempts.append(args[-1])
                if len(attempts) == 1:
                    raise RuntimeError('temporary')
                return SimpleNamespace(returncode=0, stdout=b'')
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op, 'scratch_space_id': 's',
                'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'background': False, 'docker_enabled': True,
                'status': 'interrupted', 'terminal_at': time.time()}))
            with patch('time.sleep'):
                self.assertEqual(controller._stop_command(self.op)['status'], 'stopping')
            self.assertEqual(len(attempts), 2)
            other = 'e6ded09d-f783-4ce8-98e9-dbfca68884f2'
            controller._command_path(other).write_text(json.dumps({'operation_id': other, 'scratch_space_id': 's',
                'run_id': 'r', 'background': False, 'status': 'running'}))
            def stop(op):
                if op == self.op:
                    raise RuntimeError('cannot stop')
            controller._stop_command = Mock(side_effect=stop)
            with self.assertRaises(ExecutionPodError):
                controller.cancel_run('s', 'r')
            self.assertEqual({call.args[0] for call in controller._stop_command.call_args_list}, {self.op, other})

    def test_final_output_retries_transient_read_with_longer_timeout(self):
        import base64
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch
        from execution_pod import ExecutionPodController
        failures = [True]
        def kube(args, **kwargs):
            self.assertEqual(kwargs['timeout'], 15)
            if failures:
                failures.pop()
                raise TimeoutError('temporary')
            data = b'ok' if args[-3] == 'stdout' else b''
            return SimpleNamespace(returncode=0, stdout=json.dumps({'data': base64.b64encode(data).decode(),
                'next_offset': len(data), 'eof': True}).encode())
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            with patch('time.sleep'):
                result = controller._saved_output({'operation_id': self.op, 'pod': 'pod'}, {}, tolerate_missing=True)
            self.assertEqual(base64.b64decode(result['stdout']), b'ok')
            self.assertFalse(result['truncated'])
            failures.append(True)
            with patch('time.sleep'):
                result = controller._saved_output({'operation_id': self.op, 'pod': 'pod'}, {})
            self.assertEqual(base64.b64decode(result['stdout']), b'ok')
            self.assertFalse(result['truncated'])

    def test_partial_output_survives_missing_saved_stream_and_lost_pod(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            backend = SimpleNamespace(root=Path(directory), kube=Mock(return_value=SimpleNamespace(returncode=1, stdout=b'')))
            controller = ExecutionPodController(backend, {}, admitted=False)
            record = {'operation_id': self.op, 'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod',
                      'pod_uid': 'uid', 'status': 'running', 'background': True, 'stdout': 'b2xk', 'stderr': ''}
            controller._command_path(self.op).write_text(json.dumps(record))
            controller.live_output[self.op] = {'stdout': 'cHJldmlldw==', 'stderr': '', 'truncated': False}
            with patch('execution_pod.OUTPUT_RETRY_SECONDS', 0):
                output = controller._saved_output(record, {}, tolerate_missing=True)
            self.assertEqual(output['stdout'], 'cHJldmlldw==')
            self.assertTrue(output['truncated'])
            controller._get = Mock(return_value=None)
            controller._saved_output = Mock(side_effect=RuntimeError('offline'))
            with patch('time.sleep'):
                controller._await_normal_result(record, None, publish_saved=True)
            self.assertEqual(controller._read_command(self.op)['stdout'], 'cHJldmlldw==')

    def test_preview_reads_only_new_bytes(self):
        import base64
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController, BoundedOutput
        values = {'stdout': b'first', 'stderr': b''}
        offsets = []
        def kube(args, **kwargs):
            stream, offset = args[-3], int(args[-2])
            offsets.append((stream, offset))
            data = values[stream][offset:]
            return SimpleNamespace(returncode=0, stdout=json.dumps({'data': base64.b64encode(data).decode(),
                'next_offset': offset + len(data), 'eof': True}).encode())
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            state = {'output': BoundedOutput(100), 'offsets': {'stdout': 0, 'stderr': 0}}
            record = {'operation_id': self.op, 'pod': 'pod'}
            controller._preview_saved_output(record, state)
            values['stdout'] += b' more'
            result = controller._preview_saved_output(record, state)
            self.assertEqual(base64.b64decode(result['stdout']), b'first more')
            self.assertIn(('stdout', 5), offsets)

    def test_saved_output_retry_is_bounded_and_preserves_previous_bytes(self):
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch
        from execution_pod import ExecutionPodController
        clock = [0.0]
        attempts = []
        def kube(args, **kwargs):
            attempts.append(kwargs['timeout'])
            raise TimeoutError('offline')
        def advance(seconds):
            clock[0] += seconds
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            record = {'operation_id': self.op, 'pod': 'pod', 'stdout': 'cGFydA=='}
            with patch('time.monotonic', side_effect=lambda: clock[0]), patch('time.sleep', side_effect=advance):
                outcome = {}
                result = controller._saved_output(record, outcome, tolerate_missing=True)
            self.assertEqual(clock[0], 60)
            self.assertEqual(result['stdout'], 'cGFydA==')
            self.assertTrue(result['truncated'])
            self.assertTrue(outcome['output_incomplete'])
            self.assertTrue(all(value <= 15 for value in attempts))

    def test_resumed_polling_uses_separate_foreground_and_background_intervals(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        for background, expected in ((False, 2), (True, 5)):
            with self.subTest(background=background), tempfile.TemporaryDirectory() as directory:
                backend = SimpleNamespace(root=Path(directory), kube=Mock(side_effect=[
                    SimpleNamespace(returncode=1, stdout=b''),
                    SimpleNamespace(returncode=0, stdout=b'{"running":true}'),
                    SimpleNamespace(returncode=0, stdout=b'{"status":"completed"}')]))
                controller = ExecutionPodController(backend, {}, admitted=False)
                controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
                controller._preview_saved_output = Mock(side_effect=RuntimeError('unavailable'))
                record = {'operation_id': self.op, 'pod': 'pod', 'pod_uid': 'uid', 'status': 'running',
                          'background': background}
                controller._command_path(self.op).write_text(json.dumps(record))
                with patch('time.sleep') as sleep:
                    controller._await_normal_result(record, None, publish_saved=True)
                sleep.assert_called_once_with(expected)

    def test_late_result_keeps_prior_output_when_saved_files_are_missing(self):
        import json
        import tempfile
        import time
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        def kube(args, **kwargs):
            if 'result' in args:
                return SimpleNamespace(returncode=0, stdout=json.dumps({'operation_id': self.op, 'run_id': 'r',
                    'status': 'completed', 'exit_code': 0}).encode())
            return SimpleNamespace(returncode=1, stdout=b'')
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op, 'run_id': 'r',
                'pod': 'pod', 'pod_uid': 'uid', 'status': 'interrupted', 'terminal_at': time.time(),
                'stdout': 'cGFydA==', 'stderr': ''}))
            with patch('execution_pod.OUTPUT_RETRY_SECONDS', 0):
                result = controller.command_result(self.op)
            self.assertEqual(result['stdout'], 'cGFydA==')
            self.assertTrue(result['truncated'])
            self.assertEqual(result['outcome'], 'known')

    def test_docker_stop_failure_is_retried_until_deadline(self):
        import json
        import tempfile
        import time
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController, ExecutionPodError
        clock, stops = [0.0], []
        def kube(args, **kwargs):
            if 'claim' in args:
                return SimpleNamespace(returncode=0, stdout=b'{"created":false}')
            if 'running' in args:
                return SimpleNamespace(returncode=0, stdout=b'{"running":true}')
            stops.append(1)
            raise RuntimeError('offline')
        def tick(seconds):
            clock[0] += seconds
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op, 'run_id': 'r',
                'pod': 'pod', 'pod_uid': 'uid', 'background': True, 'docker_enabled': True,
                'status': 'interrupted', 'terminal_at': time.time()}))
            with patch('time.monotonic', side_effect=lambda: clock[0]), patch('time.sleep', side_effect=tick):
                with self.assertRaises(ExecutionPodError) as error:
                    controller.stop_background(self.op)
            self.assertEqual(error.exception.code, 'stop_failed')
            self.assertGreater(len(stops), 1)
            self.assertGreaterEqual(clock[0], 10)

    def test_normal_completion_does_not_acquire_known_outcome(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
            record = {'operation_id': self.op, 'status': 'running', 'pod': 'pod'}
            controller._command_path(self.op).write_text(json.dumps(record))
            controller._finish_command(record, {'status': 'completed', 'exit_code': 0},
                                       {'stdout': '', 'stderr': '', 'truncated': False})
            self.assertNotIn('outcome', controller._read_command(self.op))

    def test_late_python_result_never_deletes_pod_when_stop_fails(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
            record = {'operation_id': self.op, 'status': 'interrupted', 'pod': 'pod', 'pod_uid': 'uid',
                      'scratch_space_id': 's', 'kind': 'python_cell', 'session_operation_id': 'session'}
            controller._command_path(self.op).write_text(json.dumps(record))
            controller._stop_python_interpreter = Mock(side_effect=RuntimeError('offline'))
            controller._stop_matching_pod = Mock()
            controller._finish_command(record, {'status': 'stopped'}, {'stdout': '', 'stderr': '',
                                       'truncated': False}, allow_late=True)
            controller._stop_matching_pod.assert_not_called()
            self.assertEqual(controller._read_command(self.op)['status'], 'stopped')

    def test_unconfigured_helper_image_does_not_retire_pod(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=False)
            controller.touch_space('s', now=1)
            controller._get = Mock(return_value={'spec': {'containers': [{'name': 'execution', 'image': 'old'}]}})
            controller.stop = Mock()
            controller.stop_idle(now=2)
            controller.stop.assert_not_called()

    def test_recovery_ignores_corrupt_and_purged_spaces(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from control import scratch_resource_name
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            spaces = root / 'execution-v2-spaces'
            spaces.mkdir()
            (spaces / 'bad.json').write_text('{')
            purged = spaces / (scratch_resource_name('purged') + '.json')
            purged.write_text(json.dumps({'scratch_space_id': 'purged', 'purged': True}))
            partial = spaces / (scratch_resource_name('partial') + '.json')
            partial.write_text(json.dumps({'scratch_space_id': 'partial', 'epoch': 1}))
            controller = ExecutionPodController(Mock(root=root), {}, admitted=False)
            operation = controller._command_path(self.op)
            operation.write_text(json.dumps({'operation_id': self.op, 'status': 'queued'}))
            (operation.parent / 'bad.json').write_text('{')
            with patch('time.time', return_value=2000):
                controller.recover_commands()
            self.assertEqual(controller._read_command(self.op)['status'], 'cancelled')
            self.assertEqual(json.loads(partial.read_text())['last_command_at'], 2000)
            self.assertNotIn('last_command_at', json.loads(purged.read_text()))

class DockerFramingRegressionTest(unittest.TestCase):
    def test_framed_result_survives_bounded_output_and_preserves_exit_code(self):
        import json
        import os
        import sys
        from execution_pod import drain_process
        nonce = 'a' * 64
        result = {'operation_id': 'op', 'run_id': 'run', 'status': 'completed', 'exit_code': 127}
        program = ("import os,sys; token=sys.stdin.readline().strip(); "
                   "os.write(2,b'x'*10000); "
                   "os.write(2,b'\\x1eaidash-result:'+token.encode()+b':'+" + repr(json.dumps(result).encode()) + "+b'\\n')")
        transport = drain_process([sys.executable, '-c', program], os.environ, 16, 3, frame_token=nonce)
        self.assertEqual(transport['framed_result'], result)
        self.assertTrue(transport['output']['truncated'])


class PodBindingTest(unittest.TestCase):
    def test_replacement_pod_rejects_command_before_execution(self):
        from unittest.mock import patch
        from sandbox_v2 import supervise_command
        with patch.dict('os.environ', {'AIDASH_POD_UID': 'replacement'}):
            with self.assertRaisesRegex(ValueError, 'Pod identity'):
                supervise_command('e6ded09d-f783-4ce8-98e9-dbfca68884f1', 'run', '.', 'true',
                                  5, False, expected_pod_uid='original')


class CommandJournalTest(unittest.TestCase):
    def test_recovery_schedules_running_without_stopping_pod(self):
        import json
        import tempfile
        import threading
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=False)
            operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            path = controller._command_path(operation)
            path.write_text(json.dumps({'operation_id': operation, 'scratch_space_id': 's', 'run_id': 'r',
                                        'pod_uid': 'uid', 'status': 'running', 'background': False}))
            started, release = threading.Event(), threading.Event()
            def resume(_operation):
                started.set()
                release.wait(3)
            controller._resume_command = resume
            controller.recover_commands()
            try:
                self.assertTrue(started.wait(2))
                self.assertEqual(json.loads(path.read_text())['status'], 'running')
                self.assertEqual(controller.foreground_jobs['s'], 1)
                controller.backend.kube.assert_not_called()
            finally:
                release.set()
                controller.queues['s'].close()

    def test_python_session_resumes_on_its_own_lane_when_admission_fails(self):
        import json
        import tempfile
        import threading
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=False)
            operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            controller._command_path(operation).write_text(json.dumps({'operation_id': operation,
                'scratch_space_id': 's', 'run_id': 'r', 'status': 'running', 'background': True,
                'kind': 'python_session'}))
            started, release = threading.Event(), threading.Event()
            controller._resume_command = lambda _: (started.set(), release.wait(3))
            controller.recover_commands()
            try:
                self.assertTrue(started.wait(2))
                self.assertIn(operation, controller.futures)
                self.assertEqual(controller._read_command(operation)['status'], 'running')
                self.assertEqual(controller.foreground_jobs.get('s', 0), 0)
            finally:
                release.set()
                controller.queues['s'].close()

    def test_stopping_background_retries_until_supervisor_is_ready(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController

        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        class Backend:
            def __init__(self, root):
                self.root = Path(root)
                self.attempts = 0
            def kube(self, args, **_kwargs):
                if args[0] == 'get':
                    return SimpleNamespace(stdout=b'{"metadata":{"uid":"pod-uid"}}')
                self.attempts += 1
                if self.attempts == 1:
                    raise RuntimeError('supervisor not ready')
                return SimpleNamespace(stdout=b'')
        with tempfile.TemporaryDirectory() as directory:
            backend = Backend(directory)
            controller = ExecutionPodController(backend, {}, admitted=True)
            path = controller._command_path(operation)
            path.write_text(json.dumps({'operation_id': operation, 'background': True,
                'status': 'running', 'pod': 'pod', 'pod_uid': 'pod-uid'}))
            self.assertEqual(controller.stop_background(operation)['status'], 'stopping')
            self.assertEqual(backend.attempts, 2)

    def test_command_result_is_journaled_and_repeated_id_is_not_replayed(self):
        import json
        import tempfile
        import time
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch
        from execution_pod import ExecutionPodController

        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        class Backend:
            def __init__(self, root):
                self.root = Path(root)
                self.command = ['kubectl']
                self.environment = {}
            def kube(self, args, data=None, **_kwargs):
                if args[0] == 'get':
                    body = {'metadata': {'uid': 'pod-uid'}, 'status': {'phase': 'Running'},
                            'spec': {'containers': [{'name': 'execution', 'securityContext': {
                                'capabilities': {'add': ['SYS_ADMIN']}}}]}}
                else:
                    body = {'operation_id': operation, 'run_id': 'run', 'status': 'completed', 'exit_code': 0}
                return SimpleNamespace(stdout=json.dumps(body).encode(), returncode=0)
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Backend(directory), {'output_bytes': 1024}, admitted=True)
            controller.touch_space('scratch')
            controller._set_admitted_uid('scratch', 'pod-uid')
            request = {'operation_id': operation, 'scratch_space_id': 'scratch', 'run_id': 'run',
                       'code': 'echo hi'}
            captured = {'exit_code': 0, 'transport_timed_out': False,
                        'output': {'stdout': 'aGkK', 'stderr': '', 'truncated': False}}
            with patch('execution_pod.drain_process', return_value=captured) as transport:
                controller.submit_command(request)
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline:
                    result = controller.command_result(operation)
                    if result['status'] == 'completed':
                        break
                    time.sleep(.01)
                self.assertEqual(result['status'], 'completed')
                self.assertEqual(result['stdout'], 'aGkK')
                controller.submit_command(request)
                self.assertEqual(transport.call_count, 1)

    def test_changelog_is_read_in_bounded_chunks(self):
        import base64
        import tempfile
        from pathlib import Path
        from sandbox_v2 import log_chunk
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'changes.jsonl'
            path.write_bytes(b'abcdefgh')
            first = log_chunk(path, 0, 3)
            second = log_chunk(path, first['next_offset'], 5)
            self.assertEqual(base64.b64decode(first['data']), b'abc')
            self.assertFalse(first['eof'])
            self.assertEqual(base64.b64decode(second['data']), b'defgh')
            self.assertTrue(second['eof'])


class AdmissionLifecycleRegressionTest(unittest.TestCase):
    def test_unverified_running_pod_rejects_commands(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController, ExecutionPodError
        class Backend:
            def __init__(self, root): self.root = Path(root)
            def kube(self, *args, **kwargs):
                return SimpleNamespace(stdout=json.dumps({'metadata': {'uid': 'pod-uid'},
                    'status': {'phase': 'Running'}, 'spec': {'containers': [{'name': 'execution'}]}}).encode())
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Backend(directory), {}, admitted=True)
            with self.assertRaises(ExecutionPodError) as error:
                controller.submit_command({'operation_id': 'e6ded09d-f783-4ce8-98e9-dbfca68884f1',
                    'scratch_space_id': 's', 'run_id': 'r', 'code': 'true'})
            self.assertEqual(error.exception.code, 'admission_unverified')

    def test_stopping_one_space_does_not_hold_controller_lock_on_kubernetes(self):
        import tempfile
        import threading
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController
        started, release = threading.Event(), threading.Event()
        with tempfile.TemporaryDirectory() as directory:
            class Backend:
                root = Path(directory)
                def kube(self, args, **kwargs):
                    if args[0] == 'delete':
                        started.set()
                        release.wait(3)
                    return SimpleNamespace(stdout=b'')
            controller = ExecutionPodController(Backend(), {}, admitted=True)
            worker = threading.Thread(target=controller.stop, args=('s',))
            worker.start()
            try:
                self.assertTrue(started.wait(2))
                self.assertTrue(controller.lock.acquire(timeout=.5))
                controller.lock.release()
            finally:
                release.set()
                worker.join(3)

    def test_failed_delete_does_not_reuse_terminated_pod_admission(self):
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            class Backend:
                root = Path(directory)
                def kube(self, args, **kwargs):
                    if args[0] == 'delete':
                        raise RuntimeError('API unavailable')
                    return SimpleNamespace(stdout=b'{"metadata":{"uid":"pod-uid"},"status":{"phase":"Pending"}}')
            controller = ExecutionPodController(Backend(), {}, admitted=True)
            controller.touch_space('s')
            controller._set_admitted_uid('s', 'pod-uid')
            with self.assertRaisesRegex(RuntimeError, 'unavailable'):
                controller.stop('s')
            self.assertIsNone(controller._admitted_uid('s'))
            with self.assertRaisesRegex(RuntimeError, 'termination'):
                controller._require_space_termination_confirmed('s')

    def test_reset_does_not_hold_controller_lock_while_ensuring(self):
        import threading
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        controller = ExecutionPodController(Mock(), {}, admitted=True)
        controller.stop = Mock()
        def ensure(settings):
            obtained = []
            thread = threading.Thread(target=lambda: (controller.lock.acquire(), obtained.append(True), controller.lock.release()))
            thread.start()
            thread.join(.5)
            self.assertEqual(obtained, [True])
            return {}
        controller.ensure = ensure
        controller.reset({'scratch_space_id': 's'})


class ReviewThreadRegressionTest(unittest.TestCase):
    def test_kubernetes_storage_quantities_compare_by_value(self):
        from execution_pod import quantity_bytes
        for value in ('1000000000', '1G', '1000000k', '1e9'):
            with self.subTest(value=value):
                self.assertEqual(quantity_bytes(value), 1000000000)
        self.assertEqual(quantity_bytes('1Gi'), 1073741824)
        self.assertNotEqual(quantity_bytes('1G'), quantity_bytes('1Gi'))
        with self.assertRaises(ValueError):
            quantity_bytes('1banana')

    def test_python_session_capacity_is_per_space(self):
        import threading
        from execution_pod import CommandQueue
        first, second = CommandQueue(), CommandQueue()
        release = threading.Event()
        started = threading.Barrier(9)
        try:
            for _ in range(8):
                first.submit_session('hold', lambda _: (started.wait(timeout=3), release.wait(3)))
            started.wait(timeout=3)
            self.assertEqual(second.submit_session('other', lambda value: value).result(timeout=2), 'other')
        finally:
            release.set()
            first.close()
            second.close()

    def test_stopping_a_space_retires_its_executor_without_waiting_for_workers(self):
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import CommandQueue, ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            class Backend:
                root = Path(directory)
                def kube(self, args, **kwargs):
                    return SimpleNamespace(stdout=b'')
            controller = ExecutionPodController(Backend(), {}, admitted=True)
            queue = CommandQueue()
            controller.queues['s'] = queue
            controller.stop('s')
            self.assertNotIn('s', controller.queues)
            with self.assertRaises(RuntimeError):
                queue.submit('r', 'command', lambda value: value)

    def test_foreground_completion_refreshes_idle_timestamp(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from control import scratch_resource_name
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
            operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            controller.touch_space('s', now=1)
            controller.foreground_jobs['s'] = 1
            controller._command_path(operation).write_text(json.dumps({'operation_id': operation,
                'scratch_space_id': 's', 'background': False, 'status': 'completed'}))
            with patch('time.time', return_value=3600):
                controller._cancelled_command(operation, Mock(cancelled=lambda: False))
            path = Path(directory) / 'execution-v2-spaces' / (scratch_resource_name('s') + '.json')
            self.assertEqual(json.loads(path.read_text())['last_command_at'], 3600)


class PythonCancellationRegressionTest(unittest.TestCase):
    def test_stopped_cell_waits_for_its_interpreter_to_stop(self):
        import json
        import tempfile
        import threading
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        cell = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        session = 'e6ded09d-f783-4ce8-98e9-dbfca68884f2'
        requested = threading.Event()
        with tempfile.TemporaryDirectory() as directory:
            backend = SimpleNamespace(root=Path(directory), command=['kubectl'], environment={},
                                      kube=Mock(return_value=SimpleNamespace(returncode=0, stdout=b'')))
            controller = ExecutionPodController(backend, {}, admitted=True)
            cell_record = {'operation_id': cell, 'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod',
                           'pod_uid': 'uid', 'docker_enabled': False, 'background': False, 'timeout_seconds': 5,
                           'cwd': '.', 'code': 'true', 'status': 'queued', 'kind': 'python_cell',
                           'session_operation_id': session}
            controller._command_path(cell).write_text(json.dumps(cell_record))
            controller._command_path(session).write_text(json.dumps({'operation_id': session,
                'scratch_space_id': 's', 'run_id': 'r', 'background': True, 'status': 'running'}))
            controller.stop_background = Mock(side_effect=lambda _: (requested.set() or {'status': 'stopping'}))
            controller._await_normal_result = Mock(return_value={'operation_id': cell, 'run_id': 'r', 'status': 'stopped'})
            transport = {'exit_code': 0, 'transport_timed_out': False,
                         'output': {'stdout': '', 'stderr': '', 'truncated': False}}
            with patch('execution_pod.drain_process', return_value=transport):
                worker = threading.Thread(target=controller._execute_command, args=(cell,))
                worker.start()
                try:
                    self.assertTrue(requested.wait(2))
                    self.assertEqual(controller.command_result(cell)['status'], 'running')
                    controller._update_command(session, status='stopped')
                    worker.join(3)
                    self.assertFalse(worker.is_alive())
                    self.assertEqual(controller.command_result(cell)['status'], 'stopped')
                finally:
                    controller._update_command(session, status='stopped')
                    worker.join(3)


    def test_unconfirmed_interpreter_stop_retires_original_pod(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        cell = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as directory:
            backend = SimpleNamespace(root=Path(directory), command=['kubectl'], environment={},
                                      kube=Mock(return_value=SimpleNamespace(returncode=0, stdout=b'')))
            controller = ExecutionPodController(backend, {}, admitted=True)
            controller._command_path(cell).write_text(json.dumps({'operation_id': cell, 'scratch_space_id': 's',
                'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'docker_enabled': False, 'background': False,
                'timeout_seconds': 5, 'cwd': '.', 'code': 'true', 'status': 'queued', 'kind': 'python_cell',
                'session_operation_id': 'e6ded09d-f783-4ce8-98e9-dbfca68884f2'}))
            controller._await_normal_result = Mock(return_value={'operation_id': cell, 'run_id': 'r', 'status': 'stopped'})
            controller._stop_python_interpreter = Mock(side_effect=RuntimeError('unconfirmed'))
            controller._stop_matching_pod = Mock()
            transport = {'exit_code': 0, 'transport_timed_out': False,
                         'output': {'stdout': '', 'stderr': '', 'truncated': False}}
            with patch('execution_pod.drain_process', return_value=transport):
                controller._execute_command(cell)
            controller._stop_matching_pod.assert_called_once_with('s', 'uid')


class ConcurrentOperationOwnershipRegressionTest(unittest.TestCase):
    def test_same_operation_id_cannot_be_claimed_by_two_spaces(self):
        import tempfile
        import threading
        from concurrent.futures import ThreadPoolExecutor
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController, ExecutionPodError
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        rendezvous, release, worker_started = threading.Barrier(2), threading.Event(), threading.Event()
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory)), {}, admitted=True)
            def get(_kind, _name):
                rendezvous.wait(timeout=3)
                return {'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'},
                        'spec': {'containers': [{'name': 'execution'}]}}
            controller._get = get
            controller._admitted_uid = Mock(return_value='uid')
            starts = []
            def execute(op):
                starts.append(op)
                worker_started.set()
                release.wait(3)
            controller._execute_command = execute
            def submit(space):
                try:
                    return controller.submit_command({'operation_id': operation, 'scratch_space_id': space,
                                                      'run_id': 'run', 'code': 'true'})
                except ExecutionPodError as error:
                    return error.code
            try:
                with ThreadPoolExecutor(max_workers=2) as pool:
                    results = list(pool.map(submit, ('space-a', 'space-b')))
                self.assertEqual(sum(item == 'command_conflict' for item in results), 1)
                self.assertEqual(sum(isinstance(item, dict) for item in results), 1)
                self.assertTrue(worker_started.wait(2))
                self.assertEqual(len(starts), 1)
            finally:
                release.set()
                for queue in controller.queues.values():
                    queue.close()


class RecoveryRegressionTest(unittest.TestCase):
    def test_restart_refreshes_idle_clocks_even_without_commands(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from control import scratch_resource_name
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=False)
            controller.touch_space('s', now=1)
            with patch('time.time', return_value=3000):
                controller.recover_commands()
            path = Path(directory) / 'execution-v2-spaces' / (scratch_resource_name('s') + '.json')
            self.assertEqual(json.loads(path.read_text())['last_command_at'], 3000)

    def test_recovery_cancels_queued_and_interrupts_docker(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=False)
            for operation, status, docker in [('e6ded09d-f783-4ce8-98e9-dbfca68884f1', 'queued', False),
                                              ('e6ded09d-f783-4ce8-98e9-dbfca68884f2', 'running', True)]:
                controller._command_path(operation).write_text(json.dumps({'operation_id': operation,
                    'scratch_space_id': 's', 'run_id': 'r', 'status': status, 'docker_enabled': docker}))
            controller.recover_commands()
            queued = controller._read_command('e6ded09d-f783-4ce8-98e9-dbfca68884f1')
            docker = controller._read_command('e6ded09d-f783-4ce8-98e9-dbfca68884f2')
            self.assertEqual((queued['status'], queued['outcome']), ('cancelled', 'not_started'))
            self.assertEqual((docker['status'], docker['outcome']), ('interrupted', 'unknown'))
            controller.backend.kube.assert_not_called()

    def test_idle_monitor_retries_unconfirmed_spaces_independently(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
            controller.touch_space('first', now=1)
            controller.touch_space('second', now=1)
            controller._mark_space_unconfirmed('first')
            controller._mark_space_unconfirmed('second')
            calls = []
            def stop(space):
                calls.append(space)
                if len(calls) == 1:
                    raise RuntimeError('node unavailable')
            controller.stop = stop
            controller.stop_idle(now=2)
            self.assertEqual(set(calls), {'first', 'second'})

    def test_pending_container_status_can_be_deleted_after_failed_start(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from control import scratch_resource_name
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            class Backend:
                root = Path(directory)
                def kube(self, args, **kwargs):
                    if args[0] == 'get':
                        return SimpleNamespace(stdout=json.dumps({'metadata': {'uid': 'pending'},
                            'status': {'phase': 'Pending', 'containerStatuses': [
                                {'name': 'execution', 'state': {'waiting': {}}}]}}).encode())
                    return SimpleNamespace(stdout=b'')
            controller = ExecutionPodController(Backend(), {}, admitted=True)
            path = Path(directory) / 'execution-v2-spaces' / (scratch_resource_name('s') + '.json')
            path.parent.mkdir()
            path.write_text(json.dumps({'scratch_space_id': 's', 'last_command_at': 1,
                                        'termination_unconfirmed': True}))
            controller.stop('s')
            controller._require_space_termination_confirmed('s')


class BackgroundListingRegressionTest(unittest.TestCase):
    def test_listing_omits_output_and_old_terminal_records(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
            for op, status, finished in [('e6ded09d-f783-4ce8-98e9-dbfca68884f1', 'running', None),
                                         ('e6ded09d-f783-4ce8-98e9-dbfca68884f2', 'completed', 1)]:
                controller._command_path(op).write_text(json.dumps({'operation_id': op, 'scratch_space_id': 's',
                    'run_id': 'r', 'background': True, 'status': status, 'terminal_at': finished,
                    'stdout': 'large', 'stderr': 'large'}))
            with patch('time.time', return_value=1000000):
                listed = controller.background_processes('s')['processes']
            self.assertEqual(len(listed), 1)
            self.assertNotIn('stdout', listed[0])
            self.assertNotIn('stderr', listed[0])


    def test_recent_interrupt_retains_old_background_command_in_listing(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
            operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            controller._command_path(operation).write_text(json.dumps({'operation_id': operation,
                'scratch_space_id': 's', 'run_id': 'r', 'background': True, 'status': 'running', 'accepted_at': 1}))
            with patch('time.time', return_value=1000000):
                controller._interrupt_space_commands('s')
                listed = controller.background_processes('s')['processes']
            self.assertEqual(len(listed), 1)
            self.assertEqual(listed[0]['status'], 'interrupted')


class PurgeLocalDataRegressionTest(unittest.TestCase):
    def test_purge_removes_only_the_deleted_spaces_journals(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            class Backend:
                root = Path(directory)
                def kube(self, args, **kwargs):
                    return SimpleNamespace(stdout=b'')
            controller = ExecutionPodController(Backend(), {}, admitted=True)
            for op, space in [('e6ded09d-f783-4ce8-98e9-dbfca68884f1', 's'),
                              ('e6ded09d-f783-4ce8-98e9-dbfca68884f2', 'other')]:
                controller._command_path(op).write_text(json.dumps({'operation_id': op, 'scratch_space_id': space,
                    'status': 'running', 'code': 'secret', 'stdout': 'data'}))
            for space in ('s', 'other'):
                controller.touch_space(space)
                controller._python_session_path(space, 'r').write_text(json.dumps({'scratch_space_id': space}))
            controller.delete('s')
            self.assertFalse(controller._command_path('e6ded09d-f783-4ce8-98e9-dbfca68884f1').exists())
            self.assertTrue(controller._command_path('e6ded09d-f783-4ce8-98e9-dbfca68884f2').exists())
            self.assertFalse(controller._python_session_path('s', 'r').exists())
            self.assertTrue(controller._python_session_path('other', 'r').exists())


class RunCancellationRegressionTest(unittest.TestCase):
    def test_cancel_signals_running_foreground_but_leaves_background(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            class Backend:
                root = Path(directory)
                calls = []
                def kube(self, args, **kwargs):
                    self.calls.append(args)
                    if args[0] == 'get':
                        return SimpleNamespace(stdout=b'{"metadata":{"uid":"pod-uid"}}')
                    return SimpleNamespace(stdout=b'')
            controller = ExecutionPodController(Backend(), {}, admitted=True)
            for op, background in [('e6ded09d-f783-4ce8-98e9-dbfca68884f1', False),
                                   ('e6ded09d-f783-4ce8-98e9-dbfca68884f2', True)]:
                controller._command_path(op).write_text(json.dumps({'operation_id': op, 'scratch_space_id': 's',
                    'run_id': 'r', 'background': background, 'status': 'running', 'pod': 'pod', 'pod_uid': 'pod-uid'}))
            controller.cancel_run('s', 'r')
            calls = [call for call in controller.backend.calls if call[0] == 'exec']
            self.assertEqual(len(calls), 1)
            self.assertEqual(calls[0][-1], 'e6ded09d-f783-4ce8-98e9-dbfca68884f1')


class IdlePodTest(unittest.TestCase):
    def test_live_viewers_and_foreground_work_keep_pod_but_background_does_not(self):
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController

        class Backend:
            def __init__(self, root):
                self.root = Path(root)
                self.deleted = []
            def kube(self, args, data=None, **_kwargs):
                import json
                if args[0] == 'get':
                    return SimpleNamespace(stdout=json.dumps({'metadata': {'uid': 'pod-uid'},
                                                               'status': {'phase': 'Pending'}}).encode())
                if args[0] == 'delete':
                    self.deleted.append(args[2])
                return SimpleNamespace(stdout=b'')
        with tempfile.TemporaryDirectory() as directory:
            backend = Backend(directory)
            controller = ExecutionPodController(backend, {}, admitted=True)
            controller.touch_space('scratch', now=1)
            controller.viewer_heartbeat('scratch', 'viewer', now=1800)
            controller.stop_idle(now=1802)
            self.assertFalse(backend.deleted)
            controller.stop_idle(now=3659)
            self.assertFalse(backend.deleted)
            controller.foreground_jobs['scratch'] = 1
            controller.stop_idle(now=4000)
            self.assertFalse(backend.deleted)
            controller.foreground_jobs['scratch'] = 0
            controller.stop_idle(now=4001)
            self.assertEqual(len(backend.deleted), 1)


class HelperUpdateTest(unittest.TestCase):
    def test_ensure_and_submit_refuse_old_helper_image(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController, ExecutionPodError
        settings = {'scratch_space_id': 's', 'image': 'image@sha256:' + 'a' * 64, 'cpu': 2,
                    'memory_bytes': 2 << 30, 'capacity_bytes': 2 << 30, 'docker_enabled': False}
        config = {'runtime_class': 'gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64, 'namespace': 'sandbox'}
        pod = {'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'},
               'spec': {'containers': [{'name': 'execution', 'image': 'old'}]}}
        claim = {'spec': {'resources': {'requests': {'storage': str(2 << 30)}}}}
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), config, admitted=True)
            controller._get = Mock(side_effect=lambda kind, name: claim if kind == 'pvc' else pod)
            with self.assertRaises(ExecutionPodError) as error:
                controller.ensure(settings)
            self.assertEqual(error.exception.code, 'pod_updating')
            with self.assertRaises(ExecutionPodError) as error:
                controller.submit_command({'operation_id': 'e6ded09d-f783-4ce8-98e9-dbfca68884f1',
                                           'scratch_space_id': 's', 'run_id': 'r', 'code': 'true'})
            self.assertEqual(error.exception.code, 'pod_updating')

    def test_update_waits_for_foreground_then_stops_even_with_viewer(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {'helper_image': 'new'}, admitted=True)
            controller.touch_space('s', now=100)
            controller.viewer_heartbeat('s', 'viewer', now=101)
            controller.recover_commands()
            controller._get = Mock(return_value={'spec': {'containers': [{'name': 'execution', 'image': 'old'}]}})
            controller.stop = Mock()
            controller.foreground_jobs['s'] = 1
            controller.stop_idle(now=102)
            controller.stop.assert_not_called()
            controller.foreground_jobs['s'] = 0
            controller.stop_idle(now=103)
            controller.stop.assert_called_once_with('s')

    def test_failed_update_stop_is_retried(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {'helper_image': 'new'}, admitted=True)
            controller.touch_space('s')
            controller.recover_commands()
            controller._get = Mock(return_value={'spec': {'containers': [{'name': 'execution', 'image': 'old'}]}})
            controller.stop = Mock(side_effect=[RuntimeError('guard unavailable'), {'stopped': True}])
            controller.stop_idle()
            controller.stop_idle()
            self.assertEqual(controller.stop.call_count, 2)

    def test_active_space_needs_one_pod_lookup_per_restart(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {'helper_image': 'new'}, admitted=True)
            controller.touch_space('s')
            controller.recover_commands()
            controller._get = Mock(return_value={'spec': {'containers': [{'name': 'execution', 'image': 'new'}]}})
            controller.stop = Mock()
            controller.stop_idle()
            controller.stop_idle()
            controller.stop_idle()
            controller._get.assert_called_once()
            controller.stop.assert_not_called()

class PythonSessionTest(unittest.TestCase):
    def test_cells_share_state_within_one_run(self):
        import os
        import subprocess
        import sys
        import tempfile
        import time
        from pathlib import Path
        from python_session import send_cell
        with tempfile.TemporaryDirectory() as directory:
            socket_path = os.path.relpath(Path(directory) / 'session.sock')
            server_path = os.path.relpath(Path(directory) / 'session.sock', Path(__file__).parent)
            server = subprocess.Popen([sys.executable, '-I', 'python_session.py', 'serve', server_path],
                                      cwd=Path(__file__).parent, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 3
                while not os.path.exists(socket_path) and time.monotonic() < deadline:
                    time.sleep(.01)
                first = send_cell(socket_path, 'value = 41', 2, 1024)
                second = send_cell(socket_path, 'value + 1', 2, 1024)
                self.assertEqual(first['status'], 'completed')
                self.assertEqual(second['stdout'], '42\n')
            finally:
                server.terminate()
                server.wait(timeout=3)
                server.stderr.close()

    def test_each_cell_uses_its_requested_working_directory(self):
        import os
        import subprocess
        import sys
        import tempfile
        import time
        from pathlib import Path
        from python_session import send_cell
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'child').mkdir()
            socket_path = os.path.relpath(root / 'session.sock')
            server = subprocess.Popen([sys.executable, '-I', str(Path(__file__).resolve().parent / 'python_session.py'),
                                       'serve', 'session.sock'], cwd=root, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 3
                while not os.path.exists(socket_path) and time.monotonic() < deadline:
                    time.sleep(.01)
                result = send_cell(socket_path, "open('relative', 'w').write('ok')", 2, 1024, cwd='child')
                self.assertEqual(result['status'], 'completed')
                self.assertEqual((root / 'child' / 'relative').read_text(), 'ok')
                self.assertEqual(send_cell(socket_path, "__import__('os').getcwd()", 2, 1024)['stdout'], repr(str(root.resolve())) + '\n')
            finally:
                server.terminate()
                server.wait(timeout=3)
                server.stderr.close()

    def test_cell_captures_file_descriptors_and_subprocess_output(self):
        import os
        import subprocess
        import sys
        import tempfile
        import time
        from pathlib import Path
        from python_session import send_cell
        with tempfile.TemporaryDirectory() as directory:
            socket_path = os.path.relpath(Path(directory) / 'session.sock')
            server_path = os.path.relpath(Path(directory) / 'session.sock', Path(__file__).parent)
            server = subprocess.Popen([sys.executable, '-I', 'python_session.py', 'serve', server_path],
                                      cwd=Path(__file__).parent, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 3
                while not os.path.exists(socket_path) and time.monotonic() < deadline:
                    time.sleep(.01)
                result = send_cell(socket_path, "import os, subprocess; os.write(1, b'fd!'); subprocess.run(['/bin/echo', 'child'])", 2, 128)
                self.assertIn('fd!', result['stdout'])
                self.assertIn('child', result['stdout'])
                limited = send_cell(socket_path, "__import__('os').write(2, b'x' * 1000)", 2, 10)
                self.assertEqual(len(limited['stdout']), 10)
                self.assertTrue(limited['truncated'])
            finally:
                server.terminate()
                server.wait(timeout=3)
                server.stderr.close()

    def test_cell_output_is_bounded(self):
        import os
        import subprocess
        import sys
        import tempfile
        import time
        from pathlib import Path
        from python_session import send_cell
        with tempfile.TemporaryDirectory() as directory:
            socket_path = os.path.relpath(Path(directory) / 'session.sock')
            server_path = os.path.relpath(Path(directory) / 'session.sock', Path(__file__).parent)
            server = subprocess.Popen([sys.executable, '-I', 'python_session.py', 'serve', server_path],
                                      cwd=Path(__file__).parent, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 3
                while not os.path.exists(socket_path) and time.monotonic() < deadline:
                    time.sleep(.01)
                result = send_cell(socket_path, 'print("x" * 100)', 2, 10)
                self.assertEqual(result['stdout'], 'x' * 10)
                self.assertTrue(result['truncated'])
            finally:
                server.terminate()
                server.wait(timeout=3)
                server.stderr.close()

    def test_cell_waits_for_interpreter_startup(self):
        import os
        import subprocess
        import sys
        import tempfile
        from pathlib import Path
        from python_session import send_cell
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.relpath(Path(directory) / 'delayed.sock')
            script = ('import time,sys; time.sleep(.2); '
                      'sys.path.insert(0,sys.argv[1]); from python_session import serve; serve(sys.argv[2])')
            server = subprocess.Popen([sys.executable, '-I', '-c', script, str(Path(__file__).parent), path],
                                      stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                result = send_cell(path, '2 + 3', 2, 1024)
                self.assertEqual(result['stdout'], '5\n')
            finally:
                server.terminate()
                server.wait(timeout=3)
                server.stderr.close()


class PythonSessionMappingTest(unittest.TestCase):
    def test_unknown_interrupted_session_is_not_restarted(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController, ExecutionPodError
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
            operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            controller._python_session_path('s', 'r').write_text(json.dumps({'operation_id':operation}))
            controller._command_path(operation).write_text(json.dumps({'operation_id':operation, 'status':'interrupted', 'pod_uid': 'uid'}))
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller.command_result = Mock(return_value={'status':'interrupted'})
            controller.submit_command = Mock()
            with self.assertRaises(ExecutionPodError) as error:
                controller.start_python('s', 'r')
            self.assertEqual(error.exception.code, 'session_interrupted')
            controller.submit_command.assert_not_called()

    def test_replaced_pod_starts_session_and_marks_cell(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {'output_bytes': 8388608}, admitted=True)
            operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            controller._python_session_path('s', 'r').write_text(json.dumps({'operation_id': operation}))
            controller._command_path(operation).write_text(json.dumps({'operation_id': operation,
                'status': 'interrupted', 'pod_uid': 'old'}))
            controller._get = Mock(return_value={'metadata': {'uid': 'new'}})
            controller.submit_command = Mock(return_value={'status': 'queued'})
            result = controller.python_cell({'operation_id': 'e6ded09d-f783-4ce8-98e9-dbfca68884f2',
                'scratch_space_id': 's', 'run_id': 'r', 'code': '1'})
            self.assertEqual(result['status'], 'queued')
            self.assertTrue(controller.submit_command.call_args.kwargs['session_replaced'])

    def test_replacement_notice_is_returned_once_not_persisted(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
            op = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            path = controller._python_session_path('s', 'r')
            path.write_text(json.dumps({'operation_id': op}))
            controller._command_path(op).write_text(json.dumps({'operation_id': op, 'status': 'interrupted', 'pod_uid': 'old'}))
            controller._get = Mock(return_value={'metadata': {'uid': 'new'}})
            controller.submit_command = Mock(return_value={'status': 'queued'})
            first = controller.start_python('s', 'r')
            self.assertTrue(first['session_replaced'])
            self.assertNotIn('session_replaced', json.loads(path.read_text()))
            controller._read_command = Mock(return_value={'status': 'running', 'pod_uid': 'new'})
            self.assertNotIn('session_replaced', controller.start_python('s', 'r'))

    def test_retried_cell_keeps_its_replacement_notice(self):
        import hashlib
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        request = {'operation_id': 'e6ded09d-f783-4ce8-98e9-dbfca68884f1',
                   'scratch_space_id': 's', 'run_id': 'r', 'code': '1'}
        digest = hashlib.sha256(json.dumps(request, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
            controller._command_path(request['operation_id']).write_text(json.dumps({'operation_id': request['operation_id'],
                'client_digest': digest, 'status': 'running', 'session_replaced': True}))
            self.assertTrue(controller.python_cell(request)['session_replaced'])

    def test_stopped_session_on_replaced_pod_is_not_reported_as_lost(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
            op = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            controller._python_session_path('s', 'r').write_text(json.dumps({'operation_id': op}))
            controller._command_path(op).write_text(json.dumps({'operation_id': op, 'status': 'stopped', 'pod_uid': 'old'}))
            controller._get = Mock(return_value={'metadata': {'uid': 'new'}})
            controller.submit_command = Mock(return_value={'status': 'queued'})
            self.assertFalse(controller.start_python('s', 'r').get('session_replaced', False))

    def test_session_is_reused_per_run_and_separate_across_runs(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            backend = Mock()
            backend.root = Path(directory)
            controller = ExecutionPodController(backend, {}, admitted=True)
            controller.submit_command = Mock(return_value={'status': 'queued'})
            controller._get = Mock(return_value={'metadata': {'uid': 'pod'}})
            controller._read_command = Mock(return_value={'status': 'running', 'pod_uid': 'pod'})
            first = controller.start_python('scratch', 'run-a')
            again = controller.start_python('scratch', 'run-a')
            second = controller.start_python('scratch', 'run-b')
            self.assertEqual(first, again)
            self.assertNotEqual(first['operation_id'], second['operation_id'])
            self.assertEqual(controller.submit_command.call_count, 2)


class BootstrapReadinessRegressionTest(unittest.TestCase):
    def test_bootstrap_reports_ready_only_after_mounts_and_initialization(self):
        import tempfile
        from pathlib import Path
        from unittest.mock import patch
        import sandbox_v2
        with tempfile.TemporaryDirectory() as directory:
            marker = str(Path(directory) / 'ready')
            tracker = str(Path(directory) / 'tracker')
            Path(tracker).write_text('stale')
            output = Path(directory) / 'output'
            output.mkdir()
            (output / 'old.stdout').write_text('old')
            def mount(_docker):
                self.assertFalse(sandbox_v2.bootstrap_ready())
                self.assertTrue(Path(tracker).exists())
            with patch.object(sandbox_v2, 'BOOTSTRAP_READY_PATH', marker, create=True), \
                 patch.object(sandbox_v2, 'BOOTSTRAP_TRACKER_PATH', tracker, create=True), \
                 patch.object(sandbox_v2, 'OUTPUT_DIRECTORY', str(Path(directory) / 'output')), \
                 patch.dict('os.environ', {'AIDASH_POD_UID': 'pod-uid', 'AIDASH_PROCESS_LIMIT': '128'}), \
                 patch('resource.setrlimit'), patch.object(sandbox_v2, 'mount_rootfs', side_effect=mount), \
                 patch('signal.pause', side_effect=InterruptedError):
                with self.assertRaises(InterruptedError):
                    sandbox_v2.bootstrap()
                self.assertTrue(sandbox_v2.bootstrap_ready())
                self.assertFalse(Path(tracker).exists())
                self.assertEqual(list(output.iterdir()), [])
                with patch.dict('os.environ', {'AIDASH_POD_UID': 'other'}):
                    self.assertFalse(sandbox_v2.bootstrap_ready())

    def test_bootstrap_continues_when_output_directory_cannot_be_created(self):
        import errno
        import tempfile
        from pathlib import Path
        from unittest.mock import patch
        import sandbox_v2
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'output'
            original_mkdir = Path.mkdir
            def mkdir(path, *args, **kwargs):
                if path == output:
                    raise OSError(errno.ENOSPC, 'full')
                return original_mkdir(path, *args, **kwargs)
            with patch.object(sandbox_v2, 'BOOTSTRAP_READY_PATH', str(Path(directory) / 'ready')), \
                 patch.object(sandbox_v2, 'BOOTSTRAP_TRACKER_PATH', str(Path(directory) / 'tracker')), \
                 patch.object(sandbox_v2, 'OUTPUT_DIRECTORY', str(output)), \
                 patch.dict('os.environ', {'AIDASH_POD_UID': 'uid', 'AIDASH_PROCESS_LIMIT': '128'}), \
                 patch('resource.setrlimit'), patch.object(sandbox_v2, 'mount_rootfs'), \
                 patch.object(Path, 'mkdir', mkdir), patch('signal.pause', side_effect=InterruptedError):
                with self.assertRaises(InterruptedError):
                    sandbox_v2.bootstrap()
                self.assertTrue(sandbox_v2.bootstrap_ready())

    def test_execution_container_readiness_uses_bootstrap_handshake(self):
        from execution_pod import execution_manifests, helper_command
        settings = {'scratch_space_id': 's', 'image': 'image@sha256:' + 'a' * 64, 'cpu': 2,
                    'memory_bytes': 2 << 30, 'capacity_bytes': 2 << 30, 'docker_enabled': False}
        config = {'runtime_class': 'gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64, 'namespace': 'sandbox'}
        _, pod = execution_manifests(settings, config)
        probe = pod['spec']['containers'][0]['readinessProbe']
        self.assertEqual(probe['exec']['command'], helper_command('ready'))


class GuardV2IdentityTest(unittest.TestCase):
    def test_guard_binds_scratch_pod_uid_container_and_runtime(self):
        import json
        import uuid
        from unittest.mock import patch
        import node_guard

        digest = 'a' * 48
        pod_uid = str(uuid.uuid4())
        request = {'protocol': 'aidash-runner/2', 'container_id': 'b' * 64, 'pod_uid': pod_uid,
                   'pod_name': 'scratch-' + digest, 'container_name': 'helpers',
                   'area_id': str(uuid.UUID(hex=digest[:32])), 'epoch': 1, 'image': 'image@sha256:' + 'c' * 64}
        info = {'ID': request['container_id'], 'Runtime': {'Name': 'io.containerd.runsc.v1'}, 'Labels': {
            'io.kubernetes.pod.namespace': node_guard.NAMESPACE, 'io.kubernetes.pod.uid': pod_uid,
            'io.kubernetes.container.name': 'helpers', 'io.kubernetes.pod.name': request['pod_name']}}
        state = {'id': request['container_id'], 'annotations': {'io.kubernetes.cri.image-name': request['image']}}
        with patch.object(node_guard, 'command', side_effect=[json.dumps(info).encode(), json.dumps(state).encode()]):
            self.assertEqual(node_guard.validate(request)[1], pod_uid)
        info['Labels']['io.kubernetes.pod.name'] = 'scratch-' + 'd' * 48
        with patch.object(node_guard, 'command', side_effect=[json.dumps(info).encode(), json.dumps(state).encode()]):
            with self.assertRaisesRegex(ValueError, 'binding'):
                node_guard.validate(request)


class GuardReviewRegressionTest(unittest.TestCase):
    def test_cleanup_handoff_preserves_v2_binding(self):
        import json
        import tempfile
        import uuid
        from pathlib import Path
        from unittest.mock import patch
        import node_guard
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record = {'container_id': 'a' * 64, 'pod_uid': str(uuid.uuid4()),
                      'area_id': str(uuid.uuid4()), 'epoch': 1, 'image': 'pinned', 'terminated': True,
                      'protocol': 'aidash-runner/2', 'pod_name': 'scratch-' + 'b' * 48, 'container_name': 'execution'}
            with patch.object(node_guard, 'ROOT', root):
                node_guard.handoff_cleanup(root / 'current.json', record)
            pending = json.loads((root / 'runtime-cleanup' / ('a' * 64 + '.json')).read_text())
            for key in ('protocol', 'pod_name', 'container_name'):
                self.assertEqual(pending[key], record[key])

    def test_new_v2_binding_cannot_replace_live_previous_sentry(self):
        import json
        import tempfile
        import uuid
        from pathlib import Path
        from unittest.mock import patch
        import node_guard
        with tempfile.TemporaryDirectory() as directory:
            root, area = Path(directory), str(uuid.uuid4())
            old = {'protocol': 'aidash-runner/2', 'area_id': area, 'pod_uid': str(uuid.uuid4()),
                   'container_id': 'a' * 64, 'epoch': 1, 'image': 'pinned', 'terminated': False}
            (root / (area + '.json')).write_text(json.dumps(old))
            request = dict(old, action='v2_limits', pod_uid=str(uuid.uuid4()), container_id='b' * 64, epoch=2)
            with patch.object(node_guard, 'ROOT', root), patch.object(node_guard, 'gone', return_value=False), \
                 patch.object(node_guard, 'validate', return_value=('b' * 64, request['pod_uid'], area, {'pid': 42})):
                with self.assertRaisesRegex(ValueError, 'previous.*live'):
                    node_guard.handle(request)


class GuardV2CapacityTest(unittest.TestCase):
    def test_unbounded_or_ambiguous_persistent_storage_is_rejected(self):
        import tempfile
        import uuid
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch
        import node_guard
        with tempfile.TemporaryDirectory() as directory:
            pod = str(uuid.uuid4())
            volumes = Path(directory) / pod / 'volumes'
            mount = volumes / 'kubernetes.io~csi' / 'pvc-test' / 'mount'
            mount.mkdir(parents=True)
            with patch.object(node_guard, 'KUBELET_PODS', Path(directory)), \
                 patch.object(node_guard.os, 'statvfs', return_value=SimpleNamespace(f_blocks=100, f_frsize=4096)):
                self.assertEqual(node_guard.scratch_capacity(pod, 409600), 409600)
                with self.assertRaisesRegex(ValueError, 'capacity'):
                    node_guard.scratch_capacity(pod, 409599)
                (volumes / 'kubernetes.io~local-volume' / 'pvc-other').mkdir(parents=True)
                with self.assertRaisesRegex(ValueError, 'exactly one'):
                    node_guard.scratch_capacity(pod, 409600)

    def test_guard_enforces_sentry_limits_before_admission(self):
        import tempfile
        import uuid
        from pathlib import Path
        from unittest.mock import patch
        import node_guard
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cgroups = root / 'cgroups'
            sentry = cgroups / 'sentry'
            sentry.mkdir(parents=True)
            for name, value in {'cpu.max': '200000 100000', 'memory.max': str(2 << 30),
                                'pids.max': 'max', 'memory.swap.max': 'max'}.items():
                (sentry / name).write_text(value)
            request = {'protocol': 'aidash-runner/2', 'action': 'v2_limits', 'area_id': str(uuid.uuid4()),
                       'pod_uid': str(uuid.uuid4()), 'container_id': 'a' * 64, 'epoch': 1,
                       'image': 'pinned', 'cpu': 2, 'memory_bytes': 2 << 30, 'host_tasks': 512,
                       'capacity_bytes': 100 << 30}
            with patch.object(node_guard, 'ROOT', root / 'guard'), \
                 patch.object(node_guard, 'CGROUP_ROOT', cgroups), \
                 patch.object(node_guard, 'sentry_cgroup', return_value=sentry), \
                 patch.object(node_guard, 'validate', return_value=('a' * 64, request['pod_uid'], request['area_id'], {'pid': 42})), \
                 patch.object(node_guard, 'binding', return_value={}), \
                 patch.object(node_guard, 'scratch_capacity', return_value=99 << 30):
                result = node_guard.handle(request)
                self.assertEqual(result['scratch_capacity_bytes'], 99 << 30)
                self.assertEqual((sentry / 'pids.max').read_text(), '512')
                self.assertEqual((sentry / 'memory.swap.max').read_text(), '0')
                (cgroups / 'pids.max').write_text('128')
                with self.assertRaisesRegex(ValueError, 'cgroup limits'):
                    node_guard.handle(request)


class InitGateTest(unittest.TestCase):
    def test_image_initialization_waits_for_guard_receipt(self):
        import tempfile
        import threading
        from pathlib import Path
        from prepare_helpers import wait_for_guard
        with tempfile.TemporaryDirectory() as directory:
            receipt = Path(directory) / 'guard-ready'
            with self.assertRaises(TimeoutError):
                wait_for_guard(receipt, timeout=.05)
            timer = threading.Timer(.05, receipt.touch)
            timer.start()
            try:
                wait_for_guard(receipt, timeout=1)
            finally:
                timer.join()


class RootAdmissionTest(unittest.TestCase):
    def test_normal_user_cannot_reacquire_docker_capabilities(self):
        from execution_pod import verify_user_probe
        report = {'uid': 0, 'kernel': '4.19.0-gvisor', 'no_new_privs': 1,
                  'caps': {name: 0 for name in ('CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb')},
                  'process_limit': [128, 128], 'process_limit_enforced': True, 'process_limit_unraiseable': True,
                  'system_absent': True, 'user_namespace_blocked': True, 'proc_root_blocked': True,
                  'service_account_absent': True, 'raw_socket': False,
                  'root_writable': True, 'helpers_read_only': True}
        verify_user_probe(report, docker=False, processes=128)
        verify_user_probe(dict(report, root_writable=False, root_full=True), docker=False, processes=128)
        with self.assertRaisesRegex(ValueError, 'system root'):
            verify_user_probe(dict(report, root_writable=False, root_full=False), docker=False, processes=128)
        for key in ('system_absent', 'user_namespace_blocked', 'proc_root_blocked'):
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'chroot'):
                verify_user_probe(dict(report, **{key: False}), docker=False, processes=128)
        for field in report['caps']:
            for capability in (18, 21):
                bad = dict(report, caps=dict(report['caps'], **{field: 1 << capability}))
                with self.subTest(field=field, capability=capability), self.assertRaisesRegex(ValueError, 'capabilit'):
                    verify_user_probe(bad, docker=False, processes=128)
        with self.assertRaisesRegex(ValueError, 'service account'):
            verify_user_probe(dict(report, service_account_absent=False), docker=False, processes=128)
        with self.assertRaisesRegex(ValueError, 'process'):
            verify_user_probe(dict(report, process_limit_enforced=False), docker=False, processes=128)

    def test_guard_allows_only_same_sentry_to_transition_from_init_to_execution(self):
        import json
        import tempfile
        import uuid
        from pathlib import Path
        from unittest.mock import patch
        import node_guard
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cgroups = root / 'cgroups'
            sentry = cgroups / 'sentry'
            sentry.mkdir(parents=True)
            for name, value in {'cpu.max': '200000 100000', 'memory.max': str(2 << 30),
                                'pids.max': '512', 'memory.swap.max': '0'}.items():
                (sentry / name).write_text(value)
            area, pod = str(uuid.uuid4()), str(uuid.uuid4())
            request = {'protocol': 'aidash-runner/2', 'action': 'v2_limits', 'area_id': area, 'pod_uid': pod,
                       'pod_name': 'scratch-' + 'a' * 48, 'container_name': 'execution',
                       'container_id': 'b' * 64, 'epoch': 3, 'image': 'pinned', 'cpu': 2,
                       'memory_bytes': 2 << 30, 'host_tasks': 512, 'capacity_bytes': 100 << 30}
            guard = root / 'guard'
            guard.mkdir()
            previous = dict(request, container_id='c' * 64, container_name='helpers', sentry_pid=42)
            (guard / (area + '.json')).write_text(json.dumps(previous))
            with patch.object(node_guard, 'ROOT', guard), patch.object(node_guard, 'CGROUP_ROOT', cgroups), \
                 patch.object(node_guard, 'sentry_cgroup', return_value=sentry), \
                 patch.object(node_guard, 'validate', return_value=('b' * 64, pod, area, {'pid': 42})), \
                 patch.object(node_guard, 'gone', return_value=False), \
                 patch.object(node_guard, 'binding', return_value={}), \
                 patch.object(node_guard, 'scratch_capacity', return_value=99 << 30):
                self.assertEqual(node_guard.handle(request)['host_tasks'], 512)
                (guard / (area + '.json')).write_text(json.dumps(dict(previous, sentry_pid=99)))
                with self.assertRaisesRegex(ValueError, 'stale'):
                    node_guard.handle(request)


class TrustedProbeRegressionTest(unittest.TestCase):
    def test_probe_cannot_be_traced_by_live_user_processes(self):
        from unittest.mock import MagicMock, patch
        import sandbox_v2
        libc = MagicMock()
        libc.prctl.return_value = 0
        with patch.object(sandbox_v2, 'enter_user_root'), patch('ctypes.CDLL', return_value=libc), \
                patch('user_probe.probe', return_value={'evidence': True}):
            self.assertEqual(sandbox_v2.run_admission_probe('/rootfs', False, 'user', None), {'evidence': True})
        libc.prctl.assert_called_once_with(sandbox_v2._PR_SET_DUMPABLE, 0, 0, 0, 0)

    def test_rejects_symlinked_system_root(self):
        import tempfile
        from pathlib import Path
        from sandbox_v2 import validate_system_root
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / 'image-root'
            image.mkdir()
            (image / 'rootfs').symlink_to('/')
            with self.assertRaises(ValueError):
                validate_system_root(str(image / 'rootfs'))

    def test_probe_runs_after_chroot_without_executing_untrusted_loader(self):
        from unittest.mock import MagicMock, patch
        from sandbox_v2 import run_admission_probe
        with patch('ctypes.CDLL', return_value=MagicMock(**{'prctl.return_value': 0})), patch('sandbox_v2.enter_user_root') as enter, \
             patch('user_probe.probe', return_value={'evidence': True}), \
             patch('os.execvp', side_effect=AssertionError('untrusted exec')):
            self.assertEqual(run_admission_probe('/rootfs', False, 'user', None), {'evidence': True})
            enter.assert_called_once_with('/rootfs', '.', False)


class ExecProcessLimitTest(unittest.TestCase):
    def test_chroot_exec_applies_limit_to_new_process(self):
        import os
        import subprocess
        import sys
        code = r"""
import os, resource
from unittest.mock import patch
from sandbox_v2 import chroot_command
os.environ['AIDASH_PROCESS_LIMIT'] = '128'
def execute(*args):
    print(*resource.getrlimit(resource.RLIMIT_NPROC))
with patch('os.chroot'), patch('os.chdir'), patch('sandbox_v2.validate_system_root'), patch('sandbox_v2.image_environment', return_value={}), patch('sandbox_v2.drop_user_capabilities'), patch('os.execvpe', execute):
    chroot_command('/rootfs', '.', ['/bin/true'], False)
"""
        result = subprocess.run([sys.executable, '-c', code], cwd=os.path.dirname(__file__),
                                capture_output=True, text=True, check=True)
        self.assertEqual(result.stdout.strip(), '128 128')


class AdmissionGateOrderTest(unittest.TestCase):
    def test_guard_checks_cgroup_before_releasing_user_image(self):
        import json
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        events = []
        pod = {'metadata': {'uid': 'pod-uid', 'annotations': {'aidash.run/epoch': '1'}},
               'spec': {'nodeName': 'node-1'}, 'status': {'initContainerStatuses': [
                   {'name': 'helpers', 'containerID': 'containerd://' + 'a' * 64, 'state': {'running': {}}}]}}
        class Backend:
            def kube(self, args, data=None, **_kwargs):
                if args[0] == 'get':
                    if args[1] == 'pvc':
                        value = {'metadata': {'uid': 'claim'}, 'spec': {'volumeName': 'pv'}}
                    elif args[1] == 'pv':
                        value = {'metadata': {'uid': 'volume', 'finalizers': ['external-provisioner.volume.kubernetes.io/finalizer']},
                                 'spec': {'claimRef': {'uid': 'claim'}, 'persistentVolumeReclaimPolicy': 'Delete'}}
                    else:
                        value = pod
                    return SimpleNamespace(stdout=json.dumps(value).encode(), returncode=0)
                if '.guard-waiting' in args[-1]:
                    events.append('waiting')
                    return SimpleNamespace(stdout=b'', returncode=0)
                events.append('release')
                return SimpleNamespace(stdout=b'', returncode=0)
            def guard_execution_pod(self, *_args):
                events.append('guard')
        controller = ExecutionPodController(Backend(), {'processes': 128}, admitted=True)
        controller._record_guard = Mock()
        controller._record_volume = Mock()
        controller._admit_pod('scratch-' + 'a' * 48, {'scratch_space_id': 'scratch'})
        self.assertEqual(events, ['waiting', 'guard', 'release'])

    def test_network_evidence_requires_gateway_and_denies_all_other_targets(self):
        from execution_pod import verify_network_probe
        report = {'gateway': True, 'peer': False, 'node': False, 'metadata': False,
                  'internet': False, 'raw_sent': False}
        verify_network_probe(report, docker=False, spoof_seen=False)
        for field in ('peer', 'node', 'metadata', 'internet'):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'network'):
                verify_network_probe(dict(report, **{field: True}), docker=False, spoof_seen=False)
        with self.assertRaisesRegex(ValueError, 'gateway'):
            verify_network_probe(dict(report, gateway=False), docker=False, spoof_seen=False)
        with self.assertRaisesRegex(ValueError, 'spoof'):
            verify_network_probe(dict(report, raw_sent=True), docker=True, spoof_seen=True)

    def test_spoof_probe_contains_requested_source_and_nonce(self):
        import socket
        import struct
        from network_probe import spoof_packet
        packet = spoof_packet('192.0.2.1', '198.51.100.2', 18081, b'nonce')
        self.assertEqual(socket.inet_ntoa(packet[12:16]), '192.0.2.1')
        self.assertEqual(socket.inet_ntoa(packet[16:20]), '198.51.100.2')
        self.assertEqual(struct.unpack('!H', packet[22:24])[0], 18081)
        self.assertTrue(packet.endswith(b'nonce'))


class V2NetworkPolicyTest(unittest.TestCase):
    def test_only_configured_gateway_is_permitted(self):
        from execution_pod import v2_network_policy
        config = {'namespace': 'sandbox', 'proxy_endpoint': 'http://10.0.0.8:8888',
                  'proxy_namespace': 'gateway', 'proxy_selector': {'app': 'proxy'}}
        policy = v2_network_policy(config)
        self.assertEqual(policy['spec']['podSelector']['matchLabels']['aidash-v2'], 'true')
        self.assertEqual(policy['spec']['ingress'], [])
        self.assertEqual(policy['spec']['egress'], [{
            'to': [{'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'gateway'}},
                    'podSelector': {'matchLabels': {'app': 'proxy'}}}],
            'ports': [{'protocol': 'TCP', 'port': 8888}],
        }])
        with self.assertRaises(ValueError):
            v2_network_policy(dict(config, proxy_endpoint='http://169.254.169.254:80'))

    def test_negative_probe_requires_reachable_positive_controls(self):
        from execution_pod import verify_positive_controls
        controls = {'peer': True, 'node': True, 'metadata': True, 'internet': True, 'gateway': True}
        verify_positive_controls(controls)
        for name in controls:
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'positive control'):
                verify_positive_controls(dict(controls, **{name: False}))

    def test_guard_network_control_distinguishes_spoof_from_positive_udp(self):
        import json
        import tempfile
        import uuid
        from pathlib import Path
        from unittest.mock import patch
        import node_guard
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            area, pod = str(uuid.uuid4()), str(uuid.uuid4())
            request = {'protocol': 'aidash-runner/2', 'action': 'v2_network_seen', 'area_id': area,
                       'pod_uid': pod, 'pod_name': 'scratch-' + 'a' * 48, 'container_name': 'execution',
                       'container_id': 'b' * 64, 'epoch': 1, 'image': 'pinned'}
            previous = dict(request, sentry_pid=42, network_listener={'nonce': 'c' * 32, 'pid': 50})
            (root / (area + '.json')).write_text(json.dumps(previous))
            with patch.object(node_guard, 'ROOT', root), patch.object(node_guard, 'gone', return_value=False), \
                 patch.object(node_guard, 'validate', return_value=('b' * 64, pod, area, {'pid': 42})):
                (root / ('network-' + 'c' * 32 + '.seen.control')).touch()
                self.assertEqual(node_guard.handle(request), {'seen': False, 'control_seen': True})
                (root / ('network-' + 'c' * 32 + '.seen')).touch()
                self.assertEqual(node_guard.handle(request), {'seen': True, 'control_seen': True})

    def test_additive_network_policy_cannot_open_another_egress_path(self):
        from execution_pod import verify_network_policies, v2_network_policy
        config = {'namespace': 'sandbox', 'proxy_endpoint': 'http://10.0.0.8:8888',
                  'proxy_namespace': 'gateway', 'proxy_selector': {'app': 'proxy'}}
        expected = v2_network_policy(config)
        labels = {'aidash-sandbox': 'true', 'aidash-v2': 'true'}
        verify_network_policies([expected], labels, expected)
        permissive = {'metadata': {'name': 'other'}, 'spec': {'podSelector': {},
                      'policyTypes': ['Egress'], 'egress': [{}]}}
        with self.assertRaisesRegex(ValueError, 'network policy'):
            verify_network_policies([expected, permissive], labels, expected)


class GuardV2TerminationTest(unittest.TestCase):
    def test_init_container_can_retire_exact_sentry_after_container_disappears(self):
        import json
        import tempfile
        import uuid
        from pathlib import Path
        from unittest.mock import patch
        import node_guard
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            area, pod = str(uuid.uuid4()), str(uuid.uuid4())
            request = {'protocol': 'aidash-runner/2', 'action': 'terminate', 'area_id': area,
                       'pod_uid': pod, 'pod_name': 'scratch-' + 'a' * 48, 'container_name': 'helpers',
                       'container_id': 'b' * 64, 'epoch': 1, 'image': 'pinned'}
            previous = dict(request, sentry_pid=42, sentry_start='123', terminated=False)
            (root / (area + '.json')).write_text(json.dumps(previous))
            with patch.object(node_guard, 'ROOT', root), patch.object(node_guard, 'gone', return_value=False), \
                 patch.object(node_guard, 'kill_sandbox') as kill, \
                 patch.object(node_guard, 'validate', side_effect=AssertionError('container already gone')):
                self.assertTrue(node_guard.handle(request)['termination_confirmed'])
                kill.assert_called_once_with({'pid': 42}, expected_start='123')

    def test_unconfirmed_node_termination_blocks_reuse_and_purge(self):
        import json
        import tempfile
        from pathlib import Path
        from execution_pod import ExecutionPodController, ExecutionPodError
        from control import scratch_resource_name
        class Backend:
            def __init__(self, root):
                self.root = Path(root)
            def terminate_execution_pod(self, _guard):
                raise RuntimeError('node unreachable')
        with tempfile.TemporaryDirectory() as directory:
            backend = Backend(directory)
            controller = ExecutionPodController(backend, {}, admitted=True)
            name = scratch_resource_name('scratch')
            spaces = backend.root / 'execution-v2-spaces'
            spaces.mkdir()
            path = spaces / (name + '.json')
            path.write_text(json.dumps({'scratch_space_id': 'scratch', 'last_command_at': 1,
                                       'guard': {'pod_uid': 'uid', 'terminated': False}}))
            with self.assertRaisesRegex(ExecutionPodError, 'node unreachable'):
                controller._terminate_recorded_pod('scratch', expected_uid='uid')
            self.assertTrue(json.loads(path.read_text())['termination_unconfirmed'])
            with self.assertRaisesRegex(ExecutionPodError, 'not been confirmed'):
                controller._require_space_termination_confirmed('scratch')


class StartupAdmissionTest(unittest.TestCase):
    def test_failed_self_test_leaves_v2_closed_and_deletes_probe(self):
        from types import SimpleNamespace
        from unittest.mock import Mock
        from control import Runner
        runner = object.__new__(Runner)
        runner.config = {'image': 'image@sha256:' + 'a' * 64, 'cpu': 2, 'memory_bytes': 2 << 30}
        runner.execution_pods = SimpleNamespace(config={'enabled': True, 'helper_image': 'helper@sha256:' + 'b' * 64},
            admitted=False, ensure=Mock(side_effect=RuntimeError('network probe failed')), delete=Mock())
        runner.activate_execution_pods()
        self.assertFalse(runner.execution_pods.admitted)
        self.assertIn('network probe failed', runner.execution_pods.admission_error)
        runner.execution_pods.delete.assert_called_once()

    def test_self_test_enables_only_after_success_and_cleanup(self):
        from types import SimpleNamespace
        from unittest.mock import Mock
        from control import Runner
        runner = object.__new__(Runner)
        runner.config = {'image': 'image@sha256:' + 'a' * 64, 'cpu': 2, 'memory_bytes': 2 << 30}
        runner.execution_pods = SimpleNamespace(config={'enabled': True, 'helper_image': 'helper@sha256:' + 'b' * 64},
            admitted=False, ensure=Mock(), delete=Mock())
        runner.activate_execution_pods()
        self.assertTrue(runner.execution_pods.admitted)
        runner.execution_pods.ensure.assert_called_once()
        runner.execution_pods.delete.assert_called_once()
        runner.execution_pods.delete.side_effect = RuntimeError('cleanup failed')
        runner.activate_execution_pods()
        self.assertFalse(runner.execution_pods.admitted)
        self.assertIn('cleanup failed', runner.execution_pods.admission_error)


class PurgeSafetyTest(unittest.TestCase):
    def test_retained_persistent_volume_is_never_reported_as_purged(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController, ExecutionPodError

        class Backend:
            def __init__(self, root):
                self.root = Path(root)
                self.calls = []
            def kube(self, args, data=None, **_kwargs):
                self.calls.append(args)
                if args[:2] == ['get', 'pvc']:
                    body = {'metadata': {'uid': 'claim-uid'}, 'spec': {'volumeName': 'volume-1'}}
                elif args[:2] == ['get', 'pv']:
                    body = {'metadata': {'uid': 'volume-uid'}, 'spec': {'persistentVolumeReclaimPolicy': 'Retain',
                            'claimRef': {'uid': 'claim-uid'}}}
                else:
                    raise AssertionError('retained data must not be deleted or reported as purged')
                return SimpleNamespace(stdout=json.dumps(body).encode())
        with tempfile.TemporaryDirectory() as directory:
            backend = Backend(directory)
            controller = ExecutionPodController(backend, {}, admitted=True)
            with self.assertRaises(ExecutionPodError) as error:
                controller.delete('scratch')
            self.assertEqual(error.exception.code, 'purge_unsupported')
            self.assertFalse(any(call[0] == 'delete' for call in backend.calls))

    def test_purge_waits_for_delete_policy_volume_to_disappear(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController
        class Backend:
            def __init__(self, root):
                self.root, self.deleted = Path(root), False
                self.pv_after_delete = 0
            def kube(self, args, data=None, **_kwargs):
                if args[0] == 'get':
                    if args[1] == 'pvc':
                        body = {'metadata': {'uid': 'claim-uid'}, 'spec': {'volumeName': 'volume-1'}}
                    elif args[1] == 'pv' and (not self.deleted or self.pv_after_delete < 2):
                        if self.deleted:
                            self.pv_after_delete += 1
                        body = {'metadata': {'uid': 'volume-uid', 'finalizers': ['external-provisioner.volume.kubernetes.io/finalizer']},
                                'spec': {'persistentVolumeReclaimPolicy': 'Delete', 'claimRef': {'uid': 'claim-uid'}}}
                    else:
                        return SimpleNamespace(stdout=b'')
                    return SimpleNamespace(stdout=json.dumps(body).encode())
                if args[:2] == ['delete', 'pvc']:
                    self.deleted = True
                return SimpleNamespace(stdout=b'')
        with tempfile.TemporaryDirectory() as directory:
            backend = Backend(directory)
            controller = ExecutionPodController(backend, {}, admitted=True)
            controller.touch_space('scratch')
            from unittest.mock import patch
            with patch('time.sleep'):
                self.assertEqual(controller.delete('scratch'), {'deleted': True})
            self.assertEqual(backend.pv_after_delete, 2)


class DockerAdmissionTest(unittest.TestCase):
    def test_guest_pid_limit_is_best_effort_while_required_caps_and_raw_are_checked(self):
        from execution_pod import verify_user_probe
        allowed = (1 << 12) | (1 << 13) | (1 << 21)
        report = {'uid': 0, 'kernel': '4.19.0-gvisor', 'no_new_privs': 1,
                  'caps': {'CapInh': 0, 'CapPrm': allowed, 'CapEff': allowed,
                           'CapBnd': allowed, 'CapAmb': 0},
                  'process_limit': [128, 128], 'process_limit_enforced': False,
                  'process_limit_unraiseable': True, 'service_account_absent': True,
                  'system_absent': True, 'user_namespace_blocked': True, 'proc_root_blocked': True,
                  'raw_socket': True, 'root_writable': True, 'helpers_read_only': True}
        verify_user_probe(report, docker=True, processes=128)
        with self.assertRaisesRegex(ValueError, 'raw'):
            verify_user_probe(dict(report, raw_socket=False), docker=True, processes=128)

class ReviewRegressionTest(unittest.TestCase):
    def test_background_does_not_consume_foreground_slots(self):
        import threading
        from execution_pod import CommandQueue
        queue, release = CommandQueue(), threading.Event()
        try:
            jobs = [queue.submit('r', i, lambda _: release.wait(5), background=True) for i in range(8)]
            self.assertEqual(queue.submit('r', 'foreground', lambda value: value).result(timeout=1), 'foreground')
        finally:
            release.set()
            for job in jobs:
                job.result(timeout=5)
            queue.close()

    def test_concurrent_intervals_are_ambiguous(self):
        import tempfile
        from pathlib import Path
        from execution_pod import change_interval, read_change_log
        with tempfile.TemporaryDirectory() as d:
            root, state = Path(d) / 'root', Path(d) / 'state'
            root.mkdir()
            change_interval(root, state, 'one', 'run-one')
            (root / 'first').write_text('a')
            change_interval(root, state, 'two', 'run-two')
            (root / 'second').write_text('b')
            change_interval(root, state, 'two', 'run-two', finished=True)
            records = read_change_log(state / 'changes.jsonl')
            self.assertEqual(records[0]['run_id'], 'run-one')
            self.assertEqual(records[1]['attribution'], 'ambiguous')
            self.assertEqual(records[1]['run_ids'], ['run-one', 'run-two'])
            self.assertIn('second', records[1]['changes']['added'])
            change_interval(root, state, 'three', 'run-one')
            (root / 'third').write_text('c')
            change_interval(root, state, 'three', 'run-one', finished=True)
            records = read_change_log(state / 'changes.jsonl')
            self.assertEqual(records[-1]['attribution'], 'ambiguous')
            self.assertEqual(records[-1]['run_ids'], ['run-one'])

    def test_nul_is_rejected_before_kubernetes_and_journal(self):
        import tempfile
        from pathlib import Path
        from execution_pod import ExecutionPodController
        class Backend:
            def __init__(self, root): self.root = Path(root)
            def kube(self, *a, **kw): raise AssertionError('must not reach Kubernetes')
        with tempfile.TemporaryDirectory() as d:
            controller = ExecutionPodController(Backend(d), {}, admitted=True)
            for field, value in [('code', 'a\x00b'), ('run_id', 'a\x00b'), ('cwd', 'a\x00b'), ('env', {'X':'a\x00b'})]:
                request = {'operation_id':'e6ded09d-f783-4ce8-98e9-dbfca68884f1','scratch_space_id':'s','run_id':'r','code':'true'}
                request[field] = value
                with self.subTest(field=field), self.assertRaises(ValueError):
                    controller.submit_command(request)
            self.assertFalse(list((Path(d) / 'execution-v2').glob('*.json')))

    def test_init_only_mounts_system_subpath(self):
        from execution_pod import execution_manifests
        settings = {'scratch_space_id':'s','image':'image@sha256:'+'a'*64,'cpu':2,'memory_bytes':2<<30,'capacity_bytes':2<<30,'docker_enabled':False}
        config = {'runtime_class':'aidash-gvisor','helper_image':'helper@sha256:'+'b'*64,'namespace':'sandbox'}
        _, pod = execution_manifests(settings, config)
        image = next(item for item in pod['spec']['initContainers'] if item['name'] == 'image-root')
        self.assertEqual(next(m for m in image['volumeMounts'] if m['name'] == 'scratch')['subPath'], 'image-root')
        blanked = {item['name'] for item in image['env'] if item['value'] == ''}
        self.assertLessEqual({'LD_PRELOAD', 'LD_AUDIT', 'LD_LIBRARY_PATH', 'PYTHONPATH', 'PYTHONHOME'}, blanked)
        self.assertIn('-I', image['command'])
        env = {item['name']:item['value'] for item in image['env']}
        for name in ('LD_PRELOAD','LD_LIBRARY_PATH','PYTHONPATH','PYTHONHOME'):
            self.assertEqual(env[name], '')

    def test_image_subpath_cannot_point_at_bookkeeping(self):
        import tempfile
        from pathlib import Path
        from prepare_helpers import prepare_image_subpath
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'operations').mkdir()
            (root / 'image-root').symlink_to('operations')
            with self.assertRaisesRegex(ValueError, 'symlink'):
                prepare_image_subpath(root / 'image-root')

    def test_old_terminated_guard_does_not_quarantine_new_pod(self):
        import json
        import tempfile
        from pathlib import Path
        from control import scratch_resource_name
        from execution_pod import ExecutionPodController
        class Backend:
            def __init__(self, root): self.root = Path(root)
        with tempfile.TemporaryDirectory() as d:
            controller = ExecutionPodController(Backend(d), {}, admitted=True)
            path = Path(d) / 'execution-v2-spaces' / (scratch_resource_name('space') + '.json')
            path.parent.mkdir()
            path.write_text(json.dumps({'terminated_pods':['old'], 'guard':{'pod_uid':'new','terminated':False}}))
            controller._terminate_recorded_pod('space', expected_uid='old')
            self.assertFalse(json.loads(path.read_text()).get('termination_unconfirmed', False))

    def test_stopping_marks_running_unknown_and_late_results_cannot_replace_it(self):
        import json
        import tempfile
        from pathlib import Path
        from execution_pod import ExecutionPodController
        class Backend:
            def __init__(self, root): self.root = Path(root)
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        with tempfile.TemporaryDirectory() as d:
            controller = ExecutionPodController(Backend(d), {}, admitted=True)
            path = controller._command_path(operation)
            path.write_text(json.dumps({'operation_id': operation, 'scratch_space_id': 's', 'status': 'running'}))
            with controller.lock:
                controller._interrupt_space_commands('s')
            controller._update_command(operation, status='completed', exit_code=0)
            result = json.loads(path.read_text())
            self.assertEqual((result['status'], result['outcome']), ('interrupted', 'unknown'))
            self.assertNotIn('exit_code', result)

    def test_docker_result_never_reads_pvc_outcome(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import patch
        from execution_pod import ExecutionPodController
        operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
        class Backend:
            def __init__(self, root): self.root, self.command, self.environment = Path(root), ['kubectl'], {}
            def kube(self, *args, **kwargs): raise AssertionError('Docker result must not read PVC')
        with tempfile.TemporaryDirectory() as d:
            controller = ExecutionPodController(Backend(d), {}, admitted=True)
            controller._command_path(operation).write_text(json.dumps({'operation_id':operation,'scratch_space_id':'s','run_id':'r','pod':'pod','pod_uid':'uid','docker_enabled':True,'background':False,'timeout_seconds':5,'cwd':'.','code':'true','status':'queued'}))
            transport = {'exit_code':0,'transport_timed_out':False,'framed_result':{'operation_id': operation, 'run_id': 'r', 'status': 'completed', 'exit_code': 127}, 'output':{'stdout':'b2s=','stderr':'','truncated':False}}
            with patch('execution_pod.drain_process', return_value=transport):
                controller._execute_command(operation)
            result = controller.command_result(operation)
            self.assertEqual((result['status'], result['exit_code'], result['stdout']), ('completed', 127, 'b2s='))


class NormalDeleteProofTest(unittest.TestCase):
    """A normal delete that completed must stay provable after its reply is lost (#713 decision 5)."""

    def _fixture(self, directory):
        import json
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController, ExecutionPodError
        from control import scratch_resource_name
        world = {'pod': {'metadata': {'uid': 'uid'}, 'spec': {'nodeName': 'node'}}, 'node_uid': 'node-uid',
                 'pod_lookup_fails': False, 'node_lookup_fails': False, 'delete_raises': None,
                 'delete_applies': True, 'wait_fails': False, 'after_delete': {}}
        deletes = []
        def get(kind, name):
            if kind == 'node':
                if world['node_lookup_fails']:
                    raise TimeoutError('node lookup timed out')
                return {'metadata': {'name': name, 'uid': world['node_uid']}} if world['node_uid'] else None
            if world['pod_lookup_fails']:
                raise TimeoutError('pod lookup timed out')
            return world['pod']
        def kube(args, *rest, **kwargs):
            if args[0] == 'delete':
                deletes.append(json.loads(rest[0]))
                if world['delete_applies']:
                    world['pod'] = None
                world.update(world['after_delete'])
                if world['delete_raises'] is not None:
                    raise world['delete_raises']
            if args[0] == 'wait' and world['wait_fails']:
                raise TimeoutError('wait timed out')
            return SimpleNamespace(returncode=0, stdout=b'')
        controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube),
                                            {'namespace': 'fixture'}, admitted=True)
        controller.touch_space('s', now=1)
        path = Path(directory) / 'execution-v2-spaces' / (scratch_resource_name('s') + '.json')
        state = json.loads(path.read_text())
        state['guard'] = {'pod_uid': 'uid', 'cluster_node': 'node', 'node_uid': 'node-uid', 'terminated': False}
        path.write_text(json.dumps(state))
        controller._terminate_recorded_pod = Mock(side_effect=ExecutionPodError('termination_unconfirmed', 'guard unavailable'))
        controller._get = get
        return controller, world, deletes, path

    def test_completed_delete_whose_proof_was_lost_counts_as_stopped(self):
        import json
        import subprocess
        import tempfile
        from unittest.mock import patch
        from execution_pod import ExecutionPodError, STOP_RETRY_SECONDS
        # What goes wrong once the delete has been applied, or whose outcome the API server left open.
        losses = {
            'reply lost': {'delete_raises': TimeoutError('delete reply lost')},
            'kubectl timed out': {'delete_raises': subprocess.TimeoutExpired('kubectl', 70)},
            'connection dropped': {'delete_raises': RuntimeError('Unable to connect to the server: EOF')},
            'server timeout': {'delete_raises': RuntimeError(
                'Error from server (ServerTimeout): the server was unable to return a response in the time allotted')},
            'wait and pod lookup time out': {'wait_fails': True, 'pod_lookup_fails': True},
            'node lookup times out afterwards': {'node_lookup_fails': True},
        }
        for label, loss in losses.items():
            with self.subTest(label), tempfile.TemporaryDirectory() as directory:
                controller, world, deletes, path = self._fixture(directory)
                with patch('time.time', return_value=100):
                    with self.assertRaises(ExecutionPodError):
                        controller.stop('s')
                world['after_delete'] = loss
                with patch('time.time', return_value=100 + STOP_RETRY_SECONDS):
                    with self.assertRaises((ExecutionPodError, RuntimeError, OSError, subprocess.TimeoutExpired)):
                        controller.stop('s')
                self.assertEqual(len(deletes), 1)
                world.update(delete_raises=None, pod_lookup_fails=False, node_lookup_fails=False, wait_fails=False)
                with patch('time.time', return_value=100 + STOP_RETRY_SECONDS + 1):
                    self.assertTrue(controller.stop('s')['stopped'])
                self.assertEqual(len(deletes), 1)
                state = json.loads(path.read_text())
                self.assertFalse(state['termination_unconfirmed'])
                self.assertNotIn('stop_requested_at', state)
                self.assertEqual(state['terminated_pods'], ['uid'])
                self.assertTrue(state['guard']['terminated'])

    def test_lost_proof_is_not_accepted_once_the_node_left_the_cluster(self):
        import tempfile
        from unittest.mock import patch
        from execution_pod import ExecutionPodError, STOP_RETRY_SECONDS
        for node_uid in (None, 'replacement-node'):
            with self.subTest(node_uid=node_uid), tempfile.TemporaryDirectory() as directory:
                controller, world, deletes, path = self._fixture(directory)
                with patch('time.time', return_value=100):
                    with self.assertRaises(ExecutionPodError):
                        controller.stop('s')
                world.update(delete_raises=TimeoutError('delete reply lost'))
                with patch('time.time', return_value=100 + STOP_RETRY_SECONDS):
                    with self.assertRaises((ExecutionPodError, RuntimeError, OSError)):
                        controller.stop('s')
                world.update(delete_raises=None, node_uid=node_uid)
                with patch('time.time', return_value=100 + STOP_RETRY_SECONDS + 1):
                    with self.assertRaises(ExecutionPodError) as error:
                        controller.stop('s')
                self.assertEqual(error.exception.code, 'termination_unconfirmed')
                self.assertEqual(len(deletes), 1)

    def test_a_missing_pod_is_no_proof_when_no_delete_was_issued_for_it(self):
        import tempfile
        from unittest.mock import patch
        from execution_pod import ExecutionPodError, STOP_RETRY_SECONDS
        with tempfile.TemporaryDirectory() as directory:
            controller, world, deletes, path = self._fixture(directory)
            with patch('time.time', return_value=100):
                with self.assertRaises(ExecutionPodError):
                    controller.stop('s')
            world['pod'] = None
            with patch('time.time', return_value=100 + STOP_RETRY_SECONDS):
                with self.assertRaises(ExecutionPodError):
                    controller.stop('s')
            self.assertEqual(deletes, [])


    def test_a_refused_delete_leaves_no_proof_for_a_pod_that_disappears_later(self):
        import tempfile
        from unittest.mock import patch
        from execution_pod import ExecutionPodError, STOP_RETRY_SECONDS
        refusals = (
            'Error from server (Forbidden): pods "x" is forbidden: User cannot delete resource "pods"',
            'Error from server (Conflict): Precondition failed: UID in precondition: uid, UID in object meta: other',
            'Error from server (Invalid): Pod "x" is invalid',
            'Error from server (Unauthorized): Unauthorized',
            'Error from server (NotFound): pods "x" not found',
        )
        for message in refusals:
            with self.subTest(message), tempfile.TemporaryDirectory() as directory:
                controller, world, deletes, path = self._fixture(directory)
                with patch('time.time', return_value=100):
                    with self.assertRaises(ExecutionPodError):
                        controller.stop('s')
                world.update(delete_raises=RuntimeError(message), delete_applies=False)
                with patch('time.time', return_value=100 + STOP_RETRY_SECONDS):
                    with self.assertRaises(RuntimeError):
                        controller.stop('s')
                self.assertEqual(len(deletes), 1)
                # The Pod then vanishes some other way while its node is still registered.
                world.update(delete_raises=None, pod=None)
                with patch('time.time', return_value=100 + STOP_RETRY_SECONDS + 1):
                    with self.assertRaises(ExecutionPodError) as error:
                        controller.stop('s')
                self.assertEqual(error.exception.code, 'termination_unconfirmed')
                self.assertEqual(len(deletes), 1)

    def test_a_refused_retry_does_not_erase_an_earlier_delete_of_unknown_outcome(self):
        import tempfile
        from unittest.mock import patch
        from execution_pod import ExecutionPodError, STOP_RETRY_SECONDS
        with tempfile.TemporaryDirectory() as directory:
            controller, world, deletes, path = self._fixture(directory)
            with patch('time.time', return_value=100):
                with self.assertRaises(ExecutionPodError):
                    controller.stop('s')
            # The first delete may have been applied: its Pod is still terminating and its reply was lost.
            world.update(delete_raises=TimeoutError('delete reply lost'), delete_applies=False)
            with patch('time.time', return_value=100 + STOP_RETRY_SECONDS):
                with self.assertRaises(TimeoutError):
                    controller.stop('s')
            world.update(delete_raises=RuntimeError('Error from server (Forbidden): forbidden'))
            with patch('time.time', return_value=100 + STOP_RETRY_SECONDS + 1):
                with self.assertRaises(RuntimeError):
                    controller.stop('s')
            world.update(delete_raises=None, pod=None)
            with patch('time.time', return_value=100 + STOP_RETRY_SECONDS + 2):
                self.assertTrue(controller.stop('s')['stopped'])
            self.assertEqual(len(deletes), 2)


class PythonInterpreterStopTest(unittest.TestCase):
    """A cell's interpreter stop ends on the Pod's own result, never on a slow watcher or a guess."""
    cell = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
    session = 'e6ded09d-f783-4ce8-98e9-dbfca68884f2'

    def _run(self, *, session_result, stop_fails=False, result_after=0, during=None, after=None):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        clock = [0.0]
        calls = []
        reads = [0]
        outcome = SimpleNamespace(clock=clock, calls=calls)
        def kube(args, *rest, **kwargs):
            calls.append(args)
            if 'stop' in args and stop_fails:
                raise RuntimeError('exec failed')
            if 'output' in args:
                return SimpleNamespace(returncode=1, stdout=b'')
            if 'result' in args:
                reads[0] += 1
                if session_result is None or clock[0] < result_after:
                    if during:
                        during(outcome.controller, reads[0])
                    return SimpleNamespace(returncode=1, stdout=b'')
                return SimpleNamespace(returncode=0, stdout=json.dumps(dict(
                    session_result, operation_id=self.session, run_id='r')).encode())
            return SimpleNamespace(returncode=0, stdout=b'')
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=True)
            outcome.controller = controller
            controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
            controller._stop_matching_pod = Mock()
            controller.submit_command = Mock(return_value={'status': 'queued'})
            cell = {'operation_id': self.cell, 'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod',
                    'pod_uid': 'uid', 'docker_enabled': False, 'background': False, 'status': 'running',
                    'kind': 'python_cell', 'session_operation_id': self.session}
            controller._command_path(self.cell).write_text(json.dumps(cell))
            controller._python_session_path('s', 'r').write_text(json.dumps({'operation_id': self.session}))
            # The session stays `running` in the journal: after a Runner restart its watcher polls only every 5 s
            # and then reads saved output, which fails here.
            controller._command_path(self.session).write_text(json.dumps({'operation_id': self.session,
                'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'docker_enabled': False,
                'background': True, 'status': 'running', 'kind': 'python_session'}))
            def tick(seconds):
                clock[0] += seconds
            with patch('time.monotonic', side_effect=lambda: clock[0]), patch('time.sleep', side_effect=tick):
                controller._finish_command(cell, {'status': 'timed_out'},
                                           {'stdout': '', 'stderr': '', 'truncated': False})
                if after:
                    after(controller)
            outcome.cell_status = controller._read_command(self.cell)['status']
            return outcome

    def test_stopped_interpreter_is_read_from_its_result_not_from_the_watcher(self):
        run = self._run(session_result={'status': 'stopped'})
        run.controller._stop_matching_pod.assert_not_called()
        self.assertEqual(run.cell_status, 'timed_out')
        self.assertTrue(any('stop' in args for args in run.calls))

    def test_supervisor_still_draining_after_the_old_ten_second_wait_keeps_the_pod(self):
        run = self._run(session_result={'status': 'stopped'}, result_after=35)
        run.controller._stop_matching_pod.assert_not_called()
        self.assertEqual(run.cell_status, 'timed_out')
        self.assertGreaterEqual(run.clock[0], 35)

    def test_accepted_stop_without_a_terminal_result_deletes_the_pod_at_the_limit(self):
        from execution_pod import PYTHON_STOP_SECONDS
        run = self._run(session_result=None)
        run.controller._stop_matching_pod.assert_called_once_with('s', 'uid')
        self.assertGreaterEqual(run.clock[0], PYTHON_STOP_SECONDS)

    def test_the_limit_covers_what_the_supervisor_does_after_a_stop(self):
        import sandbox_v2
        from execution_pod import PYTHON_STOP_SECONDS
        # run_supervised: up to 5 s to kill and reap, FORWARD_GRACE_SECONDS for the readers, up to 30 s
        # more to drain them, and 0.5 s per reader to give up.
        self.assertGreater(PYTHON_STOP_SECONDS, 5 + sandbox_v2.FORWARD_GRACE_SECONDS + 30 + 2 * .5)

    def test_supervisor_reporting_unconfirmed_termination_deletes_the_pod(self):
        run = self._run(session_result={'status': 'termination_unconfirmed'})
        run.controller._stop_matching_pod.assert_called_once_with('s', 'uid')

    def test_stop_request_that_cannot_be_delivered_deletes_the_pod(self):
        run = self._run(session_result=None, stop_fails=True)
        run.controller._stop_matching_pod.assert_called_once_with('s', 'uid')

    def test_session_being_stopped_is_not_handed_to_a_new_cell(self):
        from execution_pod import ExecutionPodError
        attempts = []
        def during(controller, read):
            if read == 1:
                try:
                    attempts.append(controller.start_python('s', 'r'))
                except ExecutionPodError as error:
                    attempts.append(error.code)
        def after(controller):
            # The stop is confirmed by the Pod while the session watcher still has the journal at `running`.
            self.assertEqual(controller._read_command(self.session)['status'], 'running')
            attempts.append(controller.start_python('s', 'r'))
        run = self._run(session_result={'status': 'stopped'}, result_after=3, during=during, after=after)
        run.controller._stop_matching_pod.assert_not_called()
        self.assertEqual(attempts[0], 'session_interrupted')
        self.assertEqual(run.controller.submit_command.call_count, 1)
        self.assertNotEqual(attempts[1]['operation_id'], self.session)
        self.assertFalse(attempts[1].get('session_replaced', False))


class SavedOutputBudgetTest(unittest.TestCase):
    op = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'

    def test_the_retry_budget_restarts_after_every_chunk_that_was_read(self):
        import base64
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch
        from execution_pod import ExecutionPodController, OUTPUT_RETRY_SECONDS
        clock = [0.0]
        chunk, chunks = 10, 40
        def kube(args, *rest, **kwargs):
            # Every exec takes 2 s and fails when it is given less time than that.
            clock[0] += 2
            if kwargs['timeout'] < 2:
                raise TimeoutError('exec timed out')
            offset = int(args[-2])
            data = b'x' * chunk
            return SimpleNamespace(returncode=0, stdout=json.dumps({'data': base64.b64encode(data).decode(),
                'next_offset': offset + chunk, 'eof': offset + chunk >= chunk * chunks}).encode())
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            outcome = {}
            with patch('time.monotonic', side_effect=lambda: clock[0]), patch('time.sleep'):
                result = controller._saved_output({'operation_id': self.op, 'pod': 'pod'}, outcome, tolerate_missing=True)
            # 80 reads at 2 s take far longer than the whole retry budget.
            self.assertGreater(clock[0], OUTPUT_RETRY_SECONDS)
            self.assertNotIn('output_incomplete', outcome)
            self.assertFalse(result['truncated'])
            self.assertEqual(len(base64.b64decode(result['stdout'])), chunk * chunks)
            self.assertEqual(len(base64.b64decode(result['stderr'])), chunk * chunks)

    def test_unusable_saved_output_is_read_once_without_retrying(self):
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import patch
        from execution_pod import ExecutionPodController
        clock, attempts = [0.0], []
        def kube(args, *rest, **kwargs):
            attempts.append(args[-3])
            raise TimeoutError('offline')
        def advance(seconds):
            clock[0] += seconds
        with tempfile.TemporaryDirectory() as directory:
            controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
            outcome = {'output_incomplete': True}
            with patch('time.monotonic', side_effect=lambda: clock[0]), patch('time.sleep', side_effect=advance):
                result = controller._saved_output({'operation_id': self.op, 'pod': 'pod', 'stdout': 'cGFydA=='},
                                                  outcome, tolerate_missing=True)
            self.assertEqual(attempts, ['stdout', 'stderr'])
            self.assertEqual(clock[0], 0)
            self.assertEqual(result['stdout'], 'cGFydA==')
            self.assertTrue(result['truncated'])


class DockerLateStartFenceTest(unittest.TestCase):
    op = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'

    def _clock(self):
        from unittest.mock import patch
        from contextlib import ExitStack
        self.now = [0.0]
        stack = ExitStack()
        stack.enter_context(patch('time.monotonic', side_effect=lambda: self.now[0]))
        stack.enter_context(patch('time.sleep', side_effect=lambda seconds: self.now.__setitem__(0, self.now[0] + seconds)))
        return stack

    def _fixture(self, directory, *, running, started):
        import json
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        from sandbox_v2 import claim_operation
        claims = Path(directory) / 'claims'
        if started:
            claim_operation(self.op, claims)
        calls = []
        polls = []
        def kube(args, *rest, **kwargs):
            calls.append(args)
            if 'claim' in args:
                return SimpleNamespace(returncode=0, stdout=json.dumps({'created': claim_operation(self.op, claims)}).encode())
            if 'running' in args:
                polls.append(1)
                state = running(len(polls)) if callable(running) else running
                return SimpleNamespace(returncode=0, stdout=json.dumps({'running': state}).encode())
            return SimpleNamespace(returncode=0, stdout=b'')
        controller = ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {}, admitted=False)
        controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
        controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op, 'scratch_space_id': 's',
            'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'background': False, 'docker_enabled': True,
            'status': 'interrupted', 'outcome': 'unknown', 'terminal_at': 1}))
        return controller, calls, claims

    def test_a_supervisor_that_has_not_published_its_marker_is_fenced_out(self):
        import tempfile
        from sandbox_v2 import claim_operation
        with tempfile.TemporaryDirectory() as directory:
            controller, calls, claims = self._fixture(directory, running=False, started=False)
            result = controller._stop_command(self.op)
            self.assertEqual((result['status'], result['outcome']), ('interrupted', 'unknown'))
            self.assertFalse(any('stop' in args for args in calls))
            self.assertFalse(claim_operation(self.op, claims), 'a late supervisor could still start the command')

    def test_a_started_supervisor_that_stays_absent_for_the_whole_window_has_finished(self):
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            controller, calls, _ = self._fixture(directory, running=False, started=True)
            with self._clock():
                self.assertEqual(controller._stop_command(self.op)['status'], 'interrupted')
            self.assertGreaterEqual(self.now[0], 10)
            self.assertFalse(any('stop' in args for args in calls))
            # Result files of a Docker command are never evidence that it finished.
            self.assertFalse(any('result' in args for args in calls))

    def test_a_started_supervisor_that_publishes_its_marker_late_is_still_stopped(self):
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            controller, calls, _ = self._fixture(directory, running=lambda poll: poll >= 4, started=True)
            with self._clock():
                result = controller._stop_command(self.op)
            self.assertEqual(result['status'], 'stopping')
            self.assertLess(self.now[0], 10)
            self.assertTrue(any('stop' in args for args in calls))

    def test_a_running_supervisor_is_still_stopped(self):
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            controller, calls, _ = self._fixture(directory, running=True, started=True)
            self.assertEqual(controller._stop_command(self.op)['status'], 'stopping')
            self.assertTrue(any('stop' in args for args in calls))

    def test_an_unreadable_claim_is_not_taken_for_a_finished_command(self):
        import tempfile
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as directory:
            controller, calls, _ = self._fixture(directory, running=False, started=False)
            controller.backend.kube = lambda args, *rest, **kwargs: SimpleNamespace(
                returncode=1 if 'claim' in args else 0, stdout=b'{"running":false}')
            with self.assertRaisesRegex(RuntimeError, 'Could not fence'):
                controller._stop_command(self.op)


class DockerForwardedOutputTest(unittest.TestCase):
    op = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'

    def test_incomplete_forwarding_marks_docker_output_truncated_without_reading_saved_files(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from unittest.mock import Mock, patch
        from execution_pod import ExecutionPodController
        for forward_incomplete in (True, False):
            with self.subTest(forward_incomplete=forward_incomplete), tempfile.TemporaryDirectory() as directory:
                backend = SimpleNamespace(root=Path(directory), command=['kubectl'], environment={},
                                          kube=Mock(side_effect=AssertionError('Docker saved files are untrusted')))
                controller = ExecutionPodController(backend, {}, admitted=True)
                controller._command_path(self.op).write_text(json.dumps({'operation_id': self.op,
                    'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod', 'pod_uid': 'uid', 'docker_enabled': True,
                    'background': False, 'timeout_seconds': 5, 'cwd': '.', 'code': 'true', 'status': 'queued'}))
                transport = {'exit_code': 0, 'transport_timed_out': False,
                             'framed_result': {'operation_id': self.op, 'run_id': 'r', 'status': 'completed',
                                               'exit_code': 0, 'forward_incomplete': forward_incomplete},
                             'output': {'stdout': 'cGFydA==', 'stderr': '', 'truncated': False}}
                with patch('execution_pod.drain_process', return_value=transport):
                    controller._execute_command(self.op)
                result = controller.command_result(self.op)
                self.assertEqual((result['status'], result['stdout']), ('completed', 'cGFydA=='))
                self.assertEqual(result['truncated'], forward_incomplete)


class PythonSessionRefusalTest(unittest.TestCase):
    """A session is refused only while the Pod it ran in still exists (#722 decision 9)."""
    operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'

    def test_a_pod_that_no_longer_exists_does_not_block_a_new_session(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        for age in (0, 30 * 86400):
            with self.subTest(age=age), tempfile.TemporaryDirectory() as directory:
                controller = ExecutionPodController(Mock(root=Path(directory)), {}, admitted=True)
                controller._python_session_path('s', 'r').write_text(json.dumps({'operation_id': self.operation}))
                controller._command_path(self.operation).write_text(json.dumps({'operation_id': self.operation,
                    'status': 'interrupted', 'pod_uid': 'old', 'terminal_at': 1_000_000 - age}))
                controller._get = Mock(return_value=None)
                controller.submit_command = Mock(return_value={'status': 'queued'})
                self.assertTrue(controller.start_python('s', 'r')['session_replaced'])
