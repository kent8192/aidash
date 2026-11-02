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
        blocked = sum(1 << cap for cap in (12, 13, 21))
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
                  'namespace': 'aidash-sandbox', 'allow_docker': False}
        with self.assertRaises(ValueError):
            execution_manifests(settings, config)
        config['allow_docker'] = True
        _, pod = execution_manifests(settings, config)
        caps = pod['spec']['containers'][0]['securityContext']['capabilities']['add']
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

    def test_ensure_is_idempotent_and_stop_preserves_claim(self):
        import json
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController

        class Backend:
            def __init__(self):
                self.objects = {}
            def kube(self, args, data=None, **_kwargs):
                if args[0] == 'get':
                    obj = self.objects.get((args[1], args[2]))
                    return SimpleNamespace(stdout=json.dumps(obj).encode() if obj else b'')
                if args[0] == 'create':
                    obj = json.loads(data)
                    kind = 'pvc' if obj['kind'] == 'PersistentVolumeClaim' else 'pod'
                    self.objects[(kind, obj['metadata']['name'])] = obj
                elif args[0] == 'delete':
                    self.objects.pop((args[1], args[2]), None)
                return SimpleNamespace(stdout=b'')

        backend = Backend()
        config = {'runtime_class': 'aidash-gvisor', 'helper_image': 'helper@sha256:' + 'b' * 64,
                  'namespace': 'aidash-sandbox'}
        settings = {'scratch_space_id': 'id', 'image': 'image@sha256:' + 'a' * 64, 'cpu': 2,
                    'memory_bytes': 1 << 30, 'capacity_bytes': 1 << 30, 'docker_enabled': False}
        controller = ExecutionPodController(backend, config, admitted=True)
        resource = controller.ensure(settings)
        self.assertEqual(controller.ensure(settings), resource)
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

    def test_startup_failure_contains_typed_reason(self):
        import json
        from types import SimpleNamespace
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
        with self.assertRaises(ExecutionPodError) as caught:
            ExecutionPodController(Backend(), config, admitted=True).ensure(settings)
        self.assertEqual(caught.exception.code, 'startup_failed')
        self.assertIn('image pull failed', caught.exception.message)
