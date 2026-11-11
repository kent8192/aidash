# Prompt cache routes: live check through OpenRouter

Live check for #776, run 2026-11-07 against OpenRouter's Chat Completions API.
It decided which method each model family uses (ADR 0115).

`probe.py` sends synthetic text shaped like a Native request: a salted `system`,
8 tools, one attachment message, append-only history, then one volatile user
message. Each arm uses a fresh random salt, so arms cannot read each other.
`results.json` is usage only: no prompts, no key. The check made 210 calls and
cost 0.64 credits.

That probe went straight to OpenRouter, so it shows what the routes do with these
request shapes. [Real Runs](#real-runs-through-aidash) then checked what a real
Agent reads. Neither went through the broker.

## Results

"Calls that read" and "read share" cover step 3 onward, once a prefix has had two
chances to be written. Read share is cached tokens over prompt tokens, so its
ceiling depends on the volatile message. Prompts were 5,000 to 30,000 tokens.

### GPT-6.1 Terra via Azure

| Request                                                           | Read share | Mean write per step |    Credits |
| ----------------------------------------------------------------- | ---------: | ------------------: | ---------: |
| No markers                                                        |       64 % |               2,287 |     0.0213 |
| Implicit mode, 2 markers                                          |       91 % |                 583 |     0.0132 |
| Explicit mode, 2 moving markers                                   |       64 % |               2,100 |     0.0209 |
| **Explicit mode, 3 markers (`system`, previous step, this step)** |   **91 %** |             **378** | **0.0126** |

Without markers, the read stops at the first 4,038 tokens and the write grows
every step. Moving to Native alone would not end the loss this route showed under
Legacy. Explicit mode with moving markers fails the same way: it looks up only the
breakpoints in the request itself, as OpenAI's guide says.

### Claude

| Request                                             | Route   | Calls that read | Read share | Mean write per step |
| --------------------------------------------------- | ------- | --------------: | ---------: | ------------------: |
| No markers                                          | Bedrock |          0 of 2 |        0 % |                   0 |
| 2 markers                                           | Bedrock |        10 of 10 |       93 % |                 678 |
| 2 markers, `session_id`                             | Bedrock |        10 of 10 |       93 % |                 676 |
| 2 markers, `session_id`, `sort: price`              | Vertex  |        10 of 10 |       93 % |                 678 |
| 2 markers, `session_id`, 16 parallel calls per step | Bedrock |          6 of 6 |       86 % |               2,805 |
| 2 markers, `session_id`, `reasoning: medium`        | Bedrock |          4 of 4 |       91 % |                 674 |
| Sonnet 5.5, 2 markers, `session_id`                 | Bedrock |          3 of 3 |       91 % |                 665 |

The history marker sits on a `tool` message. It passes on both routes, and history
without reasoning blocks still caches with `reasoning` on. Sixteen parallel calls
per step did not break the 20-block lookback on Bedrock. Parallel calls were not
tried on Vertex.

### Gemini 3.8 Flash

| Request                             | Route              | Calls that read | Read share |
| ----------------------------------- | ------------------ | --------------: | ---------: |
| Native shape, no `session_id`       | Vertex (ZDR)       |          1 of 4 |       16 % |
| **Native shape, `session_id`**      | **Vertex (ZDR)**   |     **8 of 10** |   **63 %** |
| Native shape, `session_id`          | AI Studio (no ZDR) |         9 of 10 |       75 % |
| Same request repeated, `session_id` | Vertex (ZDR)       |         9 of 10 |       71 % |
| Same request repeated, `session_id` | AI Studio (no ZDR) |          8 of 8 |       81 % |
| Native shape, 2 markers             | Vertex (ZDR)       |          0 of 4 |        0 % |
| `system` marker only, `session_id`  | Vertex (ZDR)       |          6 of 6 |       67 % |

Two markers read nothing: OpenRouter uses the last one, and it moves every step.
No markers with `session_id` is the best fit, because it reads the growing history
and writes cost nothing.

### DeepSeek V4.1 Flash on third-party hosts

| Request                                   | Hosts               | Host changes | Calls that read | Read share |
| ----------------------------------------- | ------------------- | -----------: | --------------: | ---------: |
| Native shape, no `session_id`             | InferenceNet, Morph |            1 |          8 of 8 |       89 % |
| Native shape, `session_id`                | Morph               |            0 |          8 of 8 |       89 % |
| Native shape, `session_id`, `sort: price` | Relace              |            0 |          8 of 8 |       87 % |

The host changed on 40 of 41 calls in the Legacy Runs measured earlier, and on 1 of
9 here. OpenRouter's default sticky key includes the first user message, which
changes every step under Legacy and stays fixed under Native. `session_id` removes
the first change too. Pinning a host was not needed.

## Real Runs through `aidash`

One Run per Agent on a stack built from this change, with no cache settings in the
definitions and no `[cache_salt]` in the node settings. Registration wrote the
settings below, and every Native Run was salted with the derived key. Each Run did the
same small task with core tools. Spend was 0.17 credits. The per-step tables are in
[live-runs.md](live-runs.md).

| Agent    | Registered as            | Provider (changes) | Steps | Read share, step 3 on | Mean write | Mean tail | Credits |
| -------- | ------------------------ | ------------------ | ----: | --------------------: | ---------: | --------: | ------: |
| Haiku    | `prompt_cache: explicit` | Bedrock (0)        |    10 |                66.6 % |        727 |     3,219 | 0.00636 |
| Haiku    | `prompt_cache: off`      | Bedrock (0)        |    10 |                   0 % |          0 |         - | 0.01218 |
| Terra    | `prompt_cache: explicit` | Azure (0)          |    11 |                61.2 % |        437 |     2,125 | 0.03514 |
| Terra    | `prompt_cache: off`      | Azure (0)          |    10 |                   0 % |          0 |         - | 0.06524 |
| Gemini   | no markers               | Vertex (0)         |    10 |                41.4 % |          0 |     4,520 | 0.04346 |
| DeepSeek | no markers               | Wafer (0)          |    10 |                70.8 % |          0 |     2,795 | 0.00440 |

- **Haiku and Terra read their whole history.** On every step, the read equals the
  previous step's read plus its write, so nothing written was missed. Caching cost 48 %
  and 46 % less than the same Agent with it off.
- **The volatile message sets the ceiling.** The real one was 2,000 to 3,500 tokens,
  against 60 words in the probe, so these short Runs read less than the probe did. It
  is resent at full price on every step and its share falls as history grows. These
  Runs only reached 9,000 to 15,000 tokens per request, so longer Runs are untested.
- **Gemini is uneven.** Late steps read about 3,400 tokens, then none, then about 5,100,
  and 2 of 8 read nothing. It does not reliably follow the growing history, as the
  probe's 8 of 10 suggested. For Gemini and DeepSeek the tail includes history the
  provider did not read.

These Runs did not reach compaction, parallel tool calls or a cache expiry.

## What ZDR costs

Very little. The same requests without ZDR, on each provider's own route:

| Family                         | With ZDR     | Without ZDR                                                           |
| ------------------------------ | ------------ | --------------------------------------------------------------------- |
| Claude Haiku 5.5, 2 markers    | Bedrock 93 % | Anthropic 91 %                                                        |
| GPT-6.1 Terra, 3 markers       | Azure 91 %   | OpenAI 91 %                                                           |
| Gemini 3.8 Flash, Native shape | Vertex 63 %  | AI Studio 75 %                                                        |
| DeepSeek V4.1 Flash            | Morph 89 %   | First party not measured: this account's guardrails refused the route |

The Haiku pair differs in step count, not in behaviour. ZDR narrows which providers
serve a model, and the zero Gemini reads measured earlier came from the missing
`session_id`, not from ZDR. Gemini reads less often on Vertex than on AI Studio.

## Limits

- Few calls per arm, one run, and short prompts. These are behaviours, not rates to
  plan costs on.
- Provider routing was not controlled except where the table names a route.
- Real volatile messages may be larger than these 60 words, which lowers read share
  and raises the gap between the implicit and explicit Terra requests.
- Cache lifetimes were not tested. All calls ran within a few minutes of each other.

## Reproduce

```sh
export OPENROUTER_API_KEY=...   # never recorded
python3 -I probe.py results.jsonl          # every arm, stops at 2 credits
python3 -I probe.py results.jsonl terra    # arms whose name starts with "terra"
```

## Archived records

The data files below left the working tree with #858. They stay in Git history at commit `268497a929`; each link opens the file there, and the SHA-256 is of the file as committed.

| File                                                                                                                                                                    | SHA-256                                                            |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| [`results.json`](https://github.com/kent8192/aidash/blob/268497a929bf5e32cc866939fef6b5e134950793/docs/operations/evidence/2026-11-07-prompt-cache-routes/results.json) | `357b80da2cd46a966f968d321c8dcd3f720e2c0dca948fe364d9b8c1454a8d77` |
