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

        controller = ExecutionPodController(object(), {}, admitted=True)
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
            controller.session_executor.shutdown(wait=True)

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
            def kube(self, args, data=None, **_kwargs):
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


class PodBindingTest(unittest.TestCase):
    def test_replacement_pod_rejects_command_before_execution(self):
        from unittest.mock import patch
        from sandbox_v2 import supervise_command
        with patch.dict('os.environ', {'AIDASH_POD_UID': 'replacement'}):
            with self.assertRaisesRegex(ValueError, 'Pod identity'):
                supervise_command('e6ded09d-f783-4ce8-98e9-dbfca68884f1', 'run', '.', 'true',
                                  5, False, expected_pod_uid='original')


class CommandJournalTest(unittest.TestCase):
    def test_recovery_marks_running_unknown_and_never_replays_it(self):
        import json
        import tempfile
        from pathlib import Path
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController
        from control import scratch_resource_name

        class Backend:
            def __init__(self, root):
                self.root = Path(root)
                self.calls = []
            def kube(self, args, data=None, **_kwargs):
                self.calls.append(args)
                if args[0] == 'get':
                    return SimpleNamespace(stdout=json.dumps({'metadata': {'uid': 'pod-uid'}}).encode())
                return SimpleNamespace(stdout=b'')
            def terminate_execution_pod(self, guard):
                return {'termination_confirmed': True}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'execution-v2'
            root.mkdir()
            operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            record = {'operation_id': operation, 'scratch_space_id': 'scratch', 'run_id': 'run',
                      'status': 'running', 'background': True, 'pod_uid': 'pod-uid'}
            (root / (operation + '.json')).write_text(json.dumps(record))
            spaces = Path(directory) / 'execution-v2-spaces'
            spaces.mkdir()
            name = scratch_resource_name('scratch')
            (spaces / (name + '.json')).write_text(json.dumps({'scratch_space_id': 'scratch',
                'last_command_at': 1, 'guard': {'pod_uid': 'pod-uid', 'terminated': False}}))
            backend = Backend(directory)
            controller = ExecutionPodController(backend, {}, admitted=False)
            controller.recover_commands()
            result = json.loads((root / (operation + '.json')).read_text())
            self.assertEqual(result['status'], 'interrupted')
            self.assertEqual(result['outcome'], 'unknown')
            self.assertEqual(len(backend.calls), 3)
            self.assertEqual(backend.calls[2][:2], ['delete', 'pod'])

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
            controller.session_executor.shutdown(wait=True)

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


class RecoveryRegressionTest(unittest.TestCase):
    def test_recovery_retires_commands_even_when_guard_is_unavailable(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import Mock
        from execution_pod import ExecutionPodController
        with tempfile.TemporaryDirectory() as directory:
            backend = Mock(root=Path(directory))
            controller = ExecutionPodController(backend, {}, admitted=True)
            operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            path = controller._command_path(operation)
            path.write_text(json.dumps({'operation_id': operation, 'scratch_space_id': 's',
                                        'pod_uid': 'uid', 'status': 'running'}))
            controller._stop_matching_pod = Mock(side_effect=RuntimeError('node gone'))
            controller.recover_commands()
            self.assertEqual(json.loads(path.read_text())['status'], 'interrupted')
            with self.assertRaisesRegex(RuntimeError, 'termination'):
                controller._require_space_termination_confirmed('s')

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
            socket_path = str(Path(directory) / 'session.sock')
            server = subprocess.Popen([sys.executable, '-I', 'python_session.py', 'serve', socket_path],
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

    def test_cell_output_is_bounded(self):
        import os
        import subprocess
        import sys
        import tempfile
        import time
        from pathlib import Path
        from python_session import send_cell
        with tempfile.TemporaryDirectory() as directory:
            socket_path = str(Path(directory) / 'session.sock')
            server = subprocess.Popen([sys.executable, '-I', 'python_session.py', 'serve', socket_path],
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
        import subprocess
        import sys
        import tempfile
        from pathlib import Path
        from python_session import send_cell
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / 'delayed.sock')
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
        import os
        from unittest.mock import patch
        from sandbox_v2 import run_admission_probe
        with patch('sandbox_v2.enter_user_root') as enter, \
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
with patch('os.chroot'), patch('os.chdir'), patch('sandbox_v2.validate_system_root'), patch('sandbox_v2.drop_user_capabilities'), patch('os.execvp', execute):
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
            def kube(self, args, data=None, **_kwargs):
                if args[0] == 'get':
                    if args[1] == 'pvc':
                        body = {'metadata': {'uid': 'claim-uid'}, 'spec': {'volumeName': 'volume-1'}}
                    elif args[1] == 'pv' and not self.deleted:
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
            self.assertEqual(controller.delete('scratch'), {'deleted': True})


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
            controller.session_executor.shutdown(wait=True)

    def test_init_only_mounts_system_subpath(self):
        from execution_pod import execution_manifests
        settings = {'scratch_space_id':'s','image':'image@sha256:'+'a'*64,'cpu':2,'memory_bytes':2<<30,'capacity_bytes':2<<30,'docker_enabled':False}
        config = {'runtime_class':'aidash-gvisor','helper_image':'helper@sha256:'+'b'*64,'namespace':'sandbox'}
        _, pod = execution_manifests(settings, config)
        image = next(item for item in pod['spec']['initContainers'] if item['name'] == 'image-root')
        self.assertEqual(next(m for m in image['volumeMounts'] if m['name'] == 'scratch')['subPath'], 'image-root')
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
            controller.session_executor.shutdown(wait=True)

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
            controller.session_executor.shutdown(wait=True)

    def test_docker_stream_reserves_local_transport_failure_code(self):
        from sandbox_v2 import docker_supervisor_exit_code
        self.assertEqual(docker_supervisor_exit_code({'status':'completed','exit_code':1}), 123)
        self.assertEqual(docker_supervisor_exit_code({'status':'completed','exit_code':123}), 126)
        self.assertEqual(docker_supervisor_exit_code({'status':'timed_out'}), 124)
        self.assertEqual(docker_supervisor_exit_code({'status':'stopped'}), 125)

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
            transport = {'exit_code':0,'transport_timed_out':False,'output':{'stdout':'b2s=','stderr':'','truncated':False}}
            with patch('execution_pod.drain_process', return_value=transport):
                controller._execute_command(operation)
            result = controller.command_result(operation)
            self.assertEqual((result['status'], result['exit_code'], result['stdout']), ('completed', 0, 'b2s='))
            record = json.loads(controller._command_path(operation).read_text())
            record['status'] = 'queued'
            controller._command_path(operation).write_text(json.dumps(record))
            transport['exit_code'] = 123
            with patch('execution_pod.drain_process', return_value=transport):
                controller._execute_command(operation)
            result = controller.command_result(operation)
            self.assertEqual((result['status'], result['exit_code']), ('completed', 1))
            controller.session_executor.shutdown(wait=True)
