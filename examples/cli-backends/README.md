# CLI backend examples

Read the [operating guide](../../docs/operations/cli-backends.md) first. This is an
experimental, incomplete implementation: Codex inference fails closed, and Claude's
candidate profile is limited to personal Pro/Max saved login on unmanaged Linux.
Live Linux/CLI/Aidash acceptance is still required. The code does not claim support
for managed accounts, macOS, Windows, WSL or Compose networking.

| Directory                         | Purpose                                                        |
| --------------------------------- | -------------------------------------------------------------- |
| `claude/.env.example`             | Claude saved login; port 18190                                 |
| `codex-native/.env.example`       | Codex saved ChatGPT login; port 18191; currently blocked       |
| `codex-access-token/.env.example` | Official Codex access token; currently blocked                 |
| `codex-api-key/.env.example`      | Explicit Codex API key; currently blocked                      |
| `registry/`                       | Model/Agent templates; replace placeholders with the generator |
| `settings/`                       | Exact origin fragment for both host server and Worker          |

Build `cli-backend`, then run `cli-backend init PROFILE` from the checkout root.
It copies the selected template to `private/cli-backends/PROFILE/.env`, creates it
0600 and never overwrites it. Edit locally; do not source it. `--env-file` is required
for validation, diagnostics, serving, health, smoke and example generation. Set
`AIDASH_CLI_MODEL` and `AIDASH_CLI_CONTEXT_WINDOW` from your account's verified values.
A secret mode also needs its one secret in the private file or process environment.
No secrets belong in the root `.env` or Registry files.

Use `cli-backend ... examples --model-entry ID --agent-entry ID --output-dir DIR`
to generate actual Registry JSON from the same resolved configuration. Static JSON
files are templates and are not directly importable before placeholder replacement.
Tests substitute concrete non-secret fixture values and deserialize the real Entry,
ModelConfig and AgentConfig schema, then run the actual Registry admission rules.
Output has no fabricated approval. Register and
approve the versions through Aidash's existing workflow.

Only Claude's origin appears in the settings fragment because Codex serving is
blocked. The model endpoint includes `/v1`; the allowed origin does not. Ports,
models, limits and Agent references in generated definitions come from one config.
The model/Agent examples use Legacy projection, text input, no explicit cache markers
and non-streaming. The Agent explicitly removes default compaction for the minimal
short-Run setup. Other inference dependencies remain separately configured.
