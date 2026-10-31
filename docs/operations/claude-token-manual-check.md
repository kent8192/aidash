# Manually check a Claude Code OAuth token

This checks the **official CLI only**, not an Aidash Run. The current Aidash backend
supports a personal saved login on unmanaged Linux; it does not support
`CLAUDE_CODE_OAUTH_TOKEN` or macOS serving. A nonempty token in its env file or
process environment now fails with `claude_oauth_token_backend_unsupported` rather
than silently checking a different saved login. Do not put the token in root `.env`.

## Prepare the token yourself

Use an unmodified official CLI. The installed version inspected for this branch is
2.1.302. Check it and create the token in your own terminal:

```sh
claude --version
claude setup-token
```

The official command opens browser authorization and prints the token to your
terminal. It does not save it. Keep it private; never paste it into chat, an Issue,
a PR, a shell command or a tracked file. This is the token for
`CLAUDE_CODE_OAUTH_TOKEN`, not `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, a browser
cookie, or a Codex token. An eligible Claude subscription is required; availability
of a particular model is not guaranteed.

## Explicit local CLI test (macOS or Linux)

Run this in a terminal with Python 3. The token prompt does not echo, and no secret
is put into command arguments or a file. Only the selected token and basic OS
settings enter the Claude process. `CLAUDE_CONFIG_DIR`, when set, retains its
official meaning. Existing managed policies still apply; this manual CLI check
is not proof that the CLI meets Aidash's isolation requirements.

The script first checks the reported authentication method. This check alone does
not prove that the token is valid. A real model request happens only after you enter
a full model ID you can use and type `RUN`. It consumes that account's allowance.
The model must return `OK`; this still does not test Aidash's tools or journal.

```sh
python3 - <<'PY'
import getpass
import json
import os
import subprocess
import sys
import tempfile

# The program comes from stdin; interactive answers come from the terminal.
sys.stdin = open('/dev/tty', encoding='utf-8')
environment = {key: os.environ[key] for key in (
    'HOME', 'PATH', 'USER', 'LOGNAME', 'LANG', 'TMPDIR', 'CLAUDE_CONFIG_DIR'
) if key in os.environ}
token = getpass.getpass('Claude Code OAuth token (hidden): ')
if not token or token != token.strip():
    raise SystemExit('A nonempty token without surrounding whitespace is required')
environment['CLAUDE_CODE_OAUTH_TOKEN'] = token
with tempfile.TemporaryDirectory(prefix='aidash-cli-manual-') as cwd:
    common = ['claude', '--safe-mode', '--restricted', '--setting-sources', '',
              '--settings', '{"disableAllHooks":true,"autoMemoryEnabled":false}']
    status = subprocess.run(common + ['auth', 'status'], env=environment, cwd=cwd,
                            capture_output=True, text=True, timeout=30)
    if status.returncode:
        raise SystemExit('CLI auth status failed; raw output was not printed')
    try:
        account = json.loads(status.stdout)
    except ValueError:
        raise SystemExit('CLI auth status was not valid JSON') from None
    if account.get('loggedIn') is not True or account.get('authMethod') != 'oauth_token':
        raise SystemExit('The CLI did not report the selected OAuth token method')
    print('Method: oauth_token. Token validity has not yet been tested.')
    model = input('Full model ID available to your account: ').strip()
    if not model or input('Type RUN to make a short real model request: ') != 'RUN':
        raise SystemExit('No model request made')
    result = subprocess.run(common + [
        '--print', '--tools', '', '--disallowedTools', '*',
        '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}',
        '--disable-slash-commands', '--permission-mode', 'dontAsk',
        '--permission-prompts', 'none', '--no-chrome', '--no-session-persistence',
        '--model', model, '--output-format', 'json',
    ], input='Reply with exactly OK, without tools.', env=environment, cwd=cwd,
        capture_output=True, text=True, timeout=120)
    if result.returncode:
        raise SystemExit('CLI model request failed; raw output was not printed')
    try:
        answer = json.loads(result.stdout)
    except ValueError:
        raise SystemExit('CLI result was not valid JSON') from None
    if (answer.get('subtype') != 'success' or answer.get('is_error') is not False
            or answer.get('result', '').strip() != 'OK'):
        raise SystemExit('CLI did not produce the expected successful OK result')
    print('Real CLI token/model response: OK. Aidash integration: NOT tested.')
PY
```

No actual token or live response was available when writing this guide. A token
that expires must be replaced through the official command and the process
restarted; this branch does not implement token extraction or OAuth refresh.

## What remains for Aidash

Passing the CLI test does not enable token mode in `cli-backend`. That needs a
separate verified authentication profile, token-account eligibility, isolation of
managed hooks/customizations, and HTTP → CLI → Aidash acceptance. The official CLI
can fetch server-managed settings using an environment OAuth token, so a cached
personal login cannot be used as proof of the token's organization or policy.
Do not bypass the existing host/account checks or claim that a successful
`auth status` proves those controls. See [the backend guide](cli-backends.md) for
the existing saved-login candidate and its limitations.

Sources checked on 2026-10-31:

- [Authentication and setup-token](https://code.claude.com/docs/en/authentication#generate-a-long-lived-token)
- [CLAUDE_CODE_OAUTH_TOKEN](https://code.claude.com/docs/en/env-vars)
- [Server-managed credential eligibility](https://code.claude.com/docs/en/server-managed-settings#platform-availability)
