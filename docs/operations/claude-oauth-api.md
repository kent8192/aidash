# Run Aidash inference with a Claude OAuth token

Select `AIDASH_CLI_PROVIDER=claude` and `AIDASH_CLI_AUTH_MODE=oauth-token` to
authenticate directly to Anthropic's Messages API with your own
`CLAUDE_CODE_OAUTH_TOKEN` (`sk-ant-oat01-...`). This mode does not install or run
OpenClaw or Claude Code. It does not read a saved CLI login. macOS and Linux use
the same HTTPS transport; the native CLI mode's Linux/account restrictions do not
apply to it.

The existing `cli-backend` executable also hosts this API transport. Its name and
`AIDASH_CLI_*` configuration names are retained for compatibility. Aidash sends its
existing OpenAI-compatible requests to the loopback endpoint; the backend converts
them to Messages API requests and authenticates with the selected token. Responses
and tool proposals return to Aidash for its normal Run, journal, approval and tool
execution. No shell, web, file or MCP tool is executed by the backend.

## Configure and start

Run from this feature checkout, with Rust and Python 3 available:

```sh
cargo build -p aidash-server --bin cli-backend --locked
target/debug/cli-backend init claude-oauth
```

This creates `private/cli-backends/claude-oauth/.env` with mode 0600 on POSIX.
Running init again preserves the existing file. Edit that private file locally:

```sh
${EDITOR:-vi} private/cli-backends/claude-oauth/.env
```

Set these three values:

- `CLAUDE_CODE_OAUTH_TOKEN`: your complete setup-token, with no surrounding spaces.
- `AIDASH_CLI_MODEL`: the full Anthropic model ID your account can use, without an
  `anthropic/` OpenRouter prefix. A prefix alone does not prove token validity.
- `AIDASH_CLI_CONTEXT_WINDOW`: a verified context budget for that model, greater
  than the configured maximum output (4096 by default).

Keep the token out of chat, shell arguments, root `.env`, Registry JSON and Git.
The backend reads the file literally: it never uses `source`, `eval` or shell
expansion. Explicit non-secret arguments override environment variables, which
override the named file; defaults are last. Restart to reload settings or a token.
`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, executable and work-directory settings are
rejected in this mode because no CLI is used. Codex secrets conflict with this
authentication mode. Unrelated ambient API keys are not used or forwarded.

```sh
target/debug/cli-backend --env-file private/cli-backends/claude-oauth/.env validate
target/debug/cli-backend --env-file private/cli-backends/claude-oauth/.env doctor
target/debug/cli-backend --env-file private/cli-backends/claude-oauth/.env serve
```

Leave `serve` running. In a second terminal in the same checkout:

```sh
target/debug/cli-backend --env-file private/cli-backends/claude-oauth/.env health
target/debug/cli-backend --env-file private/cli-backends/claude-oauth/.env smoke --live
```

`validate`, `doctor` and `health` do not call a model or prove token access.
`smoke --live` makes a short real API request and checks for a successful `OK`
response. It uses the selected account's allowance. Stop with Ctrl+C; restart with
the same `serve` command after editing the private file.

## Connect Aidash

Use host processes for the backend, server and Worker. Permit the exact origin
`http://127.0.0.1:18190` in **both** server and Worker using
[the existing settings fragment](../../examples/cli-backends/settings/keyless-endpoints.example.toml)
and [host deployment instructions](cli-backends.md). Restart both after changing
their configuration. The registered model endpoint is
`http://127.0.0.1:18190/v1`; the allowed origin has no `/v1`.

Generate one consistent pair of Registry definitions, choosing unused entry IDs:

```sh
target/debug/cli-backend --env-file private/cli-backends/claude-oauth/.env examples \
  --model-entry personal-claude-oauth --agent-entry personal-claude-oauth-agent \
  --output-dir private/cli-backends/claude-oauth/registry
```

The generator validates the real Registry schemas, copies the model ID and limits
from your configuration, and refuses to overwrite existing output. Register
`model.json` and `agent.json` through Aidash's Registry, obtain Catalog Approval
through the existing workflow, then start a short Run with that Agent. These IDs
are newly chosen definitions, not claims of existing registrations or approval.
The model's `provider: openrouter` selects Aidash's existing OpenAI-compatible
wire adapter; the endpoint sends **no request to OpenRouter**. There is no
`provider_credential` or `plan_connection` in these definitions. The token belongs
only to the local backend, outside the server/Worker's credential stores.

The minimal Agent removes default compaction, which otherwise calls a separate
OpenRouter Decider. Embedding, memory and research routes keep their own settings.
Containers have separate loopback interfaces: this guide does not claim that
container `127.0.0.1` reaches the host backend. Do not expose this listener publicly.

## Protocol and verification limits

Requests go only to `https://api.anthropic.com/v1/messages`, using
`Authorization: Bearer`, `anthropic-version: 2023-06-01` and
`anthropic-beta: oauth-2025-04-20`. No redirects or environment proxies are used.
The client identifies itself as Aidash, not as the official Claude Code executable.
There is no API-key fallback, account fallback or custom token refresh. A 401/403
is returned as a safe error; changing a User-Agent or claiming another client to
evade an upstream rejection is not part of this implementation.

This initial route supports text and ordinary client function tools. It does not
enable provider-hosted tools, explicit prompt caching, images or retained thinking
blocks. Requests for unsupported response blocks fail before tools are dispatched.
Only completed `end_turn` or `tool_use` messages are accepted, with exact reported
model matching and validated tool names/JSON object arguments. Use a full model
ID; a provider alias that resolves to a different ID fails this check. Refusals,
partial output and token-limit stops cannot dispatch tool proposals. JSON and SSE
share the same final-response validation; SSE comments are keepalives, not generated
text. Unknown usage remains absent and USD is never reported as OpenRouter credits.

Fixture tests cover authentication headers, tool/result round trips, schema
validation, JSON/SSE, malformed/terminal responses, missing credentials, safe
401/403/404/429 errors, redirects and deadlines. These tests do not prove that
Anthropic currently accepts a specific token, plan or model. A successful live
smoke is API evidence; a successful Aidash Run and its journal are separate evidence.
No live token or Aidash Run was available during implementation.

References checked on 2026-10-31:

- [The requested OAuth example](https://openclaw.cc/en/concepts/oauth#anthropic-setup-token)
- [OpenClaw's setup-token and direct Messages API description](https://docs.openclaw.ai/providers/anthropic#get-a-setup-token)
- [Messages API](https://platform.claude.com/docs/en/api/messages)
- [Claude setup-token](https://code.claude.com/docs/en/authentication#generate-a-long-lived-token)
