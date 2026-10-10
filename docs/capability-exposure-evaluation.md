# Capability exposure evaluation

`scripts/evaluate-capability-exposure.py` compares the legacy and `deferred@1`
[Exposure policies](operations/registry-capabilities.md#deferred-capability-exposure)
on the frozen synthetic cases in
`tests/fixtures/capability_exposure_evaluation.toml`. For each case it registers
the same tools and Skills under two new Agents, one without `exposure` and one
with `{"version": "deferred@1"}` (plus any budgets or Eager bindings the case
declares). It runs one Task with each Agent and reads the Run inspection.

Results are measurements only. The runner makes no accuracy, quality or cost
claim; a single Run per variant is not a statistical comparison. The report's
`accuracy_claim_approved`, `cost_claim_approved` and
`production_quality_approved` flags are always `false`.

## Usage

Start a server with a worker and register the model to evaluate. The model
entry carries provider credentials, so the runner does not create it.

```sh
python3 scripts/evaluate-capability-exposure.py \
  --base-url http://127.0.0.1:8080 \
  --model-id evaluation-model --model-version 1.0.0 \
  --server-revision DEPLOYED_SOURCE_FINGERPRINT \
  --output /tmp/capability-exposure-evaluation.json
```

Supply the bearer token in `AIDASH_EVALUATION_TOKEN`, or name another variable
with `--token-env`. Registry writes are operator-only, so it must be the
operator token. The token is never copied into the report. The output file is
created exclusively with mode 0600 and contains raw Task text, tool arguments
and model-visible results.

Every fixture tool is an `integration.http@1` declaration, and such calls need
human approval. By default the runner denies each approval request, so the
declared endpoint (`--tool-endpoint`, by default an unroutable local address) is
never contacted. The model's call is still recorded with a denial result. Use
`--approve-external-calls` only with an endpoint that the server may reach and
that accepts the fixture arguments. Other human requests receive a fixed
"no human is available" answer. A Run still running after `--run-timeout`
seconds (default 600) is cancelled and reported with `timed_out: true`.

Each invocation registers new immutable entries under a random `cxe-<hex>`
prefix: tools, Skills and two Agents per case. Each variant runs in a new
Workspace. Use an isolated deployment.

## API calls

| Purpose                     | Call                                                                                            | Defined at                                                 |
| --------------------------- | ----------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| Node identity               | `GET /.well-known/aidash`                                                                       | `server/src/apps/federation/peer/views/management.rs:43`   |
| Register tool, Skill, Agent | `POST /api/registry`                                                                            | `server/src/apps/registry/views/management.rs:40`          |
| Workspace                   | `POST /api/workspaces`                                                                          | `server/src/apps/workspaces/views/management.rs:32`        |
| Task                        | `POST /api/workspaces/{id}/tasks`                                                               | `server/src/apps/workspaces/views/management.rs:60`        |
| Admit the Run               | `POST /api/tasks/{id}/delegate` with this node's ID                                             | `server/src/apps/federation/remote/views/management.rs:15` |
| Find the Run and approvals  | `GET /api/state` (`runs`, `human_requests`)                                                     | `server/src/apps/execution/views/management.rs:20`         |
| Answer approvals            | `POST /api/human-requests/{id}/answer`                                                          | `server/src/apps/execution/views/management.rs:80`         |
| Run inspection              | `GET /api/runs/{id}`                                                                            | `server/src/apps/execution/views/management.rs:49`         |
| Cancel on timeout           | `POST /api/runs/{id}/control` with `{"action": "cancel"}`                                       | `server/src/apps/execution/views/management.rs:59`         |
| Per-request usage           | `GET /api/workspaces/{id}` (`model.completed` events with `run_id`, `step` and `context_usage`) | `server/src/apps/workspaces/views/management.rs:41`        |

## Fixture

`version`, `instructions` and `max_steps` apply to every Agent. `[[tools]]`
declares an `alias`, a `description` and a JSON Schema string; `[[skills]]`
declares an `id`, `name`, `description`, `instructions` and optional `files`.
Each `[[cases]]` entry has an `id`, `language`, `task` and `expected_tool`, and
selects `tools` (a list of aliases or `"all"`) and optional `skills`. It may
also set `remove_default`, `max_steps`, `eager` (aliases bound with
`"exposure": "eager"` in the deferred variant only) and a `[cases.deferred]`
table of budgets merged into the deferred policy.

## Report fields

The top level records `fixture`, `fixture_sha256`, the declared `profile`
(server revision, node, model, `model_acceptance: "unapproved"`),
`grade: "measurement-only"`, `external_calls` (`denied` or `approved`), the
approval flags above and `limitations`. `requests[]` lists every API call with
its phase, path, HTTP status and `round_trip_ms`. On failure, the report keeps
everything measured so far and adds `error`.

Each `cases[]` item has the case inputs, `tool_count`, `skill_count`, the
registered `registry` identities and `variants.legacy` / `variants.deferred`.
Each variant has these fields:

- `agent`, `exposure_policy` (`null` for legacy), `workspace`, `task`,
  `run_id`, final `phase`, `error`, `state_error`, `step`, `timed_out`,
  `compactions`.
- `provider_usage`: provider token counts from `context.usage`.
  - `input_tokens` and `output_tokens` are sums over the `model.completed`
    events the runner observed; `observed_requests` is how many there were.
  - `latest` is the Run's final `context.usage`.
  - `event_window_complete` is false if the Workspace returned its full window
    of 100 events, so earlier requests may be missing from the sums.
- `exposure_bytes` (deferred only): per-request lists of `schema_bytes` (tool
  definitions), `skill_bytes` (resident Skill bodies) and `metadata_bytes`
  (capability index) from `context.usage.exposure`, the final `latest` usage,
  and the Run's load `state` (`context.exposure`).
- `requests`: per-request `step`, `input_tokens`, `output_tokens` and
  `exposure`, including the exposed `[alias, digest]` pairs.
- `tool_calls`: every tool event in `context.history`, in order, with `name`,
  `arguments`, `error` and `dispatch_rejected`. `dispatch_rejected` is true when
  the call was refused because the alias was unavailable, not loaded, or loaded
  only by an earlier call of the same response.
- `capability_calls`: counts of `capability_search`, `capability_describe`,
  `capability_load`, `capability_unload` and `skill_asset_read` calls.
- `selection`:
  - `called`: the model emitted a call to `expected_tool`;
  - `selected`: at least one such call passed the exposure dispatch check. It
    may still have been denied approval, which is the default.
  - `first_call_index` and `first_selected_index` give positions in
    `tool_calls`.
- `human_answers`: each human request the runner answered and its answer.

## What the API does not expose

- `context.usage` holds only the latest provider response. Per-request token
  counts come from `model.completed` events, and `GET /api/workspaces/{id}`
  returns at most the 100 most recent events (see `event_window_complete`).
- Legacy requests record no tool-definition or Skill-body bytes;
  `context.usage.exposure` exists only for `deferred@1` requests. The runner
  does not estimate the legacy values.
- No endpoint returns the tool list or instructions actually sent in a request.
  `exposed` names the deferred Exposure set, but not its text.
- Compaction can remove older tool events from `context.history`; check
  `compactions` before reading `tool_calls` as complete.
