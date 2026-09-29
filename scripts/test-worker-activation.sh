#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTC_WRAPPER=
export RUST_MIN_STACK="${RUST_MIN_STACK:-8388608}"
export AIDASH_SECRET_TEST_PEER=local-peer-regression-test-token-0123456789
export AIDASH_SECRET_TEST_QDRANT=local-semantic-vector-fixture-key-0123456789
export AIDASH_ACTIVATION_EVIDENCE_DIR="${AIDASH_ACTIVATION_EVIDENCE_DIR:-$PWD/target/activation-evidence}"
mkdir -p "$AIDASH_ACTIVATION_EVIDENCE_DIR"
python3 - <<'PY'
import hashlib,json,os,pathlib,platform,subprocess,datetime
root=pathlib.Path(os.environ['AIDASH_ACTIVATION_EVIDENCE_DIR'])
patch=subprocess.check_output(['git','diff','--binary','HEAD'])
files=subprocess.check_output(['git','ls-files','--others','--exclude-standard','-z']).decode().split('\0')
hashes={p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest() for p in files if p and pathlib.Path(p).is_file()}
(root/'source.patch').write_bytes(patch)
(root/'invocation.json').write_text(json.dumps({'git_sha':subprocess.check_output(['git','rev-parse','HEAD']).decode().strip(),'dirty':bool(subprocess.check_output(['git','status','--porcelain'])),'patch_sha256':hashlib.sha256(patch).hexdigest(),'untracked_sha256':hashes,'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'platform':platform.platform(),'command':'cargo test --locked --test worker_activation -- --nocapture --test-threads=1'},indent=2))
PY
set +e
cargo test --locked --test worker_activation -- --nocapture --test-threads=1 2>&1 | tee "$AIDASH_ACTIVATION_EVIDENCE_DIR/test-output.log"
activation_status=${PIPESTATUS[0]}
set -e
python3 - "$activation_status" <<'PY'
import json,os,pathlib,sys
p=pathlib.Path(os.environ['AIDASH_ACTIVATION_EVIDENCE_DIR'])/'invocation.json'
d=json.loads(p.read_text());d['exit_code']=int(sys.argv[1]);p.write_text(json.dumps(d,indent=2)+'\n')
PY
exit "$activation_status"
