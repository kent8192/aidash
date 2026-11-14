import json
import threading
from types import SimpleNamespace

import pytest

from control import Runner
from execution_pod import ExecutionPodController, v2_network_policy, verify_network_policies


def legacy_policy(component='preview', port=3000):
    return {'metadata': {'name': 'demo-preview-sandbox', 'namespace': 'sandbox'}, 'spec': {
        'podSelector': {'matchLabels': {'aidash-sandbox': 'true'}, 'matchExpressions': [
            {'key': 'aidash-v2', 'operator': 'DoesNotExist'},
            {'key': 'aidash-journal', 'operator': 'DoesNotExist'}]},
        'policyTypes': ['Ingress'], 'ingress': [{'from': [{
            'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'app'}},
            'podSelector': {'matchLabels': {'app.kubernetes.io/instance': 'demo',
                                             'app.kubernetes.io/component': component}}}],
            'ports': [{'protocol': 'TCP', 'port': port}]}]}}


@pytest.mark.parametrize('component', ['preview', 'server'])
def test_runner_uses_existing_preview_gateway_identity_and_single_port(component):
    config = {'namespace': 'sandbox', 'proxy_endpoint': 'http://10.0.0.8:8888',
              'proxy_namespace': 'app', 'proxy_selector': {'app': 'proxy'}}
    calls = []
    runner = object.__new__(Runner)
    runner.lock = threading.RLock()
    runner.config = {'namespace': 'sandbox'}
    runner.execution_pods = SimpleNamespace(config=config)
    def kube(args, data=None, **_):
        if args[0] == 'get':
            return SimpleNamespace(stdout=json.dumps({'items': [legacy_policy(component)]}).encode())
        calls.append(json.loads(data))
    runner.kube = kube
    runner.ensure_v2_network_policy()
    rule = calls[0]['spec']['ingress']
    assert rule == legacy_policy(component)['spec']['ingress']
    assert config['preview_port'] == 3000
    labels = {'aidash-sandbox': 'true', 'aidash-v2': 'true'}
    verify_network_policies(calls, labels, calls[0])
    unauthorized = json.loads(json.dumps(calls[0]))
    unauthorized['spec']['ingress'][0]['from'][0]['podSelector']['matchLabels']['app.kubernetes.io/component'] = 'other'
    with pytest.raises(ValueError, match='ingress'):
        verify_network_policies(calls + [unauthorized], labels, calls[0])
    unauthorized_port = json.loads(json.dumps(calls[0]))
    unauthorized_port['spec']['ingress'][0]['ports'][0]['port'] = 3001
    with pytest.raises(ValueError, match='ingress'):
        verify_network_policies(calls + [unauthorized_port], labels, calls[0])
    absent = json.loads(json.dumps(calls[0]))
    absent['spec']['ingress'] = []
    with pytest.raises(ValueError, match='ingress'):
        verify_network_policies([absent], labels, calls[0])


def test_preview_rejects_other_ports_before_launch(tmp_path):
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path), {'preview_port': 3000}, admitted=True)
    with pytest.raises(ValueError, match='preview port'):
        controller.start_preview({'operation_id': '00000000-0000-4000-8000-000000000001',
                                  'scratch_space_id': 's', 'run_id': 'r', 'code': 'true', 'port': 3001})


def test_preview_rejects_other_port_over_http_as_invalid_request(tmp_path):
    import http.client
    from http.server import ThreadingHTTPServer
    from control import Handler
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path), {'preview_port': 3000}, admitted=True)
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.runner = SimpleNamespace(token='x' * 32, execution_pods=controller)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        connection = http.client.HTTPConnection('127.0.0.1', server.server_port)
        request = {'operation_id': '00000000-0000-4000-8000-000000000001', 'scratch_space_id': 's',
                   'run_id': 'r', 'code': 'true', 'port': 3001}
        connection.request('POST', '/v2/previews/start', json.dumps(request), {'Authorization': 'Bearer ' + 'x' * 32})
        response = connection.getresponse()
        assert response.status == 400
        assert json.loads(response.read())['code'] == 'invalid_request'
        connection.close()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_removing_legacy_preview_configuration_closes_ingress():
    config = {'namespace': 'sandbox', 'proxy_endpoint': 'http://10.0.0.8:8888',
              'proxy_namespace': 'app', 'proxy_selector': {'app': 'proxy'},
              'preview_port': 3000, 'preview_namespace': 'app',
              'preview_selector': {'app.kubernetes.io/instance': 'demo', 'app.kubernetes.io/component': 'preview'}}
    runner = object.__new__(Runner)
    runner.lock = threading.RLock()
    runner.config = {'namespace': 'sandbox'}
    runner.execution_pods = SimpleNamespace(config=config)
    applied = []
    def kube(args, data=None, **_):
        if args[0] == 'get':
            return SimpleNamespace(stdout=b'{"items":[]}')
        applied.append(json.loads(data))
    runner.kube = kube
    runner.ensure_v2_network_policy()
    assert applied[0]['spec']['ingress'] == []
    assert 'preview_port' not in config


def test_permissive_legacy_policy_cannot_define_a_gateway():
    from execution_pod import preview_ingress_from_legacy
    policy = legacy_policy()
    policy['spec']['ingress'][0]['from'] = [{}]
    with pytest.raises(ValueError):
        preview_ingress_from_legacy([policy])
