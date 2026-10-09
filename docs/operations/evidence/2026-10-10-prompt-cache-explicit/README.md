# Explicit prompt-cache evidence: procurement-001, one node

Live, proxy-recorded matched-task evidence for #142 explicit cache breakpoints.
It compares Ordered requests without and with `cache_control` on one
explicit-caching OpenRouter route.

The harness, task, oracle and harness description are in
`docs/operations/evidence/2026-10-10-prompt-cache-prefix/` (#172, `run.py`,
`case.json`, `oracle.json`, `README.md`). `results-explicit.json` here is the
compact, prompt-free result: per-call usage, request-shape checks, per-run
outcomes, per-variant aggregates and the route spend ledger. Raw traces with
full prompts are not committed. They are in
`.ignore/benchmarks/20261010-prompt-cache/route-b-20261009T205044Z-517c8d/`
on the machine that ran the evidence.

## Route B: explicit caching, Ordered without vs with `cache_control`

| Item | Value |
| --- | --- |
| Date | 2026-10-09 20:50–20:55 UTC (2026-10-10 JST) |
| Binary | W2 `feat/issue-142-stable-request-prefix` working tree on `889386f161eed7a320431a817122ae663309c218`, `cargo build --release --locked --bin aidash`. Those uncommitted Rust changes were committed unchanged as this branch's feature commit; the branch was later rebased onto `22837f11`, which changed only CI and test-support files. |
| Binary SHA-256 | `de1f81fccbdc3455ba662d1dc7c2e62eccc3a02c4c3d2552c22aa851fa03bb28` |
| Working tree | Uncommitted: 34 modified and 3 untracked files, including migration `registry/0017_prompt_cache`. The `git diff HEAD` SHA-256 `9995b7ba9df81601c84871cb8c3018e87933b337dd6c6d2c2c2cda795d06bfc2` was identical before the build, after the build and after the runs. The file list is `source.status_short` in `results-explicit.json`. |
| Model | `anthropic/claude-haiku-5.5` (`anthropic/claude-haiku-5.5-20261007`), `reasoning_effort` `medium`. Every call was served by Google (Vertex) through OpenRouter with `provider.zdr = true`. |
| Model registration | `cache_mode: explicit`, `projection_versions: [legacy, ordered]` |
| B1 | Agent `projection_version: ordered`, `prompt_cache` default (`off`) |
| B2 | Agent `projection_version: ordered`, `prompt_cache: explicit` |
| Schedule | 3 repeats, interleaved B1, B2, B1, B2, B1, B2 on one node with one random cache key |

### Model choice

Public `GET /api/v1/models` and `GET /api/v1/endpoints/zdr` were checked on
2026-10-10:

- `anthropic/claude-haiku-5.5` is the cheapest non-batch Anthropic model:
  USD 0.10 / 1M input, 0.50 / 1M output, 0.01 / 1M cache read and 0.125 / 1M
  5-minute cache write. It has six ZDR endpoints (Google Vertex global, us and
  europe; Amazon Bedrock ×3), and each supports `tools` and `reasoning`.
- The next cheapest is `claude-haiku-4.5` at USD 1 / 1M input. Its minimum
  cacheable prompt is 4,096 tokens, which is above this task's stable prefix.
- [Anthropic's prompt-caching documentation](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
  lists a 512-token minimum cacheable prompt for Claude Haiku 5.5 on the Claude
  API, AWS and Google Cloud. Bedrock minimums are documented separately by AWS.
  This is far below the first-call prompt here (about 4.1k tokens) and the
  `system` plus tools prefix.
- Before the runs, a direct ZDR probe sent a 2.8k-token `system` block marked
  with `cache_control`. It reported `cache_write_tokens` 2,809, then
  `cached_tokens` 2,809 on two repeats.

### Results (3 runs per variant)

| Variant | Runs passed / completed / attempted | Inference calls | Prompt tokens | Cached tokens (read) | Cached share, calls 2..n | Cache-write tokens | Completion tokens | Cost (USD) | Mean cost per run | Cost per call | Mean elapsed |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| B1 Ordered, no `cache_control` | 3 / 3 / 3 | 19 | 111,250 | 0 | 0.0% | 0 | 9,037 | 0.015644 | 0.005214 | 0.000823 | 29.3 s |
| B2 Ordered + `cache_control` | 3 / 3 / 3 | 24 | 150,095 | 53,766 | 36.8% | 37,963 | 11,596 | 0.016918 | 0.005639 | 0.000705 | 40.6 s |

Call counts:

- B1 made 6, 7 and 6 calls. B2 made 12, 6 and 6.
- Every call beyond the five of a compliant run was a retry after
  schema-rejected tool arguments: the model sent `{"arguments":{}}` instead of
  `{}`. That accounts for 4 B1 calls and 9 B2 calls, 7 of them in B2-r1.
- All 6 runs returned the optimum. There were no infrastructure errors,
  upstream errors or proxy refusals (0 / 6 runs, 0 / 43 calls).

Cache behavior:

- B1 reported 0 cached and 0 cache-write tokens on all 19 calls. Without
  breakpoints, this route did not cache.
- In B2, 16 of 21 calls after the first read from the cache, between 1,537 and
  4,878 tokens each. The same calls wrote the uncached remainder up to the
  history breakpoint: 253–847 tokens when the previous call's prefix was read,
  and 1,646–2,898 tokens when only an older, shorter prefix was read (1,537 or
  4,364 tokens).
- The other 5 read nothing and re-wrote the whole prefix (2.9k–5.6k tokens).
  [INFERENCE] Placement on a different Vertex endpoint is the likely cause; the
  response reports only `Google`.
- The first call of B2-r2 and B2-r3 read 1,537 tokens written by the previous B2
  run on the same node and cache key.

Cost:

- B2 cost 14.4% less per call (0.000823 to 0.000705 USD). Its mean cost per
  run was 8.1% higher because B2-r1 made 12 calls.
- The four runs with six calls each give a matched comparison: B2-r2 0.004683
  and B2-r3 0.003746, against B1-r1 0.004773 and B1-r3 0.004932.
- Cache writes are billed above the base input price (catalog: 1.25×). On this
  short task they offset part of the read savings.

Recorded-request checks (43 / 43 calls):

- Each of the 24 B2 requests carried exactly two `cache_control` markers: one
  on `system[0]` and one on user part `n-2`. That part is the last history part,
  or the Run-context part on the first call.
- None of the 19 B1 requests carried a marker.
- Every B1 and B2 `system` began with `Cache scope: k00000001.`.

### Route B spend

USD 0.032978 recorded against the USD 5 ceiling: 0.000417 for the three direct
probes and 0.032561 for the six runs.

### Limitations

- One task (procurement-001) and 3 repeats per variant. No claim beyond this
  task, model, route and time window.
- Call counts depend on model argument errors, which confounds totals. Compare
  cost per call, cached share on calls 2..n and the matched six-call runs.
- The binary was built from an uncommitted working tree. The commit and diff
  hash above identify it, but the diff itself is not part of this evidence.
- Provider caches outlive a run. With one node and one cache key, call 1 of a
  later run can reuse an earlier run's cached prefix.
- OpenRouter routing and provider cache placement are not controlled. Host
  load is not controlled either: the #172 route A run overlapped this run, so
  elapsed times are indicative only.
- `diff_sha256_stripped_text` in `results-explicit.json` is the value recorded at
  run time, a hash of the diff without its final newline. `diff_sha256` was
  recomputed from the unchanged tree.

### Reproduce

```sh
cargo build --release --locked --bin aidash   # in the #142 worktree
export OPENROUTER_API_KEY=...   # never recorded
python3 docs/operations/evidence/2026-10-10-prompt-cache-prefix/run.py \
  --binary /path/to/aidash-w2 --source-root "$PWD" --out /path/outside/repo \
  --route route-b --model anthropic/claude-haiku-5.5 --reasoning-effort medium \
  --variant "B1 projection=ordered projection_versions=legacy+ordered model_cache_mode=explicit prompt_cache=off" \
  --variant "B2 projection=ordered projection_versions=legacy+ordered model_cache_mode=explicit prompt_cache=explicit" \
  --repeats 3 --run-timeout 420 --results /path/outside/repo/route-b.json
```
