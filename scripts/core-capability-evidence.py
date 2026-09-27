#!/usr/bin/env python3
"""Record revision-bound evidence; missing or failed assertions cannot pass."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import platform
import re
import subprocess

ROOT = Path(__file__).resolve().parent.parent
MAPPING = ROOT / 'docs/operations/core-capabilities-evidence.json'


def git(*arguments):
    return subprocess.check_output(['git', *arguments], cwd=ROOT)


def revision():
    digest = hashlib.sha256()
    paths = set(git('ls-files', '-z', '--cached', '--others', '--exclude-standard').split(b'\0'))
    for name in sorted(paths - {b''}):
        path = ROOT / name.decode()
        digest.update(name + b'\0')
        digest.update(hashlib.sha256(path.read_bytes()).digest() if path.is_file() else b'deleted')
    return {
        'commit': git('rev-parse', 'HEAD').decode().strip(),
        'dirty': bool(git('status', '--porcelain')),
        'source_sha256': digest.hexdigest(),
        'os': platform.system(),
        'architecture': platform.machine(),
    }


def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + '\n')


def capture(directory):
    directory.mkdir(parents=True, exist_ok=True)
    write(directory / 'source.json', dict(revision(), started_at=datetime.datetime.now(datetime.timezone.utc).isoformat()))


def finish(directory, exit_code):
    mapping = json.loads(MAPPING.read_text())
    errors = []
    source = json.loads((directory / 'source.json').read_text()) if (directory / 'source.json').exists() else {}
    if source.get('source_sha256') != revision()['source_sha256']:
        errors.append('Source changed while the gate ran, or capture is missing.')
    admission = json.loads((directory / 'runtime-admission.json').read_text()) if (directory / 'runtime-admission.json').exists() else {}
    if not admission.get('verified') or not admission.get('python_verified'):
        errors.append('Real isolation admission is missing or failed.')
    recovery = json.loads((directory / 'runner-recovery.json').read_text()) if (directory / 'runner-recovery.json').exists() else {}
    if not recovery.get('passed'):
        errors.append('Real controller crash/recovery evidence is missing or failed.')
    if exit_code:
        errors.append(f'Gate command exited with status {exit_code}.')
    raw = (directory / 'runtime.log').read_text() if (directory / 'runtime.log').exists() else ''
    log = re.sub(r'\x1b\[[0-9;]*m', '', raw)
    outcomes = dict(re.findall(r'^test (\S+) \.\.\. (ok|FAILED|ignored)$', log, re.M))
    results = {}
    for case, entry in mapping['acceptance'].items():
        assertions = []
        for test in entry['tests']:
            matches = {name: value for name, value in outcomes.items() if name == test['name'] or name.startswith(test['name'] + '::')}
            passed = bool(matches) and all(value == 'ok' for value in matches.values())
            assertions.append(dict(test, passed=passed, observed=matches))
            if not passed:
                errors.append(f'{case}: missing or failed {test["name"]}')
        results[case] = {'passed': all(test['passed'] for test in assertions), 'assertions': assertions}
    report = {
        'schema': 'aidash-core-result/1', 'source': source, 'runtime': admission, 'controller_recovery': recovery,
        'command': mapping['commands']['runtime'], 'exit_code': exit_code,
        'requirements': {key: {'acceptance': entry['acceptance'], 'passed': all(results[case]['passed'] for case in entry['acceptance'])} for key, entry in mapping['requirements'].items()},
        'acceptance': results, 'test_results': outcomes, 'errors': errors,
        'passed': not errors, 'finished_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    }
    write(directory / 'result.json', report)
    print(f'Core evidence: {len(outcomes)} assertions; {len(errors)} failures. {directory / "result.json"}')
    return 0 if not errors else 1


def browser(directory):
    mapping = json.loads(MAPPING.read_text())
    report = json.loads((directory / 'browser.json').read_text())
    observed = {}

    def visit(suite):
        for spec in suite.get('specs', []):
            observed[spec['title']] = spec['ok'] and bool(spec['tests']) and all(test['status'] == 'expected' and test['results'] and all(result['status'] == 'passed' for result in test['results']) for test in spec['tests'])
        for child in suite.get('suites', []):
            visit(child)

    visit(report)
    expected = {test['name']: observed.get(test['name'], False) for test in mapping['browser_tests']}
    passed = all(expected.values()) and not report.get('errors') and not report.get('stats', {}).get('unexpected', 0)
    write(directory / 'browser-result.json', {'schema': 'aidash-core-browser-result/1', 'source': revision(), 'assertions': expected, 'passed': passed})
    print(f'Browser evidence: {len(expected)} required journeys; passed={passed}')
    return 0 if passed else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['capture', 'finish', 'browser'])
    parser.add_argument('directory', type=Path)
    parser.add_argument('--exit-code', type=int, default=0)
    arguments = parser.parse_args()
    if arguments.action == 'capture':
        capture(arguments.directory)
        return 0
    if arguments.action == 'browser':
        return browser(arguments.directory)
    return finish(arguments.directory, arguments.exit_code)


if __name__ == '__main__':
    raise SystemExit(main())
