#!/usr/bin/env bash
# A missing isolated reader must fail this explicit runtime gate.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${AIDASH_CAPABILITY_PROFILE:?Set the disposable verified reader profile.}"
: "${AIDASH_RUNNER_TOKEN_FILE:?Set its private runner token file.}"
export RUSTC_WRAPPER=
export AIDASH_SECRET_TEST_PEER=local-peer-regression-test-token-0123456789
export AIDASH_SECRET_TEST_QDRANT=local-semantic-vector-fixture-key-0123456789
exec python3 - "$@" <<'PY'
import json, os, sys, urllib.request
profile = json.load(open(os.environ['AIDASH_CAPABILITY_PROFILE']))
runner = profile['runner']
os.environ[runner['token_env']] = open(os.environ['AIDASH_RUNNER_TOKEN_FILE']).read().strip()
request = urllib.request.Request(runner['endpoint']+'/v1/health', headers={'Authorization':'Bearer '+os.environ[runner['token_env']]})
health = json.load(urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=10))
if not health.get('verified') or health.get('image') != runner['image'] or health.get('web_extraction_protocol') != 'aidash-web-extraction/1':
    raise SystemExit('The fixed Web reader is not admitted on the pinned image.')
os.execvp('cargo', ['cargo','test','--locked','--features','capability-runtime-tests','--test','web_research','web_reader_real',*sys.argv[1:]])
PY
