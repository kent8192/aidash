#!/usr/bin/env python3
"""Disposable #152 sandbox lifecycle. All output is allowlisted metadata.

prepare/plan/set-broker-image are local-only. create, probe and cleanup require separately authorized
invocations; this tool never runs Terraform apply or changes gcloud defaults.
"""
import argparse
import datetime
import fcntl
import json
import hashlib
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import tarfile
import urllib.error
import urllib.request


class Refused(Exception):
    pass


def require(condition, reason):
    if not condition:
        raise Refused(reason)


def stamp():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def save(path, value):
    path = Path(path)
    temp = path.with_suffix('.tmp')
    with os.fdopen(os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), 'w') as out:
        json.dump(value, out, indent=2)
        out.flush()
        os.fsync(out.fileno())
    os.replace(temp, path)
    directory = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def command(argv):
    try:
        result = subprocess.run(argv, capture_output=True, timeout=120, check=False)
    except (OSError, subprocess.TimeoutExpired):
        raise Refused('command unavailable or interrupted; outcome unknown') from None
    # Never include argv, stdout or stderr in an error (all can reflect tokens).
    require(result.returncode == 0, 'command failed; outcome unknown')
    return result.stdout


def cloud(project, *args):
    raw = command(['gcloud', *args, '--project=' + project, '--format=json', '--quiet'])
    try:
        return json.loads(raw or '{}')
    except (ValueError, UnicodeError):
        raise Refused('invalid cloud response') from None


def validate(m, require_image=True):
    require(m.get('schema') == 1, 'unsupported manifest')
    require(re.fullmatch('[a-f0-9]{32}', m.get('run_id', '')), 'invalid run id')
    require(re.fullmatch('[a-f0-9]{40}', m.get('source_sha', '')), 'invalid source SHA')
    digest = m.get('broker_digest')
    require((digest is None and not require_image) or isinstance(digest, str) and re.fullmatch('sha256:[a-f0-9]{64}', digest), 'reviewed broker digest required')
    require(m.get('broker_image_source_sha', m['source_sha']) == m['source_sha'], 'broker image source mismatch')
    require(re.fullmatch('[A-Z0-9]{6}-[A-Z0-9]{6}-[A-Z0-9]{6}', m.get('billing_account', '')), 'explicit billing account required')
    require(m.get('environment') == 'test', 'only unpublished test environment allowed')
    require(set(m.get('projects', {})) == {'app', 'byok'}, 'exactly two projects required')
    ids = [p.get('id', '') for p in m['projects'].values()]
    require(len(set(ids)) == 2 and all(re.fullmatch('aidash-sbx-[a-z0-9-]{1,18}[a-z0-9]', p) for p in ids), 'distinct sandbox project IDs required')
    require(m.get('parent') == 'none' or re.fullmatch(r'(folders|organizations)/[0-9]+', m.get('parent', '')), 'explicit parent or none required')


def owned(m, project):
    observed = cloud(project['id'], 'projects', 'describe', project['id'])
    require(observed.get('projectId') == project['id'], 'wrong project returned')
    require(observed.get('labels', {}).get('aidash-sandbox-run') == m['run_id'], 'project is not owned by this run')
    number = str(observed.get('projectNumber', ''))
    require(number.isdigit() and (project.get('number') in (None, number)), 'project number mismatch')
    expected_parent = None if m['parent'] == 'none' else {'type': m['parent'].split('/')[0][:-1], 'id': m['parent'].split('/')[1]}
    require((observed.get('parent') or None) == expected_parent, 'project parent mismatch')
    require(observed.get('lifecycleState') in ('ACTIVE', 'DELETE_REQUESTED'), 'unexpected project lifecycle')
    return observed


def prepare(args):
    directory = Path(args.directory).resolve()
    require(not directory.exists(), 'run directory already exists')
    m = dict(schema=1, run_id=secrets.token_hex(16), source_sha=args.source_sha,
             broker_digest=args.broker_digest, billing_account=args.billing_account,
             parent=args.parent, environment='test', status='prepared', created_at=stamp(),
             projects={key: dict(id=value, number=None, status='new') for key, value in [('app', args.app_project), ('byok', args.byok_project)]})
    validate(m, require_image=False)
    directory.mkdir(mode=0o700, parents=True)
    save(directory / 'manifest.json', m)
    return {'status': 'prepared', 'run_id': m['run_id']}


def create(m, path):
    require(m['status'] in ('prepared', 'creating', 'created'), 'creation not permitted in this lifecycle')
    m['status'] = 'creating'
    save(path, m)
    for p in m['projects'].values():
        if p['status'] == 'new':
            # No describe/import fallback on a rejected create: the API is the
            # atomic non-adoption boundary, even when an existing ID is hidden.
            p['status'] = 'creating'
            save(path, m)
            parent_flags = [] if m['parent'] == 'none' else ['--' + m['parent'].split('/')[0][:-1] + '=' + m['parent'].split('/')[1]]
            cloud(p['id'], 'projects', 'create', p['id'], *parent_flags,
                  '--labels=aidash-sandbox-run=' + m['run_id'])
        observed = owned(m, p)
        require(observed['lifecycleState'] == 'ACTIVE', 'cannot activate shutdown project')
        p['number'] = str(observed['projectNumber'])
        p['status'] = 'created'
        save(path, m)
        cloud(p['id'], 'billing', 'projects', 'link', p['id'], '--billing-account=' + m['billing_account'])
        billing = cloud(p['id'], 'billing', 'projects', 'describe', p['id'])
        require(billing.get('billingEnabled') is True and billing.get('billingAccountName') == 'billingAccounts/' + m['billing_account'], 'billing scope not verified')
    m['status'] = 'created'
    save(path, m)
    return {'status': 'created', 'run_id': m['run_id']}


def cleanup(m, path):
    require(m['status'] in ('creating', 'created', 'cleanup_pending', 'shutdown_verified'), 'no created projects to clean')
    m['status'] = 'cleanup_pending'
    save(path, m)
    errors = 0
    for p in m['projects'].values():
        if p['status'] == 'new':
            continue
        try:
            observed = owned(m, p)
            p['number'] = str(observed['projectNumber'])
            save(path, m)
            if observed['lifecycleState'] == 'ACTIVE':
                cloud(p['id'], 'billing', 'projects', 'unlink', p['id'])
                cloud(p['id'], 'projects', 'delete', p['id'])
            observed = owned(m, p)
            billing = cloud(p['id'], 'billing', 'projects', 'describe', p['id'])
            require(observed['lifecycleState'] == 'DELETE_REQUESTED', 'shutdown not observed')
            require(billing.get('billingEnabled') is False and not billing.get('billingAccountName'), 'billing unlink not observed')
            p.update(status='shutdown_verified', shutdown_observed_at=stamp())
            save(path, m)
        except Refused:
            errors += 1
    if errors:
        raise Refused('cleanup incomplete; rerun cleanup after resolving ownership or cloud access')
    m.update(status='shutdown_verified', cleanup_observed_at=stamp())
    save(path, m)
    return {'status': 'shutdown_verified', 'permanent_deletion': 'pending_google_recovery_period', 'credential_cleanup': 'project_credentials_disabled; local_login_credentials_not_managed'}


def source_receipt(m, path):
    receipt = json.loads((path.parent / 'source-manifest.json').read_text())
    require(receipt['source_sha'] == m['source_sha'] and receipt['run_id'] == m['run_id'], 'source archive identity mismatch')
    return receipt


def set_broker_image(m, path, args):
    require(m['status'] in ('prepared', 'created'), 'image registration not permitted in this lifecycle')
    require(args.source_sha == m['source_sha'], 'image must come from the recorded reviewed source')
    require(re.fullmatch('sha256:[a-f0-9]{64}', args.digest), 'actual reviewed image digest required')
    require(m.get('broker_digest') is None, 'registered broker image is immutable')
    source = path.parent / 'source'
    if source.exists():
        source_receipt(m, path)
        require(not (source / 'enterprise/infra/gcp/environments/sandbox.auto.tfvars.json').exists(), 'environment already prepared')
    require(not any((path.parent / name).exists() for name in ('plan-commands.json', 'environments-plan-commands.json')), 'environment plan already prepared')
    m.update(broker_digest=args.digest, broker_image_source_sha=args.source_sha, broker_image_registered_at=stamp())
    save(path, m)
    return {'status': 'broker_image_registered', 'source_sha': m['source_sha']}


def plan(m, path, args):
    phase = args.plan_phase
    require(phase in ('all', 'bootstrap', 'environments'), 'unknown plan phase')
    validate(m, require_image=phase != 'bootstrap')
    require(m['status'] == 'created', 'create both new projects before materializing plans')
    require(all(p.get('number', '').isdigit() for p in m['projects'].values()), 'project numbers required')
    phases = ['bootstrap', 'environments'] if phase == 'all' else [phase]
    if 'bootstrap' in phases:
        require(args.alert_email, 'explicit alert email required for bootstrap')
    target = path.parent / 'source'
    root = target / 'enterprise/infra/gcp'
    command_file = path.parent / ('plan-commands.json' if phase == 'all' else phase + '-plan-commands.json')
    require(not command_file.exists(), 'reviewed plan commands already exist')
    for name in phases:
        require(not (root / name / 'sandbox.auto.tfvars.json').exists() and not (root / name / 'sandbox.tfplan').exists(), 'reviewed plan inputs or plan already exist')
    if phase == 'environments':
        require((root / 'bootstrap/sandbox.auto.tfvars.json').exists(), 'prepare bootstrap from this source first')
    if target.exists():
        source_receipt(m, path)
    else:
        require(not (path.parent / 'source-manifest.json').exists(), 'previous source archive missing')
        repo = Path(__file__).resolve().parents[4]
        archive = command(['git', '-C', str(repo), 'archive', m['source_sha'], 'enterprise/infra/gcp'])
        import io
        target.mkdir(mode=0o700)
        with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
            bundle.extractall(target, filter='data')
        save(path.parent / 'source-manifest.json', dict(source_sha=m['source_sha'], run_id=m['run_id'], archive_sha256=hashlib.sha256(archive).hexdigest()))
    app, byok = (m['projects'][key]['id'] for key in ('app', 'byok'))
    commands = []
    if 'bootstrap' in phases:
        bootstrap = dict(project_id=app, byok_project_id=byok, state_bucket_name=app + '-state',
                         release_bucket_name=app + '-release', notification_bucket_name=app + '-notify',
                         byok_broker_environments=['test'], security_alert_emails=[args.alert_email],
                         gcip_enabled=True, environment_domains=['sandbox.invalid'])
        save(root / 'bootstrap/sandbox.auto.tfvars.json', bootstrap)
        commands += [
            ['terraform', '-chdir=' + str(root / 'bootstrap'), 'init', '-input=false'],
            ['terraform', '-chdir=' + str(root / 'bootstrap'), 'plan', '-input=false', '-out=sandbox.tfplan']]
    if 'environments' in phases:
        env = dict(project_id=app, byok_project_id=byok, state_bucket=app + '-state',
                   cloudflare_zone_id='unused-unpublished', deploy_service_account='aidash-deploy@' + app + '.iam.gserviceaccount.com',
                   domain='sandbox.invalid', environments={'test': dict(kind='test', generation=1,
                   incarnation=m['run_id'][:12], running=True, published=False, spot=True, release_sha=m['source_sha'])},
                   gcip_tenants={'own': {'tenant': 'sandbox-own'}, 'other': {'tenant': 'sandbox-other'}},
                   credential_brokers={'test': dict(enabled=True, byok_project_id=byok, secret_prefix='aidash-test-cred-',
                   broker_service_account_email='aidash-test-broker@' + app + '.iam.gserviceaccount.com',
                   signing_key_id=f'projects/{app}/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability')})
        save(root / 'environments/sandbox.auto.tfvars.json', env)
        # Only the environment stage materializes a reviewed image. Bootstrap
        # preserves the archived lock byte-for-byte, including its null guard.
        save(root / 'environments/broker-image.json', {'digest': m['broker_digest']})
        commands += [
            ['terraform', '-chdir=' + str(root / 'environments'), 'init', '-input=false', '-backend-config=bucket=' + app + '-state', '-backend-config=prefix=sandbox/' + m['run_id']],
            ['terraform', '-chdir=' + str(root / 'environments'), 'plan', '-input=false', '-out=sandbox.tfplan']]
    save(command_file, commands)
    return {'status': 'plans_prepared_not_executed', 'phases': phases, 'run_id': m['run_id']}


HOSTS = {'secretmanager.googleapis.com', 'cloudkms.googleapis.com', 'identitytoolkit.googleapis.com',
         'cloudresourcemanager.googleapis.com', 'iam.googleapis.com', 'iamcredentials.googleapis.com'}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def request(spec, token, project):
    req = urllib.request.Request(spec['url'], method=spec['method'],
        data=json.dumps(spec['body']).encode() if 'body' in spec else None,
        headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
    status = 0
    try:
        with urllib.request.build_opener(NoRedirect()).open(req, timeout=30) as response:
            status = response.status
            try:
                return status, json.loads(response.read(1048576))
            except (OSError, ValueError, UnicodeError):
                return status, {}
    except urllib.error.HTTPError as error:
        try:
            return error.code, json.loads(error.read(1048576))
        except (OSError, ValueError, UnicodeError):
            return error.code, {}
    except (OSError, ValueError, UnicodeError):
        return status, {}


def classify(status, body, spec):
    if spec['expected'] == 'deny' and 200 <= status < 300:
        return 'failed'
    if not isinstance(body, dict):
        return 'inconclusive'
    if spec['expected'] == 'allow':
        # A 2xx alone is not semantic acceptance. Assertions select non-secret
        # response fields; payloads themselves are never emitted.
        if not 200 <= status < 300:
            return 'failed'
        for key, expected in spec.get('assert', {}).items():
            value = body
            for part in key.split('.'):
                if isinstance(value, dict):
                    value = value.get(part)
                elif isinstance(value, list) and part.isdigit() and int(part) < len(value):
                    value = value[int(part)]
                else:
                    value = None
            if value != expected:
                return 'failed'
        return 'passed' if spec.get('assert') else 'observation_only'
    if 200 <= status < 300:
        return 'failed'
    error = body.get('error', {})
    if not isinstance(error, dict) or status != 403 or error.get('status') != 'PERMISSION_DENIED':
        return 'inconclusive'
    details = error.get('details', [])
    if not isinstance(details, list):
        return 'inconclusive'
    for detail in details:
        if not isinstance(detail, dict):
            continue
        metadata = detail.get('metadata', {})
        if not isinstance(metadata, dict):
            continue
        if (detail.get('@type') == 'type.googleapis.com/google.rpc.ErrorInfo'
                and detail.get('reason') == spec['denial_reason']
                and detail.get('domain') == 'iam.googleapis.com'
                and metadata.get('permission') == spec['permission']
                and metadata.get('resource') == spec['resource']):
            return 'passed'
    return 'inconclusive'


def probe(m, path, args):
    from urllib.parse import urlsplit, unquote
    require(m['status'] == 'created', 'probe requires active owned projects')
    specs = json.loads(Path(args.requests).read_text())
    require(isinstance(specs, list) and 0 < len(specs) <= 100, 'bounded request list required')
    for p in m['projects'].values():
        require(owned(m, p)['lifecycleState'] == 'ACTIVE', 'project is not active')
    evidence = path.parent / 'probe-results.json'
    results = json.loads(evidence.read_text()) if evidence.exists() else []
    require(isinstance(results, list) and len(results) + len(specs) <= 1000, 'evidence journal limit reached')
    # Validate every request before any request is sent.
    for spec in specs:
        require(spec.get('project') in m['projects'], 'unknown project role')
        p = m['projects'][spec['project']]
        parsed = urlsplit(spec['url'])
        require(parsed.scheme == 'https' and parsed.hostname in HOSTS and not parsed.query and not parsed.fragment and parsed.port in (None, 443) and parsed.username is None, 'unapproved probe URL')
        require(spec['method'] in ('GET', 'POST', 'DELETE', 'PATCH', 'PUT'), 'unsupported method')
        require(re.fullmatch('[a-z][a-z0-9-]{0,62}', spec['id']) and spec.get('assumption') in range(1, 9), 'invalid probe identifier')
        require(spec.get('expected') in ('allow', 'deny'), 'expected allow or deny required')
        principal = spec.get('identity', '')
        app = m['projects']['app']['id']
        require(re.fullmatch('[a-z][a-z0-9-]{4,28}[a-z0-9]@' + re.escape(app) + r'\.iam\.gserviceaccount\.com', principal), 'identity outside owned app project')
        # Reject alternate projects in URL, expected resource and JSON bodies,
        # including nested full resource names and percent-encoded paths.
        serialized = unquote(json.dumps(spec))
        allowed = {item for project in m['projects'].values() for item in (project['id'], project['number'])}
        targets = re.findall(r'projects/([^/\s"?:]+)', serialized)
        wildcard_iam = parsed.hostname in ('iam.googleapis.com', 'iamcredentials.googleapis.com') and spec['project'] == 'app'
        accounts = re.findall(r'serviceAccounts/([^/\s"?:]+)', serialized)
        if wildcard_iam and accounts and all(re.fullmatch('[a-z][a-z0-9-]{4,28}[a-z0-9]@' + re.escape(app) + r'\.iam\.gserviceaccount\.com', account) for account in accounts):
            allowed.add('-')
        require(targets and all(item in allowed for item in targets), 'resource outside owned projects')
        require((wildcard_iam and '-' in allowed and '/projects/-/serviceAccounts/' in parsed.path) or '/projects/' + p['id'] + '/' in parsed.path or '/projects/' + p['number'] + '/' in parsed.path or parsed.path.endswith('/projects/' + p['id'] + ':setIamPolicy') or parsed.path.endswith('/projects/' + p['id'] + ':getIamPolicy'), 'URL does not target selected project')
        if spec['expected'] == 'deny':
            require(all(spec.get(key) for key in ('permission', 'resource', 'denial_reason')), 'exact denial boundary required')
        if 'resource' in spec:
            require(len(spec['resource']) <= 512 and re.fullmatch(r'(//[a-z.]+/)?projects/[a-z0-9-]+(?:/[A-Za-z0-9@._:/-]+)?', spec['resource']), 'invalid evidence resource')
        if 'permission' in spec:
            require(re.fullmatch(r'[a-z][A-Za-z0-9.]+', spec['permission']), 'invalid evidence permission')
        if 'denial_reason' in spec:
            require(spec['denial_reason'] == 'IAM_PERMISSION_DENIED', 'unsupported denial boundary')
    current_results = []
    for spec in specs:
        project = m['projects'][spec['project']]['id']
        started = stamp()
        token = command(['gcloud', 'auth', 'print-access-token', '--impersonate-service-account=' + spec['identity'], '--project=' + project, '--quiet']).decode().strip()
        require(token and '\n' not in token, 'invalid token response')
        status, body = request(spec, token, project)
        results.append(dict(id=spec['id'], assumption=spec['assumption'], identity=spec['identity'],
            run_id=m['run_id'], project=project, expected=spec['expected'], http_status=status,
            target=spec.get('resource', urlsplit(spec['url']).netloc + urlsplit(spec['url']).path),
            method=spec['method'], permission=spec.get('permission'), denial_reason=spec.get('denial_reason'),
            request_sha256=hashlib.sha256(json.dumps(spec, sort_keys=True, separators=(',', ':')).encode()).hexdigest(),
            result=classify(status, body, spec), started_at=started, finished_at=stamp(), source_sha=m['source_sha']))
        save(path.parent / 'probe-results.json', results)
        current_results.append(results[-1])
        if results[-1]['result'] == 'failed':
            break
    return {'status': 'probes_recorded', 'passed': sum(r['result'] == 'passed' for r in current_results), 'total': len(current_results), 'all_passed': all(r['result'] == 'passed' for r in current_results), 'acceptance': 'requires_matrix_review'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='phase', required=True)
    prepare_parser = sub.add_parser('prepare')
    for key in ('directory', 'source-sha', 'billing-account', 'parent', 'app-project', 'byok-project'):
        prepare_parser.add_argument('--' + key, required=True)
    prepare_parser.add_argument('--broker-digest')
    for phase in ('create', 'plan', 'probe', 'cleanup', 'set-broker-image'):
        child = sub.add_parser(phase)
        child.add_argument('--manifest', required=True)
        if phase == 'plan':
            child.add_argument('--alert-email')
            child.add_argument('--phase', dest='plan_phase', choices=('all', 'bootstrap', 'environments'), default='all')
        if phase == 'set-broker-image':
            child.add_argument('--source-sha', required=True)
            child.add_argument('--digest', required=True)
        if phase == 'probe':
            child.add_argument('--requests', required=True)
    args = parser.parse_args()
    try:
        if args.phase == 'prepare':
            result = prepare(args)
        else:
            path = Path(args.manifest).resolve()
            with open(path.with_suffix('.lock'), 'w') as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                m = json.loads(path.read_text())
                validate(m, require_image=args.phase == 'probe')
                result = {'create': create, 'cleanup': cleanup}.get(args.phase, lambda m, p: globals()[args.phase.replace('-', '_')](m, p, args))(m, path)
        print(json.dumps(result))
        if args.phase == 'probe' and not result['all_passed']:
            return 1
    except (Refused, ValueError, KeyError, TypeError, AttributeError, OSError):
        # No reflected exception data, request bodies or provider errors.
        print(json.dumps({'status': 'blocked', 'detail': 'phase incomplete; inspect manifest lifecycle and prerequisites; raw diagnostics suppressed'}))
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
