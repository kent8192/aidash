#!/usr/bin/env python3
"""Freeze the approved twenty-query protocol without making provider requests.

An account-free manifest is a rehearsal. Before a real pilot, create a new
manifest with the verified account profile; the running product checks its
eligibility and credentials independently. Existing manifests are never edited.
"""
import argparse
from datetime import date, datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--date', type=date.fromisoformat, default=datetime.now(timezone.utc).date())
    parser.add_argument('--operator', required=True)
    parser.add_argument('--account-profile', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    protocol = root / 'docs/design/2026-09-25-web-search-evaluation.md'
    queries = []
    for line in protocol.read_text().splitlines():
        cells = [cell.strip() for cell in line.split('|')]
        if len(cells) != 6 or not cells[1][:2].isdigit():
            continue
        pair, category = cells[1].split(maxsplit=1)
        for language, country, text in [('ja', 'JP', cells[2]), ('en', 'US', cells[3])]:
            arguments = dict(query=text.replace('EVALUATION_DATE', args.date.isoformat()),
                language=language, country=country, count=5, page=0)
            if pair == '10':
                arguments['freshness'] = dict(from_=(args.date - timedelta(days=29)).isoformat(), to=args.date.isoformat())
                arguments['freshness']['from'] = arguments['freshness'].pop('from_')
            queries.append(dict(id=language.upper()+pair, category=category,
                arguments=arguments, source_criteria=cells[4]))
    if len(queries) != 20 or len({query['id'] for query in queries}) != 20:
        raise SystemExit('The approved protocol must contain exactly ten pairs.')
    account = None
    if args.account_profile:
        profile = json.loads(args.account_profile.read_bytes())
        if not re.fullmatch(r'AIDASH_SECRET_[A-Z0-9_]*', profile.get('credential_env', '')):
            raise SystemExit('The account must contain a credential environment reference, never key bytes.')
        # Include only explicit nonsecret references, never arbitrary fields
        # from an operator-provided file or the referenced credential's value.
        account = {key: profile[key] for key in ['account_id', 'agreement_reference',
            'credential_env', 'request_price_micro_usd', 'monthly_base_fee_micro_usd',
            'monthly_limit_micro_usd']}
        account['profile_sha256'] = hashlib.sha256(args.account_profile.read_bytes()).hexdigest()
    sources = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard'], cwd=root, text=True).splitlines()
    source_digest = hashlib.sha256()
    for name in sorted(set(sources)):
        if name in ('Cargo.toml', 'Cargo.lock') or name.startswith(('src/', 'migration/src/', 'runner/', 'web/src/', 'scripts/')):
            path = root / name
            if path.is_file():
                source_digest.update(name.encode()+b'\0'+hashlib.sha256(path.read_bytes()).digest())
    value = dict(protocol='aidash-web-search-pilot/1', frozen_at=datetime.now(timezone.utc).isoformat(),
        evaluation_date=args.date.isoformat(), operator=args.operator,
        base_commit=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
        source_sha256=source_digest.hexdigest(), protocol_sha256=hashlib.sha256(protocol.read_bytes()).hexdigest(),
        account=account, eligibility='MUST_VERIFY_IN_PRODUCT', status='NOT_RUN', provider_calls=0,
        search_attempt_limit=10, page_attempt_limit=20, node_monthly_limit_micro_usd=20_000_000,
        queries=queries)
    # O_EXCL prevents overwriting prior failures or re-freezing a measured run.
    with args.output.open('x') as output:
        json.dump(value, output, ensure_ascii=False, indent=2)
        output.write('\n')
    print(f'Frozen {len(queries)} queries; provider requests: 0. Manifest: {args.output}')


if __name__ == '__main__':
    main()
