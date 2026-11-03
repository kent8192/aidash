import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import uuid


class CtrPathTests(unittest.TestCase):
    def test_guard_runs_the_ctr_named_by_the_override(self):
        # Setup and Helm pass the node's ctr path explicitly (#680); the guard
        # never searches PATH for a binary it runs as root.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pod, area = str(uuid.uuid4()), str(uuid.uuid4())
            request = {'action': 'status', 'container_id': 'a' * 64, 'pod_uid': pod,
                       'area_id': area, 'epoch': 1, 'image': 'image'}
            info = {'ID': request['container_id'], 'Labels': {'io.kubernetes.pod.namespace': 'aidash-sandbox',
                               'io.kubernetes.pod.uid': pod,
                               'io.kubernetes.container.name': 'execution',
                               'io.kubernetes.pod.name': 'operation-test'},
                    'Runtime': {'Name': 'io.containerd.runsc.v1'}}
            ctr = root / 'ctr'
            ctr.write_text('#!/bin/sh\nprintf %s ' + "'" + json.dumps(info) + "'" + '\n')
            ctr.chmod(0o755)
            runsc = root / 'runsc'
            runsc.write_text('#!/bin/sh\nprintf %s ' + "'" + json.dumps({'id': request['container_id'], 'status': 'running', 'annotations': {'io.kubernetes.cri.image-name': 'image'}}) + "'" + '\n')
            runsc.chmod(0o755)
            env = dict(os.environ, PATH='', AIDASH_RUNSC_BINARY=str(runsc),
                       AIDASH_CTR_BINARY=str(ctr), AIDASH_GUARD_STATE=str(root / 'state'))
            script = Path(__file__).with_name('node_guard.py')
            result = subprocess.run([sys.executable, str(script)], input=json.dumps(request),
                                    text=True, capture_output=True, env=env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(json.loads(result.stdout)['status'], 'running')


if __name__ == '__main__':
    unittest.main()
