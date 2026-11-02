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
        controller._verify_running_pod = Mock()
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

        class Backend:
            def __init__(self, root):
                self.root = Path(root)
                self.calls = []
            def kube(self, args, data=None, **_kwargs):
                self.calls.append(args)
                if args[0] == 'get':
                    return SimpleNamespace(stdout=json.dumps({'metadata': {'uid': 'pod-uid'}}).encode())
                return SimpleNamespace(stdout=b'')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'execution-v2'
            root.mkdir()
            operation = 'e6ded09d-f783-4ce8-98e9-dbfca68884f1'
            record = {'operation_id': operation, 'scratch_space_id': 'scratch', 'run_id': 'run',
                      'status': 'running', 'background': True, 'pod_uid': 'pod-uid'}
            (root / (operation + '.json')).write_text(json.dumps(record))
            backend = Backend(directory)
            controller = ExecutionPodController(backend, {}, admitted=False)
            controller.recover_commands()
            result = json.loads((root / (operation + '.json')).read_text())
            self.assertEqual(result['status'], 'interrupted')
            self.assertEqual(result['outcome'], 'unknown')
            self.assertEqual(len(backend.calls), 2)
            self.assertEqual(backend.calls[1][:2], ['delete', 'pod'])

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
                    return SimpleNamespace(stdout=json.dumps({'metadata': {'uid': 'pod-uid'}}).encode())
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
        info = {'Runtime': {'Name': 'io.containerd.runsc.v1'}, 'Labels': {
            'io.kubernetes.pod.namespace': node_guard.NAMESPACE, 'io.kubernetes.pod.uid': pod_uid,
            'io.kubernetes.container.name': 'helpers', 'io.kubernetes.pod.name': request['pod_name']}}
        state = {'annotations': {'io.kubernetes.cri.image-name': request['image']}}
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
                  'process_limit': [128, 128], 'service_account_absent': True, 'raw_socket': False}
        verify_user_probe(report, docker=False, processes=128)
        for field in report['caps']:
            bad = dict(report, caps=dict(report['caps'], **{field: 1 << 21}))
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'capabilit'):
                verify_user_probe(bad, docker=False, processes=128)
        with self.assertRaisesRegex(ValueError, 'service account'):
            verify_user_probe(dict(report, service_account_absent=False), docker=False, processes=128)

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


class AdmissionGateOrderTest(unittest.TestCase):
    def test_guard_checks_cgroup_before_releasing_user_image(self):
        import json
        from types import SimpleNamespace
        from execution_pod import ExecutionPodController
        events = []
        pod = {'metadata': {'uid': 'pod-uid', 'annotations': {'aidash.run/epoch': '1'}},
               'spec': {'nodeName': 'node-1'}, 'status': {'initContainerStatuses': [
                   {'name': 'helpers', 'containerID': 'containerd://' + 'a' * 64, 'state': {'running': {}}}]}}
        class Backend:
            def kube(self, args, data=None, **_kwargs):
                if args[0] == 'get':
                    return SimpleNamespace(stdout=json.dumps(pod).encode(), returncode=0)
                if '.guard-waiting' in args[-1]:
                    events.append('waiting')
                    return SimpleNamespace(stdout=b'', returncode=0)
                if 'getrlimit' in args[-1]:
                    events.append('process-limit')
                    return SimpleNamespace(stdout=b'[128,128]', returncode=0)
                events.append('release')
                return SimpleNamespace(stdout=b'', returncode=0)
            def guard_execution_pod(self, *_args):
                events.append('guard')
        controller = ExecutionPodController(Backend(), {'processes': 128}, admitted=True)
        controller._admit_pod('scratch-' + 'a' * 48, {})
        self.assertEqual(events, ['waiting', 'guard', 'process-limit', 'release'])
