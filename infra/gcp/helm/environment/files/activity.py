"""Read-only Environment observation; publish one fail-closed snapshot.

The observer and edge contract is documented in "Activity observation contract"
in infra/gcp/helm/environment/README.md.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import ssl
import time
import urllib.error
import urllib.request

# The minute CronJob and the controller's on-demand Job may publish together.
PUBLISH_ATTEMPTS = 3
# The observer's output is a fixed-size summary of a few hundred bytes. Anything
# larger is not that contract, whatever it contains, so it is never trusted.
MAX_OBSERVATION_BYTES = 64 * 1024


def digest(transfers):
    # Only a previous snapshot written before the observer summarized receipts
    # carries a receipt list.
    return hashlib.sha256('\n'.join(sorted(transfers)).encode()).hexdigest()


def read_observation(path):
    data = path.read_bytes()
    if len(data) > MAX_OBSERVATION_BYTES:
        raise ValueError('oversized database observation')
    return json.loads(data)


def receipt_summary(database):
    """The observer's fixed-size receipt summary; anything else is unavailable."""
    count, seen = database.get('completed_transfers_count'), database.get('completed_transfers_digest')
    # The summary replaces the receipt list; an observer that still sends one is
    # not the bounded contract and its list must never reach the ConfigMap.
    valid = type(count) is int and count >= 0 and isinstance(seen, str) and re.fullmatch(r'[0-9a-f]{64}', seen)
    if 'completed_transfers' in database or not valid:
        raise ValueError('invalid receipt summary')
    return count, seen


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
    # Receipts are durable and only accumulate, while a ConfigMap is capped near
    # 1 MiB. The observer sends a fixed-size count and digest and this persists
    # the same pair, never the set. Any change renews idle, so the post-transfer
    # interval is never shortened. A previous snapshot without a digest is
    # compared as its legacy list, or as no transfers when it has neither.
    count, seen = receipt_summary(database)
    before = previous.get('completed_transfers_digest') or digest(previous.get('completed_transfers', []))
    # The pair is the renewal key: a changed count alone also renews.
    before_count = previous.get('completed_transfers_count', len(previous.get('completed_transfers', [])))
    if active or gap or previous.get('busy', True) or seen != before or count != before_count:
        last = now
    return dict(database, counts=counts, busy=active or gap, observation_gap=gap,
                observed_at=now, last_active=last, edge_closed=edge['closed'],
                completed_transfers_digest=seen, completed_transfers_count=count)


def request(url, token=None, *, data=None, method=None, context=None):
    headers = {'Content-Type': 'application/merge-patch+json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    with urllib.request.urlopen(urllib.request.Request(url, data=data, headers=headers,
                                                      method=method), context=context, timeout=15) as response:
        return json.load(response)


def unavailable():
    # Unavailable observations never authorize a stop and never emit secrets.
    now = time.time()
    return {'protocol': 'aidash-infra-activity/1', 'busy': True, 'observed_at': now, 'last_active': now,
            'observation_gap': True, 'error': 'activity unavailable'}


def main():
    account = Path('/var/run/secrets/kubernetes.io/serviceaccount')
    token = (account / 'token').read_text().strip()
    namespace = (account / 'namespace').read_text().strip()
    api = f'https://{os.environ["KUBERNETES_SERVICE_HOST"]}:{os.environ["KUBERNETES_SERVICE_PORT"]}'
    url = f'{api}/api/v1/namespaces/{namespace}/configmaps/{os.environ["ACTIVITY_CONFIGMAP"]}'
    context = ssl.create_default_context(cafile=str(account / 'ca.crt'))
    previous = request(url, token, context=context)
    try:
        observed = (
            read_observation(Path('/snapshot/database.json')),
            request(os.environ['EDGE_ACTIVITY_ENDPOINT'] + '/activity'),
            request(os.environ['RUNNER_ENDPOINT'] + '/v1/activity', os.environ['AIDASH_CORE_RUNNER_TOKEN']),
        )
    except Exception:
        observed = None
    for attempt in range(PUBLISH_ATTEMPTS):
        try:
            snapshot = unavailable() if observed is None else combine(
                *observed, json.loads(previous['data']['snapshot.json']), time.time())
        except Exception:
            snapshot = unavailable()
        # The seal accepts only the snapshot its own Job published: a concurrent
        # minute Job may publish later with evidence gathered before the drain.
        snapshot['observation_job'] = os.environ.get('OBSERVATION_JOB', '')
        patch = {'metadata': {'resourceVersion': previous['metadata']['resourceVersion']},
                 'data': {'snapshot.json': json.dumps(snapshot)}}
        try:
            request(url, token, data=json.dumps(patch).encode(), method='PATCH', context=context)
            return
        except urllib.error.HTTPError as error:
            if error.code != 409 or attempt + 1 == PUBLISH_ATTEMPTS:
                raise
        # Another collector published first: combine against its snapshot so the
        # gap, idle history and transfer digest stay continuous.
        previous = request(url, token, context=context)


if __name__ == '__main__':
    main()
