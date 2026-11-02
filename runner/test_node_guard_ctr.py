import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import uuid


class CtrPathTests(unittest.TestCase):
    def test_guard_uses_ctr_from_path_and_honors_override(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pod, area = str(uuid.uuid4()), str(uuid.uuid4())
            request = {'action': 'status', 'container_id': 'a' * 64, 'pod_uid': pod,
                       'area_id': area, 'epoch': 1, 'image': 'image'}
            info = {'Labels': {'io.kubernetes.pod.namespace': 'aidash-sandbox',
                               'io.kubernetes.pod.uid': pod,
                               'io.kubernetes.container.name': 'execution',
                               'io.kubernetes.pod.name': 'operation-test'},
                    'Runtime': {'Name': 'io.containerd.runsc.v1'}}
            ctr = root / 'ctr'
            ctr.write_text('#!/bin/sh\nprintf %s ' + "'" + json.dumps(info) + "'" + '\n')
            ctr.chmod(0o755)
            runsc = root / 'runsc'
            runsc.write_text('#!/bin/sh\nprintf %s ' + "'" + json.dumps({'status': 'running', 'annotations': {'io.kubernetes.cri.image-name': 'image'}}) + "'" + '\n')
            runsc.chmod(0o755)
            env = dict(os.environ, PATH=str(root), AIDASH_RUNSC_BINARY=str(runsc),
                       AIDASH_GUARD_STATE=str(root / 'state'))
            env.pop('AIDASH_CTR_BINARY', None)
            script = Path(__file__).with_name('node_guard.py')
            for override in (None, str(ctr)):
                if override:
                    env['AIDASH_CTR_BINARY'] = override
                    env['PATH'] = ''
                result = subprocess.run([sys.executable, str(script)], input=json.dumps(request),
                                        text=True, capture_output=True, env=env)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(json.loads(result.stdout)['status'], 'running')


if __name__ == '__main__':
    unittest.main()
