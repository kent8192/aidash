#!/usr/bin/env python3
"""Verify the opt-in execution chart in a newly owned disposable kind cluster."""
import argparse
import importlib.util
import json
from pathlib import Path
import secrets
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('setup', ROOT / 'scripts/setup-capability-runtime.py')
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


def run(args, data=None, capture=False):
    return subprocess.run([str(arg) for arg in args], input=data, check=True,
                          stdout=subprocess.PIPE if capture else None).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--name', required=True)
    parser.add_argument('--directory', type=Path, required=True)
    args = parser.parse_args()
    if not args.name.startswith('aidash-155-'):
        parser.error('owned cluster name must start with aidash-155-')
    if args.name in run(['kind', 'get', 'clusters'], capture=True).decode().split():
        parser.error('refusing to reuse any existing cluster')
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=False, mode=0o700)
    setup.write_private(directory / 'owner.json', json.dumps({'cluster': args.name, 'source': str(ROOT)}))
    kubeconfig = directory / 'kubeconfig'
    node_config = directory / 'kind.yaml'
    setup.write_private(node_config, f'''kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
networking:
  disableDefaultCNI: true
  podSubnet: 10.201.0.0/16
nodes:
- role: control-plane
  image: {setup.NODE}
  kubeadmConfigPatches:
  - |
    kind: KubeletConfiguration
    podPidsLimit: 1024
''')
    kube = ['kubectl', '--kubeconfig', kubeconfig]
    created = False
    forward = None
    try:
        created = True
        run(['kind', 'create', 'cluster', '--name', args.name, '--config', node_config, '--kubeconfig', kubeconfig])
        kubeconfig.chmod(0o600)
        cilium = run(['helm', 'template', 'cilium', 'cilium', '--repo', 'https://helm.cilium.io/',
                      '--version', setup.CILIUM, '-n', 'kube-system', '--set', 'operator.replicas=1',
                      '--set', 'ipam.mode=kubernetes', '--set', 'kubeProxyReplacement=false',
                      '--set', 'securityContext.privileged=true'], capture=True)
        run(kube + ['apply', '-f', '-'], cilium)
        run(kube + ['-n', 'kube-system', 'rollout', 'status', 'daemonset/cilium', '--timeout=240s'])
        cluster_node = args.name + '-control-plane'
        run(kube + ['label', 'node', cluster_node, 'aidash.run/local-test=' + args.name])
        arch = run(['docker', 'exec', cluster_node, 'uname', '-m'], capture=True).decode().strip()
        ctr = run(['docker', 'exec', cluster_node, 'sh', '-c', 'command -v ctr'], capture=True).decode().strip()
        images = {}
        for role, dockerfile in (('control', 'control.Dockerfile'), ('sandbox', 'Dockerfile')):
            tag = f'docker.io/library/aidash155-{role}:{args.name}'
            metadata = directory / (role + '-image.json')
            run(['docker', 'buildx', 'build', '--load', '--provenance=false', '--metadata-file', metadata,
                 '-t', tag, '-f', ROOT / 'runner' / dockerfile, ROOT / 'runner'])
            digest = json.loads(metadata.read_bytes())['containerimage.digest']
            run(['kind', 'load', 'docker-image', '--name', args.name, tag])
            image = tag.split(':')[0] + '@' + digest
            run(['docker', 'exec', cluster_node, 'ctr', '-n', 'k8s.io', 'images', 'tag', tag, image])
            images[role] = image
        namespace = args.name
        run(kube + ['create', 'namespace', namespace])
        token = secrets.token_urlsafe(48)
        secret = {'apiVersion': 'v1', 'kind': 'Secret', 'metadata': {'name': 'runner', 'namespace': namespace},
                  'stringData': {'AIDASH_CORE_RUNNER_TOKEN': token}}
        run(kube + ['create', '-f', '-'], json.dumps(secret).encode())
        values = {'node': {'id': 'aidash://execution-chart-test'}, 'existingSecret': 'runner',
                  'server': {'replicas': 0}, 'worker': {'replicas': 0}, 'frontend': {'replicas': 0},
                  'environment': {'nodeSelector': {'aidash.run/local-test': args.name}},
                  'execution': {'createNamespaces': True, 'runtimeClass': {'create': True},
                                'paths': {'ctr': ctr},
                                'gvisor': {'sha256': setup.HASHES[arch]},
                                'sandboxImage': images['sandbox'],
                                'installer': {'enabled': True, 'image': images['control']},
                                'guard': {'enabled': True, 'image': images['control']},
                                'runner': {'enabled': True, 'image': images['control'], 'existingSecret': 'runner'}}}
        values_path = directory / 'values.json'
        setup.write_private(values_path, json.dumps(values))
        # Helm assigns the release namespace to unqualified application objects
        # while respecting explicit trusted/sandbox namespaces in this chart.
        run(['helm', 'install', args.name, ROOT / 'deploy/helm/aidash', '--kubeconfig', kubeconfig,
             '-n', namespace, '-f', values_path])
        run(kube + ['-n', namespace, 'rollout', 'status', 'deployment/' + args.name + '-execution-runner', '--timeout=600s'])
        # Port zero asks kubectl for an unused local port. Its private log has
        # no bearer token or Kubernetes Secret contents.
        with (directory / 'port-forward.log').open('w') as log:
            forward = subprocess.Popen([str(v) for v in kube + ['-n', namespace, 'port-forward',
                                       'service/' + args.name + '-execution-runner', ':8949']], stdout=log, stderr=log)
            deadline = time.monotonic() + 30
            import re
            while time.monotonic() < deadline:
                match = re.search(r'127.0.0.1:(\d+) ->', (directory / 'port-forward.log').read_text())
                if match:
                    break
                time.sleep(.2)
            else:
                raise RuntimeError('port forward unavailable')
            request = urllib.request.Request(f'http://127.0.0.1:{match[1]}/v1/health',
                                             headers={'Authorization': 'Bearer ' + token})
            with urllib.request.urlopen(request, timeout=15) as response:
                health = json.load(response)
        assert health['verified'] and health['python_verified'], health
        assert health['probe']['resources']['host_tasks'] == 512
        assert health['probe']['resources']['processes'] == 128
        assert health['probe']['guest_capacity_verified'] == 112
        (directory / 'health.json').write_text(json.dumps(health, indent=2) + '\n')
        print('Chart verify_isolation and verify_freeze passed; resource evidence saved in health.json')
    finally:
        if forward:
            forward.terminate()
            forward.wait(timeout=10)
        if created:
            for namespace in (args.name, args.name + '-guard', args.name + '-sandbox'):
                result = subprocess.run([str(v) for v in kube + ['-n', namespace, 'get', 'pods', '-o', 'wide']],
                                        capture_output=True)
                (directory / (namespace + '-pods.log')).write_bytes(result.stdout + result.stderr)
                result = subprocess.run([str(v) for v in kube + ['-n', namespace, 'logs', '-l',
                                        'aidash.run/runner=' + args.name, '--all-containers=true', '--tail=100']],
                                        capture_output=True)
                (directory / (namespace + '-runner.log')).write_bytes(result.stdout + result.stderr)
            for role in ('guard', 'installer'):
                result = subprocess.run([str(v) for v in kube + ['-n', args.name + '-guard', 'logs',
                    'daemonset/' + args.name + '-execution-' + role, '--tail=100']], capture_output=True, check=False)
                (directory / (role + '.log')).write_bytes(result.stdout + result.stderr)
            run(['kind', 'delete', 'cluster', '--name', args.name])


if __name__ == '__main__':
    main()
