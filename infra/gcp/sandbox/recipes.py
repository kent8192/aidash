#!/usr/bin/env python3
"""Generate reviewable dummy-resource requests; never executes cloud operations.

Resources JSON: server, worker, runtime, control service-account emails; own_tenant,
other_tenant, dummy_user; optional fresh byok_policy (with etag). All are sandbox
outputs/fixtures. Generated files are independent probes, not a batch transaction.
"""
import argparse
import copy
import json
from pathlib import Path
import sys

from runner import Refused, require, save, validate


def build(m, r):
    app, byok = (m['projects'][key]['id'] for key in ('app', 'byok'))
    number = m['projects']['byok']['number']
    broker = 'aidash-test-broker@' + app + '.iam.gserviceaccount.com'
    deploy = 'aidash-deploy@' + app + '.iam.gserviceaccount.com'
    cases = []

    def add(identifier, assumption, role, identity, method, url, expected, *, body=None, permission=None, resource=None, assertions=None):
        case = dict(id=identifier, assumption=assumption, project=role, identity=identity,
                    method=method, url=url, expected=expected)
        if body is not None:
            case['body'] = body
        if expected == 'deny':
            case.update(permission=permission, resource=resource, denial_reason='IAM_PERMISSION_DENIED')
        if assertions:
            case['assert'] = assertions
        cases.append(case)

    for identity_name in ('server', 'worker', 'runtime', 'control'):
        require(r[identity_name].endswith('@' + app + '.iam.gserviceaccount.com'), 'fixture identity outside app project')
    own = f'projects/{number}/secrets/aidash-test-cred-dummy'
    other = f'projects/{number}/secrets/aidash-other-cred-dummy'
    for suffix, resource in [('own', own), ('other', other)]:
        add('prefix-server-' + suffix, 2, 'byok', r['server'], 'GET',
            'https://secretmanager.googleapis.com/v1/' + resource,
            'allow' if suffix == 'own' else 'deny', permission='secretmanager.secrets.get', resource=resource,
            assertions={'name': resource} if suffix == 'own' else None)
        version = resource + '/versions/1'
        for name, identity in [('broker', broker), ('server', r['server']), ('worker', r['worker']), ('runtime', r['runtime']), ('deploy', deploy)]:
            allowed = name == 'broker' and suffix == 'own'
            add('payload-' + name + '-' + suffix, 3, 'byok', identity, 'GET',
                'https://secretmanager.googleapis.com/v1/' + version + ':access',
                'allow' if allowed else 'deny', permission='secretmanager.versions.access', resource=version,
                assertions={'name': version, 'payload.data': 'YWlkYXNoLXNhbmRib3gtZHVtbXk='} if allowed else None)
    key = f'projects/{app}/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability/cryptoKeyVersions/1'
    for name, identity in [('worker', r['worker']), ('server', r['server'])]:
        add('kms-sign-' + name, 5, 'app', identity, 'POST', 'https://cloudkms.googleapis.com/v1/' + key + ':asymmetricSign',
            'allow' if name == 'worker' else 'deny', body={'data': 'YWlkYXNoLXNhbmRib3gtZHVtbXk='},
            permission='cloudkms.cryptoKeyVersions.useToSign', resource=key)
    for name, tenant in [('own', r['own_tenant']), ('other', r['other_tenant']), ('project', None)]:
        resource = f'projects/{app}' + ('/tenants/' + tenant if tenant else '')
        add('gcip-lookup-' + name, 6, 'app', r['server'], 'POST',
            'https://identitytoolkit.googleapis.com/v1/' + resource + '/accounts:lookup',
            'allow' if name == 'own' else 'deny', body={'localId': [r['dummy_user']]},
            permission='firebaseauth.users.get', resource=resource)
    account = f'projects/-/serviceAccounts/{broker}'
    iam = 'https://iam.googleapis.com/v1/' + account
    credentials = 'https://iamcredentials.googleapis.com/v1/' + account
    operations = [
        ('set-policy', 'iam.serviceAccounts.setIamPolicy', 'POST', iam + ':setIamPolicy', {'policy': {'version': 3, 'bindings': []}}),
        ('access-token', 'iam.serviceAccounts.getAccessToken', 'POST', credentials + ':generateAccessToken', {'scope': ['https://www.googleapis.com/auth/cloud-platform'], 'lifetime': '600s'}),
        ('sign-blob', 'iam.serviceAccounts.signBlob', 'POST', credentials + ':signBlob', {'payload': 'YWlkYXNoLXNhbmRib3gtZHVtbXk='}),
        ('sign-jwt', 'iam.serviceAccounts.signJwt', 'POST', credentials + ':signJwt', {'payload': '{"sub":"sandbox-dummy"}'}),
        ('create-key', 'iam.serviceAccountKeys.create', 'POST', iam + '/keys', {'privateKeyType': 'TYPE_GOOGLE_CREDENTIALS_FILE'}),
        ('delete', 'iam.serviceAccounts.delete', 'DELETE', iam, None),
        ('undelete', 'iam.serviceAccounts.undelete', 'POST', iam + ':undelete', {}),
        ('disable', 'iam.serviceAccounts.disable', 'POST', iam + ':disable', {}),
        ('id-token', 'iam.serviceAccounts.getOpenIdToken', 'POST', credentials + ':generateIdToken', {'audience': 'https://sandbox.invalid'}),
        ('delegation', 'iam.serviceAccounts.implicitDelegation', 'POST', 'https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/' + r['control'] + ':generateAccessToken', {'delegates': [account], 'scope': ['https://www.googleapis.com/auth/cloud-platform'], 'lifetime': '600s'}),
    ]
    for name, permission, method, url, body in operations:
        add('deploy-deny-' + name, 8, 'app', deploy, method, url, 'deny', body=body, permission=permission, resource=account)
    if 'byok_policy' in r:
        baseline = r['byok_policy']
        require(baseline.get('etag'), 'fresh IAM policy etag required')
        for name, role in [('create', f'projects/{byok}/roles/aidashByokCreate'),
                           ('manage', f'projects/{byok}/roles/aidashByokManage'),
                           ('payload', f'projects/{byok}/roles/aidashByokBrokerRead')]:
            policy = copy.deepcopy(baseline)
            policy.setdefault('bindings', []).append({'role': role, 'members': ['serviceAccount:' + r['control']]})
            add('grant-' + name, 1, 'byok', deploy, 'POST', f'https://cloudresourcemanager.googleapis.com/v1/projects/{byok}:setIamPolicy',
                'deny' if name == 'payload' else 'allow', body={'policy': policy}, permission='resourcemanager.projects.setIamPolicy', resource=f'projects/{byok}')
        policy = copy.deepcopy(baseline)
        policy['auditConfigs'] = []
        add('audit-config-removal', 1, 'byok', deploy, 'POST', f'https://cloudresourcemanager.googleapis.com/v1/projects/{byok}:setIamPolicy',
            'deny', body={'policy': policy, 'updateMask': 'auditConfigs,etag'}, permission='resourcemanager.projects.setIamPolicy', resource=f'projects/{byok}')
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', required=True)
    parser.add_argument('--resources', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    try:
        m = json.loads(Path(args.manifest).read_text())
        validate(m)
        require(m['status'] == 'created', 'sandbox creation required')
        cases = build(m, json.loads(Path(args.resources).read_text()))
        output = Path(args.output)
        output.mkdir(mode=0o700)
        for case in cases:
            save(output / (case['id'] + '.json'), [case])
        print(json.dumps({'requests_prepared': len(cases), 'executed': 0}))
    except (Refused, ValueError, KeyError, TypeError, OSError):
        print(json.dumps({'status': 'blocked', 'detail': 'check owned sandbox resources; raw diagnostics suppressed'}))
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
