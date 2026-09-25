#!/usr/bin/env python3
"""Crash only the owned acceptance controller; prove durable, non-replayed effects."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('private', type=Path)
    parser.add_argument('evidence', type=Path)
    parser.add_argument('controller_pid', type=int)
    args = parser.parse_args()
    private = args.private.resolve()
    evidence = args.evidence.resolve()
    assert (private / 'owner.json').is_file(), 'an owned disposable installation is required'
    original_profile = (private / 'profile.json').read_text()
    profile = json.loads(original_profile)
    runner = profile['runner']
    assert runner['endpoint'].startswith('http://127.0.0.1:')
    token = (private / 'token').read_text().strip()
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    checks = []
    report = {'schema': 'aidash-runner-recovery/1', 'checks': checks, 'passed': False}

    def check(name, condition):
        checks.append({'name': name, 'passed': bool(condition)})
        assert condition, name

    def call(path, value=None, expected=200):
        data = None if value is None else json.dumps(value).encode()
        request = urllib.request.Request(runner['endpoint'] + path, data=data, headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
        try:
            response = client.open(request, timeout=15)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            assert response.status == expected, (path, response.status)
            return json.loads(response.read())

    def until(operation, states, seconds=120):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            result = call('/v1/operations/' + operation)
            if result['status'] in states:
                return result
            assert result['status'] not in ('failed', 'cancelled', 'uncertain'), result['status']
            time.sleep(0.2)
        raise AssertionError('operation did not reach ' + repr(states))

    def prepare(code):
        request = {'operation_id': str(uuid.uuid4()), 'area_id': str(uuid.uuid4()), 'epoch': 1, 'digest': hashlib.sha256(code.encode()).hexdigest(), 'kind': 'shell', 'code': code, 'seconds': 120, 'files': []}
        call('/v1/operations', request)
        call('/v1/operations/' + request['operation_id'] + '/start', {})
        return request

    try:
        before = call('/v1/health')
        identity = json.loads((Path(runner['journal']) / 'instance').read_text())
        check('controller belongs to the owned persistent journal', before['instance'] == identity)
        command = subprocess.check_output(['ps', '-p', str(args.controller_pid), '-o', 'command='], text=True)
        check('crash targets this checkout controller', str(ROOT / 'runner/control.py') in command)
        known = prepare("printf 'once\\n' >> count.txt; sleep 20; printf 'known result\\n'")
        until(known['operation_id'], ['running'])
        unknown = prepare("printf 'possible prior effect\\n' > uncertain.txt; sleep 100")
        until(unknown['operation_id'], ['running'])
        os.kill(args.controller_pid, signal.SIGKILL)
        # Remove the one accepted fixture Pod while the observer is down. Its
        # effects cannot be reconstructed from an absent collector or journal.
        kube = [runner['kubectl'], '--kubeconfig', runner['kubeconfig'], '--namespace', runner['namespace']]
        pods = json.loads(subprocess.check_output(kube + ['get', 'pods', '-l', 'aidash-area=' + unknown['area_id'], '-o', 'json']))['items']
        journal = json.loads((Path(runner['journal']) / (unknown['operation_id'] + '.json')).read_text())
        check('only the unknown fixture Pod is selected', len(pods) == 1 and pods[0]['metadata']['uid'] == journal['pod_uid'])
        subprocess.run(kube + ['delete', 'pod', pods[0]['metadata']['name'], '--wait=true', '--timeout=60s'], check=True, stdout=subprocess.DEVNULL)
        # Simulate unavailable node evidence for only this destroyed Pod. Other
        # Pods, including fresh deployment admission probes, use the real adapter.
        # This avoids pretending a recoverable, proven-stopped writer is unknown.
        fault_adapter = private / 'lost-fixture-evidence.py'
        fault_adapter.write_text(
            'import json, subprocess, sys\n'
            'data = sys.stdin.buffer.read()\n'
            f'if json.loads(data)["pod_uid"] == {journal["pod_uid"]!r}:\n'
            '    print(json.dumps({"error": "fixture node evidence unavailable"})); sys.exit(1)\n'
            f'sys.exit(subprocess.run({runner["node_guard"]!r}, input=data).returncode)\n'
        )
        runner['node_guard'] = [sys.executable, str(fault_adapter)]
        (private / 'profile.json').write_text(json.dumps(profile))
        with (evidence / 'runner-restarted.log').open('w') as log:
            restarted = subprocess.Popen([sys.executable, str(ROOT / 'scripts/start-capability-runner.py'), str(private)], stdout=log, stderr=subprocess.STDOUT)
        (private / 'runner.pid').write_text(str(restarted.pid))
        deadline = time.monotonic() + 180
        while True:
            assert restarted.poll() is None, 'restarted controller exited'
            try:
                after = call('/v1/health')
                break
            except (urllib.error.URLError, TimeoutError):
                assert time.monotonic() < deadline, 'restart admission timeout'
                time.sleep(1)
        check('restart repeats physical isolation admission', after['verified'] and after['python_verified'])
        check('journal identity survives process death', after['instance'] == before['instance'])
        result = until(known['operation_id'], ['completed'])
        check('known result has a proven stopped writer', result['termination_confirmed'])
        files = [file for file in result['files'] if file['path'] == 'count.txt']
        check('known output is exported once', len(files) == 1)
        data = call('/v1/operations/' + known['operation_id'] + '/files/' + files[0]['object_id'])
        check('command was never replayed', base64.b64decode(data['data']) == b'once\n')
        repeated = call('/v1/operations', known)
        check('same invocation returns the durable result', repeated['status'] == 'completed' and repeated['files'] == result['files'])
        changed = dict(known, code='echo must-not-run')
        call('/v1/operations', changed, expected=409)
        checks.append({'name': 'changed input under the same operation ID conflicts', 'passed': True})
        lost = until(unknown['operation_id'], ['uncertain'])
        check('unobserved prior effects stay uncertain', lost['status'] == 'uncertain')
        replay = call('/v1/operations', unknown)
        check('retry does not recreate an uncertain sandbox', replay['status'] == 'uncertain')
        replacement = dict(unknown, operation_id=str(uuid.uuid4()), epoch=2)
        if not lost.get('termination_confirmed'):
            call('/v1/operations', replacement, expected=409)
            checks.append({'name': 'unproven writer blocks epoch takeover', 'passed': True})
        newer = dict(known, operation_id=str(uuid.uuid4()), epoch=2, code='echo newer generation')
        call('/v1/operations', newer)
        call('/v1/operations/' + newer['operation_id'] + '/start', {})
        until(newer['operation_id'], ['completed'])
        stale = dict(known, operation_id=str(uuid.uuid4()))
        call('/v1/operations', stale, expected=409)
        checks.append({'name': 'a stale execution epoch cannot create work after takeover', 'passed': True})
        report.update(passed=True, image=after['image'], instance=after['instance'])
        print('Owned controller crash/recovery probes passed.')
    finally:
        (private / 'profile.json').write_text(original_profile)
        (evidence / 'runner-recovery.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
