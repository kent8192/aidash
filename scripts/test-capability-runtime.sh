#!/usr/bin/env bash
# Real execution is opt-in. A missing isolated runner must fail this gate.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${AIDASH_CAPABILITY_PROFILE:?Set AIDASH_CAPABILITY_PROFILE to the verified isolated execution profile.}"
export AIDASH_SECRET_TEST_PEER=local-peer-regression-test-token-0123456789
export AIDASH_SECRET_TEST_QDRANT=local-semantic-vector-fixture-key-0123456789
export RUSTC_WRAPPER=
# Each case owns up to two independent twelve-connection pools.
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-4}"
exec python3 - "$@" <<'PY'
import json, os, urllib.request, sys
profile = json.load(open(os.environ['AIDASH_CAPABILITY_PROFILE']))
runner = profile.get('runner')
if not runner or not profile.get('admission'):
    raise SystemExit('The runtime test gate requires an enabled, isolated runner profile.')
if os.environ.get('AIDASH_RUNNER_TOKEN_FILE'):
    os.environ[runner['token_env']] = open(os.environ['AIDASH_RUNNER_TOKEN_FILE']).read().strip()
token = os.environ.get(runner['token_env'])
if not token:
    raise SystemExit('Set the runner token environment variable or AIDASH_RUNNER_TOKEN_FILE; do not put a token in the profile.')
request = urllib.request.Request(runner['endpoint'].rstrip('/') + '/v1/health', headers={'Authorization': 'Bearer ' + token})
health = json.load(urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=10))
if not health.get('verified') or not health.get('python_verified') or health.get('image') != runner['image']:
    raise SystemExit('The runner has not passed isolation and physical writer-freeze admission.')
print(json.dumps({key:health.get(key) for key in ['protocol','image','instance','verified','python_verified']}, sort_keys=True), flush=True)
os.execvp('cargo', ['cargo','test','--locked','--features','capability-runtime-tests','--lib','--test','core_capabilities','--test','scoped_remote_execution','--test','migrations',*sys.argv[1:]])
PY
