# Personal CLI inference backends (experimental)

This change is an **incomplete, guarded implementation**, not a declaration that all
Claude Code and Codex configurations are supported. Do not deploy it for shared
accounts, Cloud, Dedicated, or Federation. The first candidate route is a personal
Self-hosted Deployment with backend, server and Worker on the **same unmanaged Linux
host**, under the same person. macOS/Windows/WSL and managed Claude accounts are
rejected. No Compose backend integration is supplied.

## Status and architecture

| Surface                                       | Implemented                                                          | Remaining gate                                                         |
| --------------------------------------------- | -------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| Claude saved login                            | Literal env loader, CLI profile, structured result adapter, HTTP/SSE | Live Linux isolation and account smoke, then a real Aidash Run         |
| Codex saved ChatGPT login                     | Separate config and safe child environment                           | All native tools and automatic discovery cannot yet be proven disabled |
| Codex access token / saved access-token login | Separate validated modes; capability check for `--with-access-token` | Same isolation gate; execution deliberately refused                    |
| Codex API key / saved API-key login           | Separate validated modes, no fallback                                | Same isolation gate; execution deliberately refused                    |

Aidash continues to own Runs, journal, approvals, tool dispatch, retry/recovery and
Registry selection. The backend translates an existing Keyless Chat Completions
request to a CLI response envelope. Tool names and JSON arguments are validated
against the request's actual function schemas before returning proposals. No tool
executes in the backend. Each subsequent request carries the conversation, including
Aidash tool results; no resumable CLI session is maintained.

The Registry transport is named `openrouter` in current Aidash even for a custom
Keyless endpoint. Omit both `provider_credential` and `plan_connection`. The endpoint
is local, so no request goes to OpenRouter through this model. The transport's
`provider.zdr` metadata is not a promise by either CLI; the bridge ignores that
provider-specific metadata. CLI data retention, plan limits and prices remain those
of the selected account and service. Embeddings, memory, research, generation and
other Agents do not automatically switch to this backend.

No token is read from an auth file or credential store by Aidash. No OAuth refresh
implementation is added. CLI auth is never converted into the Subject-owned
`plan_connection: "chatgpt"` contract (ADR 0007). This process must never be offered as
a shared gateway to another person's subscription.

## Authentication and isolation decisions

Sources checked on 2026-10-28:

- [Claude CLI flags](https://code.claude.com/docs/en/cli-reference)
- [Claude settings](https://code.claude.com/docs/en/settings-reference#disableallhooks)
- [Claude server-managed settings](https://code.claude.com/docs/en/server-managed-settings)
- [OpenClaw Claude CLI reuse](https://docs.openclaw.ai/gateway/authentication#anthropic%3A-claude-cli-reuse)
- [Codex authentication](https://learn.chatgpt.com/docs/auth)
- [Codex access tokens](https://learn.chatgpt.com/docs/enterprise/access-tokens)
- [Codex non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode)
- [Codex command reference](https://learn.chatgpt.com/docs/cli/reference)
- [Codex configuration](https://learn.chatgpt.com/docs/config-file/config-reference)
- [Codex environment variables](https://learn.chatgpt.com/docs/config-file/environment-variables)

The installed official CLIs examined were Claude Code **2.1.302** and Codex CLI
**0.165.0**, including their help and Codex's generated experimental app-server
schema. Use unmodified official CLIs; an arbitrary executable reporting the same
version is not a trust attestation. The executable and PATH must be controlled by
the deployment owner.

Claude's profile is pinned to 2.1.302. It requires `--safe-mode`, `--restricted`, an
empty `--tools`, `--disallowedTools '*'`, empty strict MCP config, empty setting
sources, disabled slash commands, and denied permission prompts. Input uses
stream-json text blocks rather than shell arguments. User/project instructions,
skills, plugins, memory and hooks are not meant to enter the request. Output is
buffered until the CLI exits successfully with a successful result envelope; native
tool events, refusals, partial output and reported-model changes are rejected.

`--safe-mode` and user `disableAllHooks` **do not disable managed hooks**. Therefore
the candidate profile refuses `/etc/claude-code`, WSL and any saved-login account
whose official status is not personal Pro/Max. CLI login/status is used rather than
credential-file inspection. macOS MDM and Windows policy enforcement have not been
audited and these systems fail closed. An account plan is an isolation eligibility
check, not a promise of model availability. A live test of auto-loaded paths and
customizations on the supported Linux route is still required before release.

Codex exec JSONL and `--output-schema` are the intended protocol. The adapter has a
strict JSONL result parser, but it is **not reachable for production inference**.
Read-only sandbox, an empty cwd, `--ignore-user-config`, and a list of feature flags
do not establish a complete deny-all boundary for built-ins, skills, hooks, MCP,
plugins, web, browser and instructions. The inspected app-server thread-start schema
also lacked an audited complete boundary, so app-server was not substituted.
`doctor` and `serve` return `codex_native_tools_and_discovery_isolation_unverified`.
There is no bypass flag. Fixture parser success does not establish CLI support.

Codex access tokens are official ChatGPT workspace-issued credentials. They are
separate from browser cookies, arbitrary OAuth tokens, Platform API keys and
app-server client transport tokens. The access-token documentation currently names
Business and Enterprise workspaces; local Codex access and the relevant access-token
permission are both required. Follow the token-creation dialog's CLI version
requirement. Account identity comes from the selected official CLI login, not the
chosen mode's name. Do not claim that every account can create these tokens.

For saved modes, let the official CLI retain and refresh its own login:

```sh
claude auth login
claude auth status
codex login
codex login status
# In a secure terminal where the variable is already set (never paste it in chat):
printenv CODEX_ACCESS_TOKEN | codex login --with-access-token
printenv CODEX_API_KEY | codex login --with-api-key
```

Keep `CLAUDE_CONFIG_DIR` / `CODEX_HOME` the same when logging in and starting the
backend. `CODEX_HOME` is not temporary storage. Its credential-store setting remains
owned by Codex; this implementation neither edits that setting nor parses auth.json.
The API-key mode would use API billing only when explicitly selected. Expired,
revoked or unauthorized credentials never trigger a different auth mode. Codex
execution is blocked before these live cases can be exercised; it does not claim
that login success proves token entitlement or remaining quota.

## Env contract

Root `.env` belongs to the existing Aidash deployment/Compose environment. Native
server settings are composed by Reinhardt; merely placing an env file does not load
it into a host process. CLI secrets belong only to a dedicated env passed with
`cli-backend --env-file PATH`. Do not place them in root `.env`, model config,
Provider Credentials or shared containers.

Precedence: non-secret CLI options, process environment, specified env file, defaults.
There is no implicit env search. All settings are read at startup; restart to reload.
An explicitly empty value overrides a lower source and fails if required. A comment
starts a line or follows a space after an unquoted value; quoted values may have a
trailing comment. Single/double quotes are literal delimiters; escape sequences,
backslashes, `$` expansion and backticks are rejected. No `source`, `eval` or command
substitution is used. Unknown/duplicate env keys and unknown `AIDASH_CLI_*` environment
keys fail. Empty secret placeholders are not treated as supplied credentials.

| Variable                       | Type / default                                                                         | Required / consumer                                                         |
| ------------------------------ | -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `AIDASH_CLI_PROVIDER`          | `claude` or `codex`; no default                                                        | Backend                                                                     |
| `AIDASH_CLI_AUTH_MODE`         | `native`, `access-token`, `access-token-login`, `api-key`, `api-key-login`; no default | Claude accepts only native; native Codex means saved ChatGPT, not API key   |
| `AIDASH_CLI_EXECUTABLE`        | path or executable name; provider name                                                 | Backend launches this unmodified trusted CLI                                |
| `AIDASH_CLI_MODEL`             | full ID; no default                                                                    | Required, aliases/placeholders rejected                                     |
| `AIDASH_CLI_CONTEXT_WINDOW`    | integer 2048..2000000; no default                                                      | Required, verified by the user; must exceed output cap                      |
| `AIDASH_CLI_PORT`              | integer 1024..65535; 18190                                                             | Backend loopback only; examples use Codex 18191                             |
| `AIDASH_CLI_TIMEOUT_SECONDS`   | integer 1..900; 120                                                                    | Inference child deadline; capability probes have their own 15-second bounds |
| `AIDASH_CLI_MAX_OUTPUT_TOKENS` | integer 1..32768; 4096                                                                 | CLI output request and response validation                                  |
| `AIDASH_CLI_WORK_DIR`          | literal path; `private/cli-backends/work`                                              | Parent of private disposable cwd; never changes HOME or CODEX_HOME          |
| `CLAUDE_CONFIG_DIR`            | optional absolute path                                                                 | Only Claude child; official meaning unchanged                               |
| `CODEX_HOME`                   | optional absolute path                                                                 | Only Codex child; official state/auth root unchanged                        |
| `CODEX_ACCESS_TOKEN`           | secret, no default                                                                     | Required only for access-token; never argv                                  |
| `CODEX_API_KEY`                | secret, no default                                                                     | Required only for api-key; never argv                                       |

Saved access-token/API-key modes require the corresponding prior official login and
no nonempty direct secret. A nonempty secret for a different selected mode is an
error, even if inherited accidentally. Foreign config roots are rejected. Only the
selected provider's root and selected secret are forwarded. A small OS environment
allowlist preserves HOME, PATH, USER, LOGNAME, LANG, TMPDIR and Linux keyring transport;
provider, proxy, shell-startup, debugging and SDK injection variables are not inherited.
Diagnostics omit secrets and raw CLI stderr. HTTP failures contain stable error codes,
not provider output. To investigate auth, invoke the official CLI yourself.

## Host installation path

The following commands exist; the live route remains gated as described above.
Run from the checkout root. Build prerequisites and the underlying personal
Self-hosted Deployment are described in the root README. Use its database migration,
secret-store initialization, identity and dashboard setup before starting a Run.

1. Install official Claude Code on an unmanaged Linux host and verify `claude
--version` is 2.1.302. Codex setup can be prepared, but cannot serve inference yet.
2. Run `claude auth login` yourself under the host user. Do not copy tokens out of
   its credential store. Set `CLAUDE_CONFIG_DIR` explicitly if needed.
3. Build and initialize the dedicated file:

```sh
cargo build --locked -p aidash-server --bin cli-backend
./target/debug/cli-backend init claude
# Alternatives for preparation only:
./target/debug/cli-backend init codex-native
./target/debug/cli-backend init codex-access-token
./target/debug/cli-backend init codex-api-key
```

4. Edit `private/cli-backends/claude/.env` in a local editor. Set the full model ID
   and context window you have verified for the account. The initializer fills only
   known non-secret defaults and preserves existing files. Files are created 0600
   on POSIX. It does not search for tokens. Token profiles start with an empty secret.
5. Validate and check capabilities (neither command calls a model):

```sh
./target/debug/cli-backend --env-file private/cli-backends/claude/.env validate
./target/debug/cli-backend --env-file private/cli-backends/claude/.env doctor
```

6. Run the foreground backend in its own terminal:

```sh
./target/debug/cli-backend --env-file private/cli-backends/claude/.env serve
```

From another terminal:

```sh
./target/debug/cli-backend --env-file private/cli-backends/claude/.env health
# Explicit opt-in, consumes the selected real account's allowance:
./target/debug/cli-backend --env-file private/cli-backends/claude/.env smoke --live
```

7. Merge `examples/cli-backends/settings/keyless-endpoints.example.toml` into the
   host server **and Worker** settings, preserving existing permitted origins. Or
   explicitly pass `AIDASH_KEYLESS_PRIVATE_ORIGINS=http://127.0.0.1:18190` to both
   processes. This is not a CLI secret. Restart both. Use `server` and `worker`
   management commands in the root README with the existing self-hosted configuration.
   Keep all three processes in the host network namespace. A container's loopback
   cannot reach this listener; the root Compose Cloud stack is not this route.
   Do not change the bind address or publicly proxy the unauthenticated listener.
8. Generate validated definitions, choosing unused local Registry IDs:

```sh
./target/debug/cli-backend --env-file private/cli-backends/claude/.env examples \
  --model-entry local.claude-model --agent-entry local.claude-agent \
  --version 1.0.0 --output-dir private/cli-backends/claude/registry
```

Import/register `model.json` and then `agent.json` in the Tenant Registry. These are
proposed local IDs, not preexisting approvals. Review and grant Catalog Approval for
the exact versions and required dependencies in the existing Trust/Registry UI. Keep
the Tenant's normal permissions and human tool approvals. No command changes them.
The Agent removes the default `compaction` Decider explicitly: short Runs can avoid
an OpenRouter dependency, but long context can exhaust the budget and stop. This is
not an implicit fallback to CLI compaction. Bind an explicitly approved alternative
Decider for longer Runs, or keep the normal OpenRouter binding when desired.

9. In the dashboard, start a short goal using that approved Agent version, such as
   asking it to send a brief workspace message. Inspect the Run and journal, then any
   pending human approval. This is a separate verification from CLI smoke success.
10. Stop the foreground backend with Ctrl-C or `kill -TERM <backend-pid>`. Active
    exchanges are dropped and their child process groups killed; owned temporary
    directories are removed. Restart with the same `serve` command after config edits.
    SIGKILL/host failure can leave a disposable cwd; inspect and remove only the
    backend's own `private/cli-backends/work/call-*` / `probe-*` after stopping it.

The listener accepts only loopback clients, exact Host and JSON requests, rejects
browser Origin headers, caps body/output to 1/2 MiB, allows one active inference and
32 connections, and bounds read/connection lifetimes. SSE emits comment keepalives
while buffering; they are not generation progress. Only a validated final result is
sent as data. The example uses non-streaming because Aidash's stall clock correctly
does not count comments. Unknown usage remains absent; Claude cache breakdowns are
included once in total input. USD estimates are not emitted as OpenRouter credits.
Local users/processes can reach this unauthenticated service, hence the personal-host
scope. Process-group cleanup is not a containment mechanism for a malicious CLI.

## Verification and migration

```sh
cargo test --locked -p aidash-integrations --lib cli_backend
cargo test --locked -p aidash-runtime --lib cli_child
cargo test --locked -p aidash-server --bin cli-backend
python3 -m unittest discover -s scripts -p test_cli_backend_examples.py
scripts/check.sh
```

The first three suites use deterministic child CLI fixtures. Standard Rust partition
inventory places them in foundation/server-unit, which existing CI runs. The final
Python test is also in `check.sh` and infrastructure CI. No public CI needs personal
credentials. Fixture transport success, capability/login checks, live model smoke and
an Aidash Run are separate evidence levels. A fixture is never a supported official
CLI or an isolation proof. See the delivery manifest/PR notes for actual results.

No existing config changes automatically. Existing Keyless models remain valid. To
adopt this backend, create new Registry versions, configure the exact private origin,
and complete approval. Remove any incorrectly copied CLI secrets from shared server
environments; rotate them yourself if they were exposed. There is no prior patch or
prior Claude implementation assumed by this change.

## Applying the final patch

The delivery contains `private/aidash-cli-auth.patch` and its JSON manifest. The
manifest fixes the repository, PR base branch, base SHA, final HEAD, SHA-256 and exact
commands. The patch is generated from committed code with:

```sh
python3 scripts/make-cli-backend-patch.py \
  --base BASE_SHA --head HEAD_SHA --base-branch develop/0.1.0
# Internally: git diff --binary --full-index BASE_SHA HEAD_SHA
```

On a clean checkout at the manifest's base SHA, run `git apply --check PATCH`, then
`git apply --index PATCH`. `git write-tree` must match `git rev-parse HEAD_SHA^{tree}`
from the source repository, also recorded in the manifest. This verifies new files
and executable bits as well as content. Do not apply a patch on top of its own HEAD.
The patch excludes private envs, credentials, logs and itself. Regenerate it after
any new commit; the PR's latest HEAD and manifest must agree.
