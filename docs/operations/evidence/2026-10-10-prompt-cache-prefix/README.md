# Prompt-cache prefix evidence: procurement-001, one node

Live, proxy-recorded matched-task evidence for the Ordered projection (#172).
Route A compares `legacy` with `ordered` on an automatic-caching OpenRouter
route. The explicit-caching comparison (#142) uses the same harness and is
reported in `docs/operations/evidence/2026-10-10-prompt-cache-explicit/` on the
#142 branch.

## Files

- `run.py`: single-node harness (Python 3, standard library only).
- `case.json`, `oracle.json`: goal, Agent instructions, facts-tool data and all
  nine feasible plans. Regenerate with `python3 run.py --prepare-only`.
- `results-automatic.json`: compact, prompt-free results of route A. It contains
  per-call usage, request-shape checks, per-run outcomes, per-variant aggregates
  and the route spend ledger.

Raw traces (`calls.jsonl` with full prompts and responses, tool invocations,
node logs, workspace snapshots, per-run reports) are not committed. They are in
`.ignore/benchmarks/20261010-prompt-cache/` on
the machine that ran the evidence.

## Harness

`run.py` builds nothing. It takes a prebuilt `--binary` and the worktree it
came from (`--source-root`, used for `compose.yaml` and `server/migrations`).
One invocation does the following:

1. Starts PostgreSQL and NATS from `compose.yaml` under a unique Compose project
   name on free ports, then creates a new database from `template0`.
2. Applies migrations with `aidash migrate`, then starts one `aidash serve`
   process. `AIDASH_PROMPT_CACHE_KEY` is a new random 64-hex-character key for
   each invocation and is never written to disk. `AIDASH_PROMPT_CACHE_KEY_VERSION`
   is set only with `--prompt-cache-key-version`.
3. Runs a local proxy. It forwards `/chat/completions` unchanged to
   `https://openrouter.ai/api/v1/chat/completions` and records the request and
   response bodies. It never records Authorization headers. The proxy refuses
   requests for another model, requests without `provider.zdr == true`, calls
   beyond `--max-calls-per-run` or `--max-calls`, and calls after the route
   ceiling is reached. The ceiling uses the recorded `usage.cost` sum in a
   per-route ledger (`OUT/ledger-ROUTE.json`, persisted across invocations).
   It refuses the next call when the spend plus the largest call cost so far
   would exceed `--cost-ceiling-usd` (default 5). It also refuses further calls
   after a successful response without `usage.cost`, because the ceiling can no
   longer be enforced. A run is not started when the spend plus the most
   expensive run so far would exceed the ceiling.
4. Serves four deterministic facts tools (`procurement`, `engineering`,
   `transport`, `production`). They are registered as `integration.http@1`
   tools with a node-local credential. The harness approves only the exact
   external-call approval requests of these four tools.
5. Registers one model and one Agent per variant through `/api/registry`.
   Model fields come from the live OpenRouter catalog (context window, maximum
   output, prices). The Agent binds the four facts tools, removes every
   removable default tool and uses `max_steps` 12. Each new field is sent only
   when it differs from the default: `projection_versions`, `cache_mode`,
   `projection_version` and `prompt_cache`. A binary that predates a field
   never receives it.
6. Runs `--repeats` rounds, interleaving the variants in the given order (A1,
   A2, A1, A2, ...). Each run submits the goal through `/api/conversations`,
   waits for a terminal root-task status (`--run-timeout`, default 600 s),
   cancels any unfinished Run of a timed-out task, and scores the final
   artifact against the oracle.
7. Writes `setup.json`, `registered.json`, `calls.jsonl`, `tools.jsonl`,
   `refusals.jsonl`, `node.log`, `runs/<variant>-r<k>/{report,workspace,runs}.json`
   and `aggregate.json` under `OUT/<route>-<UTC timestamp>-<id>/`. With
   `--results`, it also writes the prompt-free aggregate. Containers are then
   stopped with `docker compose down`, keeping the volumes of the unique
   project, unless `--keep-infra` is given.

Variants use `--variant 'LABEL key=value ...'`. The keys are `projection`
(`legacy|ordered`), `projection_versions` (`legacy+ordered`),
`model_cache_mode` (`none|automatic|explicit`) and `prompt_cache`
(`off|explicit`). Without `--variant`, the top-level flags of the same names
define a single variant.

Each per-call record lists `prompt_tokens`, `completion_tokens`, reasoning
tokens, `prompt_tokens_details.cached_tokens`,
`prompt_tokens_details.cache_write_tokens` and `usage.cost`. A missing value is
recorded as `null`, meaning unknown, never zero. A sum that contains an unknown
value is also unknown. The record also includes the response provider and the
tool calls the model requested. The request-shape check counts every
`cache_control` marker and requires one of two shapes:

- `prompt_cache=explicit`: exactly two markers, on `system[0]` and on user part
  `n-2` (the last history part, or the Run-context part when the history is
  empty).
- Any other variant: no markers.

It also requires `system` to begin with `Cache scope: k` exactly when the
projection is `ordered`. Calls are attributed to a run by the root task ID in
the request context.

### Task

procurement-001 from the 2026-09-21 federation pilot, adapted to one node. One
Agent calls exactly one facts tool per response in the fixed order procurement,
engineering, transport, production, then returns only the final JSON. A
compliant run makes five inference calls. Later calls can reuse the prefix of
earlier ones. The oracle is unchanged: there are nine feasible plans, the unique
optimum costs JPY 440,000 and the next cheapest costs JPY 445,000. A run passes
when all of the following hold:

- The root task is `COMPLETED`.
- Every field of the final JSON matches the optimum.
- `evidence_ids` contains exactly the four evidence IDs.
- All four facts tools were invoked.

Tool-order compliance is reported separately and is not part of the oracle.

## Route A: automatic caching, Legacy vs Ordered

| Item           | Value                                                                                                                                                                                                                                                                                  |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Date           | 2026-10-09 20:44–20:53 UTC (2026-10-10 JST)                                                                                                                                                                                                                                            |
| Binary         | W1 `feat/issue-172-ordered-projection` at `889386f161eed7a320431a817122ae663309c218`, `cargo build --release --locked --bin aidash`. The branch was later rebased onto `22837f11`, which changed only CI and test-support files; the Rust sources of the feature commit are unchanged. |
| Binary SHA-256 | `a29d92d51512e06a21750a41b0768b461fd6f75280d34384a9ce45e801a6d6e0`                                                                                                                                                                                                                     |
| Working tree   | Only `web/src/forms.tsx` modified, which is outside the Rust binary, plus this untracked directory. `git diff HEAD` SHA-256 `a5fb7d05af98d1a31db8ad2243dbf9d5b248dcb6824dea104ba65d9eb77c7b3e`                                                                                         |
| Model          | `openai/gpt-5.4-mini`, `reasoning_effort` `medium`. Every call was served by Azure through OpenRouter with `provider.zdr = true`.                                                                                                                                                      |
| A1             | Agent default projection (`legacy`); model registered without `projection_versions`                                                                                                                                                                                                    |
| A2             | Agent `projection_version: ordered`; model `projection_versions: [legacy, ordered]`                                                                                                                                                                                                    |
| Schedule       | 3 repeats, interleaved A1, A2, A1, A2, A1, A2 on one node with one cache key                                                                                                                                                                                                           |

Route choice: the specified Gemini route (`google/gemini-3.8-flash`, medium) was
run once per variant first. Both runs passed with five calls, but no call
reported cached tokens. These prompts are 2.6k–4.7k tokens, and Google documents
a 4,096-token minimum for Gemini 3.8 Flash implicit caching, so that route cannot
show the effect on this task. It was stopped and is kept as a documented
below-threshold case (`below_threshold_case` in `results-automatic.json`). The
prompt was not padded. ZDR OpenAI routes were then probed directly with two
repeated ~2.9k-token prompts: `gpt-4.1-mini`, `gpt-5.1-codex-mini`,
`gpt-5.4-mini` and `gpt-5-mini`. All four reported `cached_tokens` 2,816 on the
repeat, consistent with OpenAI's 1,024-token automatic-caching minimum. Of
these, `gpt-5.4-mini` is the most recent `-mini` model.

### Results (3 runs per variant)

| Variant    | Runs passed / completed / attempted | Inference calls | Prompt tokens | Cached tokens | Cached share, calls 2..n | Cache-write tokens | Completion tokens | Cost (USD) | Mean cost per run | Cost per call | Mean elapsed |
| ---------- | ----------------------------------- | --------------- | ------------- | ------------- | ------------------------ | ------------------ | ----------------- | ---------- | ----------------- | ------------- | ------------ |
| A1 Legacy  | 1 / 3 / 3                           | 24              | 94,941        | 0             | 0.0%                     | 0                  | 8,383             | 0.108929   | 0.036310          | 0.004539      | 65.4 s       |
| A2 Ordered | 3 / 3 / 3                           | 20              | 75,652        | 26,112        | 38.0%                    | 0                  | 9,710             | 0.082808   | 0.027603          | 0.004140      | 90.5 s       |

Per-run detail:

- A1 made 7, 5 and 12 inference calls. A2 made 10, 5 and 5.
- A1-r2 completed with a wrong plan after calling only `procurement_facts`.
- A1-r3 ended `FAILED` at the 12-step limit after repeated `procurement_facts`
  calls.
- A1-r1 and A2-r1 also repeated `procurement_facts` but recovered.

All failures are model protocol deviations. There were no infrastructure
errors, upstream errors or proxy refusals (0 / 6 runs, 0 / 44 calls).

Cache behavior:

- No A1 call was cached.
- Every A2 call after the first reported cached tokens in 512-token steps (1,024
  to 3,072), except one: the final call of A2-r3 reported 0.

Recorded-request checks (44 / 44 calls):

- No request carried `cache_control`.
- Every A2 `system` began with `Cache scope: k00000001.` and no A1 `system` did.
- The Ordered user content grew by two text parts per tool step: the approval
  event and the tool event. Earlier parts and `system` were byte-identical
  between consecutive calls, checked on the Gemini A2 run.

Interpretation, limited to this task:

- With Ordered, later calls reused about 38% of their prompt tokens. Legacy
  reused none.
- [INFERENCE] The Legacy context JSON places the per-step state before the
  history, so only `system` and the tools are stable. Together they are below
  OpenAI's 1,024-token minimum.
- Cost per call fell by 8.8% (0.004539 to 0.004140 USD), and A2 also made fewer
  calls.
- Elapsed time and pass rate differ, but 3 runs with model-driven call counts
  cannot attribute either difference to the projection.

### Route A spend

USD 0.274184 recorded against the USD 5 ceiling:

- 0.074514: Gemini runs.
- 0.007932: 12 direct cache probes.
- 0.191738: gpt-5.4-mini runs.

### Limitations

- One task (procurement-001) and 3 repeats per variant. No claim beyond this
  task, model, route and time window.
- Call counts depend on model compliance, which confounds totals. Compare cost
  per call and cached share on calls 2..n.
- Provider caches outlive a run. With one node and one cache key, call 1 of a
  later run can reuse a previous run's prefix. Interleaving spreads this
  equally across variants.
- External-call approvals appear as history events. They are identical across
  variants.
- One node on one machine. OpenRouter routing and provider cache placement are
  not controlled. Host load is not controlled either: the #142 release build
  and the route B run overlapped part of this run, so elapsed times are
  indicative only.
- `results-automatic.json` was produced before the harness began hashing the
  raw `git diff HEAD` bytes. `diff_sha256_stripped_text` is the value recorded
  at run time, a hash of the diff without its final newline. `diff_sha256` was
  recomputed afterwards from the unchanged tree; the recorded stripped hash
  still matched.

### Reproduce

```sh
cargo build --release --locked --bin aidash   # in the #172 worktree
cp "$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/release/aidash" /path/to/aidash-w1
export OPENROUTER_API_KEY=...   # never recorded
python3 docs/operations/evidence/2026-10-10-prompt-cache-prefix/run.py \
  --binary /path/to/aidash-w1 --source-root "$PWD" --out /path/outside/repo \
  --route route-a --model openai/gpt-5.4-mini --reasoning-effort medium \
  --variant "A1 projection=legacy" \
  --variant "A2 projection=ordered projection_versions=legacy+ordered" \
  --repeats 3 --run-timeout 420 --results /path/outside/repo/route-a.json
```

The Gemini case used the same command with `--model google/gemini-3.8-flash
--repeats 1`.
