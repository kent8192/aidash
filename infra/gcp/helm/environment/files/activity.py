"""Read-only Environment observation; publish one fail-closed snapshot."""
import hashlib
import json
import os
from pathlib import Path
import ssl
import time
import urllib.request


def digest(transfers):
    return hashlib.sha256('\n'.join(sorted(transfers)).encode()).hexdigest()


def combine(database, edge, runner, previous, now):
    if database['protocol'] != 'aidash-infra-activity/1' or runner['protocol'] != 'aidash-runner-activity/1':
        raise ValueError('unknown activity protocol')
    counts = dict(database['counts'], **runner['counts'], http_requests=edge['inflight'])
    if any(type(value) is not int or value < 0 for value in counts.values()):
        raise ValueError('invalid activity counts')
    gap = not 0 <= now - previous.get('observed_at', 0) <= 120
    # Periodic DB bookkeeping does not renew idle; sealing rechecks it later.
    active = any(value for key, value in counts.items() if key != 'database_work')
    last = max(previous.get('last_active', now), edge['last_active'],
               database.get('last_work_completed', 0), runner.get('last_work_completed', 0))
    # Receipt IDs are durable and only accumulate, while a ConfigMap is capped near
    # 1 MiB. Persist a fixed-size digest, never the set. Any change renews idle, so
    # the post-transfer interval is never shortened. A snapshot without a digest is
    # compared as its legacy list, or as no transfers when it has neither.
    transfers = database['completed_transfers']
    seen = digest(transfers)
    before = previous.get('completed_transfers_digest') or digest(previous.get('completed_transfers', []))
    if active or gap or previous.get('busy', True) or seen != before:
        last = now
    snapshot = {key: value for key, value in database.items() if key != 'completed_transfers'}
    return dict(snapshot, counts=counts, busy=active or gap, observation_gap=gap,
                observed_at=now, last_active=last, edge_closed=edge['closed'],
                completed_transfers_digest=seen, completed_transfers_count=len(transfers))


def request(url, token=None, *, data=None, method=None, context=None):
    headers = {'Content-Type': 'application/merge-patch+json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    with urllib.request.urlopen(urllib.request.Request(url, data=data, headers=headers,
                                                      method=method), context=context, timeout=15) as response:
        return json.load(response)


def main():
    account = Path('/var/run/secrets/kubernetes.io/serviceaccount')
    token = (account / 'token').read_text().strip()
    namespace = (account / 'namespace').read_text().strip()
    api = f'https://{os.environ["KUBERNETES_SERVICE_HOST"]}:{os.environ["KUBERNETES_SERVICE_PORT"]}'
    url = f'{api}/api/v1/namespaces/{namespace}/configmaps/{os.environ["ACTIVITY_CONFIGMAP"]}'
    context = ssl.create_default_context(cafile=str(account / 'ca.crt'))
    previous = request(url, token, context=context)
    try:
        database = json.loads(Path('/snapshot/database.json').read_bytes())
        edge = request(os.environ['EDGE_ACTIVITY_ENDPOINT'] + '/activity')
        runner = request(os.environ['RUNNER_ENDPOINT'] + '/v1/activity',
                         os.environ['AIDASH_CORE_RUNNER_TOKEN'])
        snapshot = combine(database, edge, runner,
                           json.loads(previous['data']['snapshot.json']), time.time())
    except Exception:
        # Unavailable observations never authorize a stop and never emit secrets.
        snapshot = {'protocol': 'aidash-infra-activity/1', 'busy': True,
                    'observed_at': time.time(), 'last_active': time.time(),
                    'observation_gap': True, 'error': 'activity unavailable'}
    patch = {'metadata': {'resourceVersion': previous['metadata']['resourceVersion']},
             'data': {'snapshot.json': json.dumps(snapshot)}}
    request(url, token, data=json.dumps(patch).encode(), method='PATCH', context=context)


if __name__ == '__main__':
    main()
