#!/usr/bin/env bash
# Real execution is opt-in. A missing isolated runner must fail this gate.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${AIDASH_CAPABILITY_PROFILE:?Set AIDASH_CAPABILITY_PROFILE to the verified isolated execution profile.}"
export RUSTC_WRAPPER=
scripts/build-test-postgres.sh
# Each case owns up to two independent twelve-connection pools.
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-4}"
# Scoped remote execution polls the full two-node HTTP/worker stack on each
# integration-test thread. Give that deep async call path room beyond Rust's
# 2 MiB default, which overflows on Linux in the lost-reply reconciliation case.
export RUST_MIN_STACK="${RUST_MIN_STACK:-8388608}"
exec python3 - "$@" <<'PY'
import json, os, urllib.request, sys
import pathlib, socket, subprocess, time
enterprise = sys.argv[1:] == ['--enterprise-allocation']
if '--enterprise-allocation' in sys.argv[1:] and not enterprise:
    raise SystemExit('The Enterprise allocation selector accepts no additional test filters.')
profile_path = pathlib.Path(os.environ['AIDASH_CAPABILITY_PROFILE']).resolve()
profile = json.loads(profile_path.read_text())
runner = profile.get('runner')
if not runner or not profile.get('admission'):
    raise SystemExit('The runtime test gate requires an enabled, isolated runner profile.')
if os.environ.get('AIDASH_RUNNER_TOKEN_FILE'):
    os.environ[runner['token_env']] = open(os.environ['AIDASH_RUNNER_TOKEN_FILE']).read().strip()
token = os.environ.get(runner['token_env'])
if not token:
    raise SystemExit('Set the runner token environment variable or AIDASH_RUNNER_TOKEN_FILE; do not put a token in the profile.')
def health_check():
    request = urllib.request.Request(runner['endpoint'].rstrip('/') + '/v1/health', headers={'Authorization': 'Bearer ' + token})
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request, timeout=10) as reply:
        health = json.load(reply)
    if not health.get('verified') or not health.get('python_verified') or health.get('image') != runner['image']:
        raise SystemExit('The runner has not passed isolation and physical writer-freeze admission.')
    return {key:health.get(key) for key in ['protocol','image','instance','verified','python_verified']}

if not enterprise:
    print(json.dumps(health_check(), sort_keys=True), flush=True)
    os.execvp('cargo', ['cargo','test','--locked','--features','capability-runtime-tests','--lib','--test','core_capabilities','--test','scoped_remote_execution','--test','migrations',*sys.argv[1:]])

# This opt-in gate owns the initial controller. Never attach to somebody else's
# listener or reuse a journal from an earlier acceptance attempt.
directory = profile_path.parent
owner = json.loads((directory / 'owner.json').read_text())
evidence = directory / 'allocation-evidence'
if (profile_path != directory / 'profile.json' or profile['idle_seconds'] != 60
        or owner.get('purpose') != 'Aidash capability acceptance'
        or pathlib.Path(owner['source']).resolve() != pathlib.Path.cwd()
        or runner.get('journal') != str(directory / 'journal')
        or runner.get('kubectl') != str(directory / 'observe-kubectl')
        or runner.get('listen_host') != '127.0.0.1'
        or runner['endpoint'] != f"http://127.0.0.1:{runner['listen_port']}"
        or (directory / 'journal').exists() or not evidence.is_dir()):
    raise SystemExit('Enterprise acceptance requires fresh owned setup with --idle-seconds 60 --observe-allocations.')
with socket.socket() as probe:
    if probe.connect_ex(('127.0.0.1', runner['listen_port'])) == 0:
        raise SystemExit('The owned controller port is already in use; refusing to attach.')
gate_path = directory / 'allocation-gate.json'
fd = os.open(gate_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
controller = None
try:
    # stdout/stderr remain private runtime diagnostics, never the evidence packet.
    log_fd = os.open(directory / 'allocation-controller.log', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(log_fd, 'wb') as log:
        controller = subprocess.Popen([sys.executable, 'scripts/start-capability-runner.py', str(directory)],
                                      stdin=subprocess.DEVNULL, stdout=log, stderr=log)
    with os.fdopen(fd, 'w') as state:
        json.dump({'pid': controller.pid, 'profile': str(profile_path), 'source': str(pathlib.Path.cwd()),
                   'python': sys.executable}, state)
        state.flush()
        os.fsync(state.fileno())
    deadline = time.monotonic() + 180
    while True:
        if controller.poll() is not None:
            raise SystemExit('Owned controller exited before admission; inspect its private log.')
        try:
            health = health_check()
            break
        except (OSError, urllib.error.URLError):
            if time.monotonic() >= deadline:
                raise SystemExit('Owned controller admission timed out.') from None
            time.sleep(0.2)
    print(json.dumps(health, sort_keys=True), flush=True)
    os.environ['AIDASH_ALLOCATION_GATE_OWNER'] = str(gate_path)
    expected = 'real_allocation::actual_allocation_receipts_reach_frozen_usage_before_independent_ack'
    command = ['cargo', 'test', '--locked', '--manifest-path', 'enterprise/Cargo.toml',
               '-p', 'aidash-enterprise-server', '--test', 'billing', expected]
    inventory = subprocess.run([*command, '--', '--ignored', '--exact', '--list'], check=True,
                               stdout=subprocess.PIPE, text=True)
    found = [line.removesuffix(': test') for line in inventory.stdout.splitlines() if line.endswith(': test')]
    if found != [expected]:
        raise SystemExit('Enterprise allocation test inventory must contain exactly its one expected owner.')
    # Do not poll/reap controller while Rust owns its stop boundary: its PID
    # cannot be recycled while this parent retains the terminated child.
    result = subprocess.run([*command, '--', '--ignored', '--exact', '--nocapture', '--test-threads=1'])
    raise SystemExit(result.returncode)
finally:
    if controller is not None:
        if controller.poll() is None:
            controller.terminate()
        try:
            controller.wait(timeout=20)
        except subprocess.TimeoutExpired:
            controller.kill()
            controller.wait(timeout=10)
PY
