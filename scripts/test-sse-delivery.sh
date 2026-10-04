#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/build-test-postgres.sh
export RUSTC_WRAPPER=
export RUST_MIN_STACK="${RUST_MIN_STACK:-8388608}"
export AIDASH_SECRET_TEST_PEER=local-peer-regression-test-token-0123456789
export AIDASH_SECRET_TEST_QDRANT=local-semantic-vector-fixture-key-0123456789
export AIDASH_SSE_EVIDENCE_DIR="${AIDASH_SSE_EVIDENCE_DIR:-$PWD/target/sse-evidence/$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$AIDASH_SSE_EVIDENCE_DIR"
if [[ "${1:-}" != "" && "${1:-}" != "--benchmark" ]]; then
  echo 'usage: scripts/test-sse-delivery.sh [--benchmark]' >&2
  exit 2
fi
export AIDASH_SSE_BENCHMARK="${1:-}"
python3 - <<'PY'
import datetime, hashlib, io, json, os, pathlib, platform, shutil, subprocess, tarfile
root = pathlib.Path(os.environ['AIDASH_SSE_EVIDENCE_DIR']).resolve()
patch = subprocess.check_output(['git','diff','--binary','HEAD'])
files = subprocess.check_output(['git','ls-files','--others','--exclude-standard','-z']).decode().split('\0')
source = root/'untracked-source'
hashes = {}
for name in files:
    path = pathlib.Path(name)
    if not name or not path.is_file() or path.resolve().is_relative_to(root): continue
    hashes[name] = hashlib.sha256(path.read_bytes()).hexdigest()
    destination = source/name
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(path,destination)
(root/'source.patch').write_bytes(patch)
record = {'git_sha':subprocess.check_output(['git','rev-parse','HEAD']).decode().strip(),
          'dirty':bool(subprocess.check_output(['git','status','--porcelain'])),
          'patch_sha256':hashlib.sha256(patch).hexdigest(),'untracked_sha256':hashes,
          'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'platform':platform.platform(),
          'rustc':subprocess.check_output(['rustc','--version']).decode().strip(),
          'settings':{'api_processes':3,'connections':100,'filtered_connections':90,'global_connections':10,'workspaces':10,'active_seconds':30,'events_per_second':10,'active_repetitions':3,'idle_seconds':60,'idle_repetitions':3,'active_reconcile_ms':60000,'idle_reconcile_ms':5000},
          'service_images':['aidash-orm-test-postgres:17-pg-jsonschema-0.3.4','nats:2.12-alpine','qdrant/qdrant:v1.19.1'],
          'resource_limits':'No per-process limits; API processes on host, services in local Docker engine.',
          'timing':'Controller monotonic clock, release of writer event/commit advisory barrier through complete frame parsing; nearest-rank percentiles of slowest eligible client per event.'}
(root/'invocation.json').write_text(json.dumps(record,indent=2)+'\n')
if os.environ['AIDASH_SSE_BENCHMARK']:
    ref = os.environ.get('AIDASH_SSE_BASELINE_REVISION','827480c13d796bca142787be5bbd25ce34c80551')
    sha = subprocess.check_output(['git','rev-parse',ref+'^{commit}']).decode().strip()
    archive = subprocess.check_output(['git','archive',sha])
    baseline = root/'baseline-source'
    baseline.mkdir(exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar: tar.extractall(baseline,filter='data')
    with (root/'baseline-build.jsonl').open('w') as out, (root/'baseline-build.log').open('w') as err:
        subprocess.run(['cargo','build','--locked','--release','--bin','aidash','--message-format=json-render-diagnostics'],cwd=baseline,stdout=out,stderr=err,check=True)
    executables = [json.loads(line).get('executable') for line in (root/'baseline-build.jsonl').read_text().splitlines() if line.startswith('{')]
    binary = next(path for path in reversed(executables) if path)
    saved = root/'baseline-aidash'
    shutil.copy2(binary,saved)
    record['baseline_sha'] = sha
    record['server_build_profile'] = 'release'
    record['driver_build_profile'] = 'debug'
    record['baseline_binary_sha256'] = hashlib.sha256(saved.read_bytes()).hexdigest()
    (root/'invocation.json').write_text(json.dumps(record,indent=2)+'\n')
PY
export AIDASH_SSE_BASELINE_BINARY="$AIDASH_SSE_EVIDENCE_DIR/baseline-aidash"
if [[ "${1:-}" == "--benchmark" ]]; then
  cargo build --locked -p aidash-server --release --bin aidash --message-format=json-render-diagnostics > "$AIDASH_SSE_EVIDENCE_DIR/candidate-release-build.jsonl"
  export AIDASH_SSE_CANDIDATE_BINARY="$AIDASH_SSE_EVIDENCE_DIR/candidate-aidash"
fi
cargo test --locked -p aidash-server --test sse_delivery --no-run --message-format=json-render-diagnostics > "$AIDASH_SSE_EVIDENCE_DIR/candidate-build.jsonl"
python3 - <<'PY'
import hashlib,json,os,pathlib,shutil,subprocess
root=pathlib.Path(os.environ['AIDASH_SSE_EVIDENCE_DIR'])
path=root/'invocation.json'; data=json.loads(path.read_text())
patch=subprocess.check_output(['git','diff','--binary','HEAD'])
assert hashlib.sha256(patch).hexdigest()==data['patch_sha256'], 'tracked source changed during build'
for name,digest in data['untracked_sha256'].items():
    assert hashlib.sha256(pathlib.Path(name).read_bytes()).hexdigest()==digest, 'source changed during build: '+name
build_log = 'candidate-release-build.jsonl' if os.environ['AIDASH_SSE_BENCHMARK'] else 'candidate-build.jsonl'
artifacts=[json.loads(line) for line in (root/build_log).read_text().splitlines() if line.startswith('{')]
binary=next(item['executable'] for item in artifacts if item.get('target',{}).get('name')=='aidash' and item.get('executable'))
data['candidate_binary_sha256']=hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest()
if os.environ['AIDASH_SSE_BENCHMARK']: shutil.copy2(binary,root/'candidate-aidash')
data['commands']=['cargo test --locked -p aidash-server --test sse_delivery -- --nocapture --test-threads=1']
if os.environ['AIDASH_SSE_BENCHMARK']: data['commands'].append('cargo test --locked -p aidash-server --test sse_delivery -- --ignored --nocapture --test-threads=1')
data['image_metadata']=[]
for name in data['service_images']:
    result=subprocess.run(['docker','image','inspect',name,'--format','{{json .Id}} {{json .RepoDigests}} {{json .Architecture}}'],capture_output=True,text=True)
    data['image_metadata'].append({'image':name,'metadata':result.stdout.strip()})
data['docker_resources']=subprocess.check_output(['docker','info','--format','CPUs={{.NCPU}} MemoryBytes={{.MemTotal}}']).decode().strip()
path.write_text(json.dumps(data,indent=2)+'\n')
PY
set +e
cargo test --locked -p aidash-server --test sse_delivery -- --nocapture --test-threads=1 2>&1 | tee "$AIDASH_SSE_EVIDENCE_DIR/functional.log"
sse_status=${PIPESTATUS[0]}
if [[ "$sse_status" == 0 && "${1:-}" == "--benchmark" ]]; then
  cargo test --locked -p aidash-server --test sse_delivery -- --ignored --nocapture --test-threads=1 2>&1 | tee "$AIDASH_SSE_EVIDENCE_DIR/benchmark.log"
  sse_status=${PIPESTATUS[0]}
fi
set -e
python3 - "$sse_status" <<'PY'
import json,os,pathlib,sys
path=pathlib.Path(os.environ['AIDASH_SSE_EVIDENCE_DIR'])/'invocation.json'
data=json.loads(path.read_text());data['exit_code']=int(sys.argv[1]);path.write_text(json.dumps(data,indent=2)+'\n')
PY
exit "$sse_status"
