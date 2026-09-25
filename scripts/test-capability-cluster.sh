#!/usr/bin/env bash
# Rebuild and verify the real isolation boundary in an owned disposable cluster.
set -euo pipefail
cd "$(dirname "$0")/.."
cluster="aidash-core-ci-$(date +%s)-$$"
state="$PWD/.ignore/core-runtime/$cluster"
tools_dir="$state/tools"
evidence="$state/evidence"
mkdir -p "$tools_dir" "$evidence"
export PATH="$tools_dir:$PATH"
runner_pid=
cleanup() {
  command_status=$?
  trap - EXIT
  set +e
  python3 scripts/core-capability-evidence.py finish "$evidence" --exit-code "$command_status"
  evidence_status=$?
  if [[ -f "$state/private/runner.pid" ]]; then runner_pid=$(cat "$state/private/runner.pid"); fi
  if [[ -n "$runner_pid" ]]; then kill "$runner_pid" 2>/dev/null || true; fi
  if [[ -f "$state/private/owner.json" ]]; then kind delete cluster --name "$cluster"; fi
  if [[ "$command_status" -ne 0 || "$evidence_status" -ne 0 ]]; then exit 1; fi
}
trap cleanup EXIT
python3 scripts/core-capability-evidence.py capture "$evidence"
python3 - "$tools_dir" <<'PY'
import hashlib, io, pathlib, platform, sys, tarfile, urllib.request
root=pathlib.Path(sys.argv[1]);osname=platform.system().lower()
arch={'aarch64':'arm64','arm64':'arm64','x86_64':'amd64'}[platform.machine()]
for name,url,checksum in [
    ('kind',f'https://kind.sigs.k8s.io/dl/v0.30.0/kind-{osname}-{arch}', '.sha256sum'),
    ('kubectl',f'https://dl.k8s.io/release/v1.34.0/bin/{osname}/{arch}/kubectl','.sha256'),
    ('helm',f'https://get.helm.sh/helm-v4.1.4-{osname}-{arch}.tar.gz','.sha256sum'),
]:
    data=urllib.request.urlopen(url,timeout=120).read()
    expected=urllib.request.urlopen(url+checksum,timeout=60).read().decode().split()[0]
    if hashlib.sha256(data).hexdigest()!=expected: raise SystemExit('tool checksum mismatch: '+name)
    if name=='helm':
        with tarfile.open(fileobj=io.BytesIO(data),mode='r:gz') as archive:
            data=archive.extractfile(f'{osname}-{arch}/helm').read()
    path=root/name;path.write_bytes(data);path.chmod(0o700)
PY
runner_port=$(python3 - <<'PORT'
import socket
with socket.socket() as listener:
    listener.bind(('127.0.0.1',0))
    print(listener.getsockname()[1])
PORT
)
python3 scripts/setup-capability-runtime.py --name "$cluster" --directory "$state/private" --port "$runner_port" > "$evidence/setup.log" 2>&1
python3 scripts/start-capability-runner.py "$state/private" > "$evidence/runner.log" 2>&1 &
runner_pid=$!
printf '%s\n' "$runner_pid" > "$state/private/runner.pid"
export AIDASH_CAPABILITY_PROFILE="$state/private/profile.json"
export AIDASH_RUNNER_TOKEN_FILE="$state/private/token"
# Admission runs actual isolation/freeze/resource probes before opening HTTP.
python3 - "$runner_pid" "$evidence/runtime-admission.json" <<'PY'
import json, os, pathlib, sys, time, urllib.error, urllib.request
profile=json.loads(pathlib.Path(os.environ['AIDASH_CAPABILITY_PROFILE']).read_text())
runner=profile['runner'];token=pathlib.Path(os.environ['AIDASH_RUNNER_TOKEN_FILE']).read_text().strip()
client=urllib.request.build_opener(urllib.request.ProxyHandler({}))
request=urllib.request.Request(runner['endpoint']+'/v1/health',headers={'Authorization':'Bearer '+token})
for _ in range(180):
    os.kill(int(sys.argv[1]),0)
    try:
        with client.open(request,timeout=5) as response: health=json.load(response)
        if not health.get('verified') or not health.get('python_verified'): raise SystemExit('isolation admission failed')
        pathlib.Path(sys.argv[2]).write_text(json.dumps(health,indent=2)+'\n')
        break
    except (urllib.error.URLError, TimeoutError): time.sleep(1)
else: raise SystemExit('runner admission did not become ready')
PY
python3 scripts/test-capability-recovery.py "$state/private" "$evidence" "$runner_pid" > "$evidence/recovery.log" 2>&1
# Keep each harness result line intact for the evidence parser, then include
# captured measurements and fault diagnostics in the successful-test output.
RUST_TEST_THREADS=1 scripts/test-capability-runtime.sh -- --show-output > "$evidence/runtime.log" 2>&1
