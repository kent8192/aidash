"""Durable UID-bound provider receipts, independent of operation payload retention."""
import bisect
from datetime import datetime, timezone
import hashlib
import json
import time
from runner_journal import Rejected, identity, save

FINALIZER = 'aidash.dev/allocation-receipt'


def instant(value):
    parsed = datetime.fromisoformat(value.replace('Z', '+00:00'))
    if parsed.tzinfo is None or parsed.year < 1971:
        raise ValueError('provider timestamp unavailable')
    return parsed.astimezone(timezone.utc)


def digest(receipt):
    return hashlib.sha256(json.dumps(receipt, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()


class AllocationJournal:
    def initialize_allocations(self, fresh_instance=False):
        self.allocations = self.root / 'allocations'
        marker = self.allocations / 'instance'
        # The initialization anchor is outside the subjournal whose loss it detects.
        anchor = self.root / 'allocation-journal'
        if not anchor.exists():
            if not fresh_instance or self.allocations.exists():
                raise RuntimeError('allocation journal initialization unavailable')
            save(anchor, self.instance)
            self.allocations.mkdir(mode=0o700)
            save(marker, self.instance)
        try:
            self.confirm_allocation_instance()
        except Rejected as error:
            raise RuntimeError('allocation journal identity unavailable') from error
        self.allocation_pending = []
        self.allocation_watch = []
        self.allocation_incomplete = set()
        self.allocation_cursor = ''
        # One startup scan. Requests page the index and open at most limit files.
        for path in self.allocations.glob('*.json'):
            record = json.loads(path.read_bytes())
            if record['runner_instance'] != self.instance:
                raise RuntimeError('allocation journal identity changed')
            if not record.get('released'):
                self.allocation_watch.append(path.stem)
            if not record.get('acknowledged'):
                self.allocation_pending.append(path.stem)
                if record.get('pending_reason'):
                    self.allocation_incomplete.add(path.stem)
            if record.get('quarantined'):
                self.allocation_incomplete.add(path.stem)
        self.allocation_pending.sort()
        self.allocation_watch.sort()

    def observe_allocation(self, record, pod):
        if record.get('admission_probe'):
            return
        tenant = record.get('request', {}).get('usage_tenant')
        uid = identity(pod['metadata']['uid'])
        path = self.allocations / (uid + '.json')
        with self.lock:
            operation = record.get('pod_operation', record['operation_id'])
            origin = self.get(operation)
            annotations = pod['metadata'].get('annotations', {})
            if (record.get('pod_uid', uid) != uid or origin.get('pod_uid', uid) != uid
                    or record['area_id'] != origin['area_id']
                    or pod['metadata']['name'] != self.pod_name(operation)
                    or annotations.get('aidash/digest') != origin['wire_digest']
                    or annotations.get('aidash/area') != origin['area_id']
                    or annotations.get('aidash/epoch') != str(origin['epoch'])):
                bound = origin.get('pod_uid') or record.get('pod_uid')
                bound_path = self.allocations / (identity(bound)+'.json') if bound else None
                if bound_path and bound_path.exists():
                    self.quarantine_allocation(bound_path, json.loads(bound_path.read_bytes()), 'allocation origin identity changed')
                raise Rejected(409, 'allocation origin identity changed')
            if not origin.get('pod_uid'):
                # Persist first adoption in the trusted operation journal before indexing usage.
                self.update(operation, pod_uid=uid)
            old = json.loads(path.read_bytes()) if path.exists() else None
            if old and old.get('quarantined'):
                self.allocation_diagnostic(old)
                raise Rejected(503, 'allocation evidence quarantined')
            if not old:
                tenant = origin.get('request', {}).get('usage_tenant')
                if not isinstance(tenant, str) or not tenant or len(tenant.encode()) > 255:
                    raise Rejected(409, 'native allocation usage context unavailable')
                old = dict(allocation_id=uid, runner_instance=self.instance, tenant=tenant,
                           operation_id=operation, area_id=origin['area_id'], epoch=origin['epoch'],
                           pod_name=pod['metadata']['name'], wire_digest=origin['wire_digest'],
                           first_observed_at=time.time(), work_operation=record['operation_id'],
                           work_epoch=record['epoch'], cleanup_ready=False, idle_cleanup_ready=False)
                bisect.insort(self.allocation_pending, uid)
                bisect.insort(self.allocation_watch, uid)
                self.allocation_incomplete.add(uid)
            if (old['runner_instance'] != self.instance or old['area_id'] != record['area_id']
                    or old['pod_name'] != pod['metadata']['name']
                    or old['wire_digest'] != pod['metadata']['annotations'].get('aidash/digest')
                    or tenant is not None and tenant != old['tenant']):
                self.quarantine_allocation(path, old, 'allocation identity changed')
                raise Rejected(409, 'allocation identity changed')
            old['finalizers'] = pod['metadata'].get('finalizers', [])
            old['resource_version'] = pod['metadata'].get('resourceVersion')
            if record['epoch'] > old.get('work_epoch', 0):
                old.update(work_operation=record['operation_id'], work_epoch=record['epoch'],
                           cleanup_ready=False, idle_cleanup_ready=False)
            start = pod.get('status', {}).get('startTime')
            try:
                instant(start)
            except (ValueError, TypeError, AttributeError):
                start = None
            if old.get('receipt') and not start:
                self.quarantine_allocation(path, old, 'allocation provider start unavailable after receipt')
                raise Rejected(409, 'allocation provider start unavailable')
            if old.get('started_at') and start and start != old['started_at']:
                self.quarantine_allocation(path, old, 'allocation provider start changed')
                raise Rejected(409, 'allocation provider start changed')
            old['started_at'] = old.get('started_at') or start
            # Keep latest trusted guard binding for Python cells; origin stays fixed.
            if record.get('container_id') and record['epoch'] >= old.get('monitor', {}).get('epoch', 0):
                old['monitor'] = {k:v for k,v in record.items() if k in
                    ('operation_id', 'pod_operation', 'pod_uid', 'container_id', 'area_id', 'epoch', 'image', 'cluster_node')}
            expected = [c['name'] for group in ('initContainers', 'containers')
                        for c in pod['spec'].get(group, [])]
            statuses = {c['name']:c for group in ('initContainerStatuses', 'containerStatuses')
                        for c in pod.get('status', {}).get(group, [])}
            try:
                containers = []
                for name in sorted(expected):
                    status = statuses[name]
                    if status.get('restartCount', 0) != 0:
                        raise ValueError('unexpected container restart')
                    finished = status['state']['terminated']['finishedAt']
                    if instant(finished) < instant(old['started_at']):
                        raise ValueError('provider stop precedes start')
                    containers.append(dict(name=name, finished_at=finished))
                if not containers:
                    raise ValueError('provider container evidence unavailable')
                end = max(containers, key=lambda c: instant(c['finished_at']))['finished_at']
                receipt = {k:old[k] for k in ('allocation_id', 'runner_instance', 'tenant', 'area_id', 'epoch', 'operation_id')}
                receipt.update(protocol='aidash-allocation/1', started_at=old['started_at'], ended_at=end,
                               evidence=dict(pod_uid=uid, start_time=old['started_at'], containers=containers))
                receipt['digest'] = digest(receipt)
                if old.get('receipt') and old['receipt'] != receipt:
                    self.quarantine_allocation(path, old, 'allocation receipt evidence changed')
                    raise Rejected(409, 'allocation receipt changed')
                old['receipt'] = receipt
                old.pop('pending_reason', None)
                self.allocation_incomplete.discard(uid)
            except (KeyError, ValueError, TypeError, AttributeError):
                if old.get('receipt'):
                    self.quarantine_allocation(path, old, 'allocation terminal evidence changed or unavailable')
                    raise Rejected(409, 'allocation terminal evidence unavailable') from None
                stopped = expected and all('terminated' in statuses.get(name, {}).get('state', {}) for name in expected)
                if stopped or old.get('stop_requested') or pod.get('status', {}).get('phase') in ('Failed', 'Succeeded'):
                    old['pending_reason'] = 'provider start/stop evidence unavailable'
                    self.allocation_incomplete.add(uid)
                else:
                    old.pop('pending_reason', None)
                    self.allocation_incomplete.discard(uid)
            save(path, old)

    def quarantine_allocation(self, path, allocation, reason):
        allocation.update(quarantined=True, pending_reason=reason,
                          cleanup_blocker='provider evidence quarantined; manual recovery required')
        save(path, allocation)
        self.allocation_incomplete.add(allocation['allocation_id'])
        self.allocation_diagnostic(allocation)

    def allocation_receipts(self, after='', limit=16):
        if after:
            identity(after)
        if type(limit) is not int or not 1 <= limit <= 64:
            raise Rejected(400, 'allocation page limit')
        with self.lock:
            self.confirm_allocation_instance()
            offset = bisect.bisect(self.allocation_pending, after)
            ids = self.allocation_pending[offset:offset + limit]
            records = [json.loads((self.allocations / (uid + '.json')).read_bytes()) for uid in ids]
            return dict(instance=self.instance, receipts=[r['receipt'] for r in records
                        if r.get('receipt') and not r.get('quarantined') and not r.get('pending_reason')],
                        next_after=ids[-1] if len(ids) == limit else '', pending_evidence=len(self.allocation_incomplete))

    def acknowledge_allocation(self, uid, instance, receipt_digest):
        uid = identity(uid)
        with self.lock:
            self.confirm_allocation_instance()
            path = self.allocations / (uid + '.json')
            if not path.exists():
                raise Rejected(409, 'allocation receipt unavailable')
            record = json.loads(path.read_bytes())
            if (instance != self.instance or record['runner_instance'] != instance
                    or record.get('quarantined') or record.get('pending_reason')
                    or not record.get('receipt') or record['receipt']['digest'] != receipt_digest):
                raise Rejected(409, 'allocation receipt identity mismatch')
            record['acknowledged'] = True
            save(path, record)
            offset = bisect.bisect_left(self.allocation_pending, uid)
            if offset < len(self.allocation_pending) and self.allocation_pending[offset] == uid:
                self.allocation_pending.pop(offset)
            return dict(allocation_id=uid, instance=instance, digest=receipt_digest, acknowledged=True)

    def confirm_allocation_instance(self):
        # Callers hold the journal lock through the following read or acknowledgement.
        try:
            if (json.loads((self.root / 'allocation-journal').read_bytes()) == self.instance
                    and json.loads((self.allocations / 'instance').read_bytes()) == self.instance):
                return
        except (OSError, ValueError):
            pass
        raise Rejected(503, 'allocation journal identity unavailable')

    def begin_allocation_work(self, record):
        uid = record.get('pod_uid')
        if not uid or record.get('admission_probe'):
            return
        with self.lock:
            path = self.allocations / (identity(uid) + '.json')
            allocation = json.loads(path.read_bytes())
            allocation.update(work_operation=record['operation_id'], work_epoch=record['epoch'],
                              cleanup_ready=False, idle_cleanup_ready=False)
            save(path, allocation)

    def allocation_harvested(self, record, idle=False):
        uid = record.get('pod_uid')
        if not uid or record.get('admission_probe'):
            return
        with self.lock:
            path = self.allocations / (identity(uid) + '.json')
            if not path.exists():
                return
            allocation = json.loads(path.read_bytes())
            if (allocation.get('work_operation') != record['operation_id']
                    or allocation.get('work_epoch') != record['epoch']):
                return
            # Only the output owner calls this after fsync and its terminal journal save.
            allocation.update(cleanup_ready=True, idle_cleanup_ready=idle)
            save(path, allocation)

    def close_allocation(self, record):
        with self.lock:
            operation = record.get('pod_operation', record['operation_id'])
            uid = record.get('pod_uid')
            if not uid:
                print(json.dumps(dict(event='allocation_usage', operation_id=operation,
                                      status='evidence_unavailable', cleanup_blocker='allocation UID unavailable')), flush=True)
                return
            if record.get('admission_probe'):
                self.delete_allocation(uid, self.pod_name(operation))
                return
            path = self.allocations / (identity(uid) + '.json')
            if not path.exists():
                print(json.dumps(dict(event='allocation_usage', allocation_id=uid, runner_instance=self.instance,
                                      status='evidence_unavailable', cleanup_blocker='allocation journal unavailable')), flush=True)
                return
            allocation = json.loads(path.read_bytes())
            allocation['stop_requested'] = True
            if allocation.get('quarantined'):
                self.allocation_diagnostic(allocation)
                return
            if not allocation.get('cleanup_ready'):
                allocation['cleanup_blocker'] = 'durable output harvest pending'
                save(path, allocation)
                self.allocation_diagnostic(allocation)
                return
            allocation['cleanup_attempts'] = allocation.get('cleanup_attempts', 0) + 1
            save(path, allocation)
            try:
                # API-side preconditions also protect the gap between observation and deletion.
                # Finalizer retains the API object while Kubernetes terminates containers.
                self.delete_allocation(uid, allocation['pod_name'], allocation.get('resource_version'))
                allocation.pop('cleanup_blocker', None)
                if allocation.get('receipt'):
                    finalizers = allocation.get('finalizers', [])
                    patch = [{'op':'test', 'path':'/metadata/uid', 'value':uid},
                             {'op':'test', 'path':'/metadata/finalizers', 'value':finalizers},
                             {'op':'replace', 'path':'/metadata/finalizers', 'value':[f for f in finalizers if f != FINALIZER]}]
                    self.kube(['patch', 'pod', allocation['pod_name'], '--type=json', '-p', json.dumps(patch)], check=True)
                    allocation['released'] = True
                    if uid in self.allocation_watch:
                        self.allocation_watch.remove(uid)
                else:
                    allocation['pending_reason'] = 'provider start/stop evidence unavailable'
                    self.allocation_incomplete.add(uid)
            except Exception as error:
                allocation['cleanup_blocker'] = str(error)[-2000:]
                if not allocation.get('receipt'):
                    allocation['pending_reason'] = 'provider cleanup/evidence unavailable'
                    self.allocation_incomplete.add(uid)
            save(path, allocation)
            self.allocation_diagnostic(allocation)

    def delete_allocation(self, uid, name, resource_version=None):
        preconditions = {'uid':identity(uid)}
        if resource_version:
            preconditions['resourceVersion'] = resource_version
        self.kube(['delete', '--raw', '/api/v1/namespaces/' + self.config['namespace'] + '/pods/' + name, '-f', '-'],
                  json.dumps({'preconditions':preconditions}).encode(), check=True)

    def allocation_diagnostic(self, allocation):
        status = ('evidence_unavailable' if allocation.get('quarantined') else
                  'acknowledged' if allocation.get('acknowledged') else
                  'receipt_pending_delivery' if allocation.get('receipt') else
                  'evidence_unavailable' if allocation.get('evidence_lost') else
                  'pending_evidence' if allocation.get('pending_reason') else 'running')
        first = allocation.get('first_observed_at')
        # Observation age is operational only; it never enters the billable receipt.
        print(json.dumps(dict(event='allocation_usage', allocation_id=allocation['allocation_id'],
                              runner_instance=allocation['runner_instance'], status=status,
                              pending_reason=allocation.get('pending_reason'), cleanup_blocker=allocation.get('cleanup_blocker'),
                              age_seconds=max(0, time.time() - first) if first is not None else None,
                              observation_attempts=allocation.get('observation_attempts', 0),
                              cleanup_attempts=allocation.get('cleanup_attempts', 0))), flush=True)


    def allocation_housekeeping(self):
        with self.lock:
            offset = bisect.bisect(self.allocation_watch, self.allocation_cursor)
            ids = self.allocation_watch[offset:offset + 16]
            self.allocation_cursor = ids[-1] if len(ids) == 16 else ''
        for uid in ids:
            path = self.allocations / (uid + '.json')
            try:
                # Serialize journal writes with observation and acknowledgement, including
                # the read/modify/write around a provider call and its diagnostic state.
                with self.lock:
                    allocation = json.loads(path.read_bytes())
                    allocation['observation_attempts'] = allocation.get('observation_attempts', 0) + 1
                    save(path, allocation)
                    if allocation.get('quarantined'):
                        self.allocation_diagnostic(allocation)
                        continue
                    result = self.kube(['get', 'pod', allocation['pod_name'], '-o', 'json', '--ignore-not-found'])
                    pod = json.loads(result.stdout) if result.stdout.strip() else None
                    if pod is None or pod['metadata']['uid'] != uid:
                        if not allocation.get('receipt'):
                            allocation['pending_reason'] = 'allocation provider identity/evidence lost'
                            allocation['evidence_lost'] = True
                            allocation['cleanup_blocker'] = 'provider Pod absent or replaced; retained evidence requires manual recovery'
                            save(path, allocation)
                            self.allocation_incomplete.add(uid)
                            self.allocation_diagnostic(allocation)
                            continue
                        allocation['released'] = True
                        save(path, allocation)
                        self.allocation_watch.remove(uid)
                        continue
                    record = dict(operation_id=allocation['operation_id'], area_id=allocation['area_id'],
                                  epoch=allocation['epoch'], pod_uid=uid, request={'usage_tenant':allocation['tenant']})
                    self.observe_allocation(record, pod)
                    allocation = json.loads(path.read_bytes())
                    monitor = allocation.get('monitor')
                    # A current non-idle harvest also hands off stop intent, including
                    # replay after the terminal save but before the first DELETE.
                    harvested_stop = allocation.get('cleanup_ready') and not allocation.get('idle_cleanup_ready')
                    stopped = (allocation.get('idle_cleanup_ready') and not allocation.get('receipt') and monitor
                               and self.guard(monitor, 'status').get('termination_confirmed') is True)
                    if (allocation.get('receipt') or allocation.get('stop_requested')
                            or pod['metadata'].get('deletionTimestamp') or harvested_stop or stopped):
                        self.close_allocation(record)
                    elif allocation.get('pending_reason'):
                        self.allocation_diagnostic(allocation)
            except Exception as error:
                with self.lock:
                    allocation = json.loads(path.read_bytes())
                    allocation['cleanup_blocker'] = str(error)[-2000:]
                    save(path, allocation)
                    self.allocation_diagnostic(allocation)

    def allocation_loop(self):
        while True:
            self.allocation_housekeeping()
            time.sleep(1)
