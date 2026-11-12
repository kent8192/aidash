# #776 live check: prompt caching by route, real Runs through a built `aidash`

Run 2026-11-07 (UTC 01:59 to 02:07) on Compose project `aidash-776`, Tenant `dewy-fjord-8z1b33mk`. Usage only: no prompts, no key.

Code: snapshot of `feat/issue-776-prompt-cache-by-route` at HEAD 59a91360a6 plus uncommitted diff sha256 68e0b488a519747432f722e69408ce1c0f3b8ce1bcd8f6f4ff49a1bdd6863e34 (`out/snapshot.diff`). The worktree still matched it after the Runs. No `[cache_salt]`; backend log: `Tenant Cache Salt available current_key_version=1`.

## Registration

| Definition      | Field values read back (API and registry table agree)                                         |
| --------------- | --------------------------------------------------------------------------------------------- |
| live-haiku      | model_id=anthropic/claude-haiku-5.5, cache_mode=explicit                                      |
| live-terra      | model_id=openai/gpt-6.1-terra, cache_mode=explicit                                            |
| live-gemini     | model_id=google/gemini-3.8-flash, cache_mode=<absent>                                         |
| live-deepseek   | model_id=deepseek/deepseek-v4.1-flash-1017, cache_mode=<absent>                               |
| agent-haiku     | projection_version=native, harness_contract=structured@3, prompt_cache=explicit, max_steps=12 |
| agent-terra     | projection_version=native, harness_contract=structured@3, prompt_cache=explicit, max_steps=12 |
| agent-gemini    | projection_version=native, harness_contract=structured@3, prompt_cache=<absent>, max_steps=12 |
| agent-deepseek  | projection_version=native, harness_contract=structured@3, prompt_cache=<absent>, max_steps=12 |
| agent-haiku-off | projection_version=native, harness_contract=structured@3, prompt_cache=off, max_steps=12      |
| agent-terra-off | projection_version=native, harness_contract=structured@3, prompt_cache=off, max_steps=12      |

## Per-Run summary

| Agent           | Run      | Phase     | Calls | Read share, step 3+ | Mean write, step 3+ | Volatile tail, step 3+ (mean, min-max) | Providers (changes) | Credits |
| --------------- | -------- | --------- | ----: | ------------------: | ------------------: | -------------------------------------: | ------------------- | ------: |
| agent-haiku     | 66f882e7 | COMPLETED |    10 |              66.6 % |                 727 |                       3219 (2716-3635) | Amazon Bedrock (0)  | 0.00636 |
| agent-terra     | 728588f4 | COMPLETED |    11 |              61.2 % |                 437 |                       2125 (1988-2239) | Azure (0)           | 0.03514 |
| agent-gemini    | 109df972 | COMPLETED |    10 |              41.4 % |                   0 |                       4520 (2604-7410) | Google (0)          | 0.04346 |
| agent-deepseek  | 4d150c19 | COMPLETED |    10 |              70.8 % |                   0 |                       2795 (2498-3714) | Wafer (0)           | 0.00440 |
| agent-haiku-off | 607a036b | COMPLETED |    10 |               0.0 % |                   0 |                n/a, no cache (= input) | Amazon Bedrock (0)  | 0.01218 |
| agent-terra-off | d36b618c | COMPLETED |    10 |               0.0 % |                   0 |                n/a, no cache (= input) | Azure (0)           | 0.06524 |

Total spent on this stack: 0.16678 credits

Notes:

- The tail is input minus read minus write. On Haiku and Terra that is the volatile message plus nothing else, since the prefix chain holds. On Gemini and DeepSeek it also holds history the provider did not read, so it is an upper bound on the volatile message.
- Each Run did the same task shape: a chain of four artifacts, a read-back after each, one `workspace_observe`, then the answer. Terra added one unrequested `skill_list` call (no Skills are bound), which is its 11th step.
- No step failed or was rejected, every usage row has purpose `main`, and no Run paused or failed.

## Checks per Run

| Agent           | Calls that read, step 3+ | Prefix chain holds (read n = read n-1 + write n-1) | Tail, first to last | Read share at the last step before the observe result | Reads and writes vs input, whole Run  |
| --------------- | -----------------------: | -------------------------------------------------- | ------------------- | ----------------------------------------------------: | ------------------------------------- |
| agent-haiku     |                   8 of 8 | yes                                                | 1835 to 3635        |                                                69.5 % | read 68539, write 11785, input 110342 |
| agent-terra     |                   9 of 9 | yes                                                | 1305 to 2239        |                                                66.2 % | read 38905, write 6645, input 67613   |
| agent-gemini    |                   6 of 8 | n/a, no writes reported                            | 4225 to 4937        |                                                58.8 % | read 25540, write 0, input 71166      |
| agent-deepseek  |                   8 of 8 | n/a, no writes reported                            | 6378 to 3714        |                                                73.8 % | read 59264, write 0, input 90225      |
| agent-haiku-off |                   0 of 8 | n/a, no writes reported                            | 7369 to 15201       |                                                 0.0 % | read 0, write 0, input 108493         |
| agent-terra-off |                   0 of 8 | n/a, no writes reported                            | 3899 to 8895        |                                                 0.0 % | read 0, write 0, input 62592          |

## Per-step tables

### agent-haiku

Run `66f882e7-c6f0-4e17-acd5-9117473574f8`, phase COMPLETED, control ACTIVE

| Step | Purpose | Outcome   | Input | Output | Read | Write | Reasoning | Provider       |  Credits | At (UTC) |
| ---: | ------- | --------- | ----: | -----: | ---: | ----: | --------: | -------------- | -------: | -------- |
|    1 | main    | completed |  7362 |    196 |    0 |  5527 |        92 | Amazon Bedrock | 0.000972 | 01:59:55 |
|    2 | main    | completed |  8400 |    151 | 5527 |   441 |        32 | Amazon Bedrock | 0.000429 | 02:00:04 |
|    3 | main    | completed |  9230 |    291 | 5968 |   546 |       125 | Amazon Bedrock | 0.000545 | 02:00:08 |
|    4 | main    | completed | 10015 |    246 | 6514 |   527 |       125 | Amazon Bedrock | 0.000551 | 02:00:14 |
|    5 | main    | completed | 10578 |    336 | 7041 |   556 |       159 | Amazon Bedrock | 0.000606 | 02:00:21 |
|    6 | main    | completed | 11369 |    190 | 7597 |   540 |        67 | Amazon Bedrock | 0.000562 | 02:00:28 |
|    7 | main    | completed | 11938 |    373 | 8137 |   563 |       194 | Amazon Bedrock | 0.000662 | 02:00:44 |
|    8 | main    | completed | 12733 |    198 | 8700 |   546 |        75 | Amazon Bedrock | 0.000603 | 02:00:51 |
|    9 | main    | completed | 13297 |    264 | 9246 |   563 |       177 | Amazon Bedrock | 0.000644 | 02:00:59 |
|   10 | main    | completed | 15420 |    146 | 9809 |  1976 |       121 | Amazon Bedrock | 0.000782 | 02:01:05 |

### agent-terra

Run `728588f4-580f-49b7-b650-20978426824e`, phase COMPLETED, control ACTIVE

| Step | Purpose | Outcome   | Input | Output | Read | Write | Reasoning | Provider |  Credits | At (UTC) |
| ---: | ------- | --------- | ----: | -----: | ---: | ----: | --------: | -------- | -------: | -------- |
|    1 | main    | completed |  3900 |     75 |    0 |  2595 |        59 | Azure    | 0.004924 | 02:01:31 |
|    2 | main    | completed |  4350 |     35 | 2595 |   118 |         0 | Azure    | 0.002089 | 02:01:38 |
|    3 | main    | completed |  5003 |     51 | 2713 |   294 |         0 | Azure    | 0.002754 | 02:01:45 |
|    4 | main    | completed |  5342 |     72 | 3007 |   347 |        11 | Azure    | 0.002932 | 02:01:52 |
|    5 | main    | completed |  5750 |     46 | 3354 |   338 |         0 | Azure    | 0.002878 | 02:01:56 |
|    6 | main    | completed |  6092 |     73 | 3692 |   339 |        17 | Azure    | 0.003034 | 02:02:00 |
|    7 | main    | completed |  6518 |     50 | 4031 |   332 |         0 | Azure    | 0.003022 | 02:02:07 |
|    8 | main    | completed |  6861 |     77 | 4363 |   346 |        17 | Azure    | 0.003188 | 02:02:12 |
|    9 | main    | completed |  7284 |     49 | 4709 |   339 |         0 | Azure    | 0.003140 | 02:02:21 |
|   10 | main    | completed |  7629 |     19 | 5048 |   345 |         0 | Azure    | 0.003015 | 02:02:26 |
|   11 | main    | completed |  8884 |     18 | 5393 |  1252 |         0 | Azure    | 0.004164 | 02:02:31 |

### agent-gemini

Run `109df972-df69-424c-9d8f-f8e45df3d0e5`, phase COMPLETED, control ACTIVE

| Step | Purpose | Outcome   | Input | Output | Read | Write | Reasoning | Provider |  Credits | At (UTC) |
| ---: | ------- | --------- | ----: | -----: | ---: | ----: | --------: | -------- | -------: | -------- |
|    1 | main    | completed |  4225 |    532 |    0 |     0 |       495 | Google   | 0.005164 | 02:02:41 |
|    2 | main    | completed |  5241 |     63 |    0 |     0 |         0 | Google   | 0.004167 | 02:02:47 |
|    3 | main    | completed |  5971 |    214 | 3367 |     0 |       144 | Google   | 0.003008 | 02:02:52 |
|    4 | main    | completed |  6491 |    150 |    0 |     0 |        86 | Google   | 0.005431 | 02:03:02 |
|    5 | main    | completed |  6903 |    278 | 3389 |     0 |       207 | Google   | 0.003932 | 02:03:08 |
|    6 | main    | completed |  7410 |    204 |    0 |     0 |       142 | Google   | 0.006322 | 02:03:14 |
|    7 | main    | completed |  7818 |    225 | 3405 |     0 |       156 | Google   | 0.004409 | 02:03:20 |
|    8 | main    | completed |  8318 |    157 | 5118 |     0 |        95 | Google   | 0.003373 | 02:03:27 |
|    9 | main    | completed |  8717 |    114 | 5126 |     0 |       100 | Google   | 0.003505 | 02:03:34 |
|   10 | main    | completed | 10072 |     17 | 5135 |     0 |         0 | Google   | 0.004152 | 02:03:51 |

### agent-deepseek

Run `4d150c19-2bcf-4a94-8fed-80f874347752`, phase COMPLETED, control ACTIVE

| Step | Purpose | Outcome   | Input | Output | Read | Write | Reasoning | Provider |  Credits | At (UTC) |
| ---: | ------- | --------- | ----: | -----: | ---: | ----: | --------: | -------- | -------: | -------- |
|    1 | main    | completed |  6378 |    151 |    0 |     0 |        46 | Wafer    | 0.000777 | 02:04:01 |
|    2 | main    | completed |  7216 |     98 | 4992 |     0 |         0 | Wafer    | 0.000334 | 02:04:05 |
|    3 | main    | completed |  7874 |    129 | 5376 |     0 |        13 | Wafer    | 0.000382 | 02:04:09 |
|    4 | main    | completed |  8350 |     99 | 5760 |     0 |         0 | Wafer    | 0.000380 | 02:04:13 |
|    5 | main    | completed |  8783 |    136 | 6144 |     0 |        19 | Wafer    | 0.000407 | 02:04:16 |
|    6 | main    | completed |  9268 |     94 | 6528 |     0 |         0 | Wafer    | 0.000401 | 02:04:20 |
|    7 | main    | completed |  9696 |    128 | 7040 |     0 |        15 | Wafer    | 0.000412 | 02:04:24 |
|    8 | main    | completed | 10171 |     93 | 7424 |     0 |         0 | Wafer    | 0.000408 | 02:04:28 |
|    9 | main    | completed | 10583 |    104 | 7808 |     0 |        36 | Wafer    | 0.000420 | 02:04:32 |
|   10 | main    | completed | 11906 |     13 | 8192 |     0 |         0 | Wafer    | 0.000481 | 02:04:37 |

### agent-haiku-off

Run `607a036b-6f60-4591-bae5-17970dc75001`, phase COMPLETED, control ACTIVE

| Step | Purpose | Outcome   | Input | Output | Read | Write | Reasoning | Provider       |  Credits | At (UTC) |
| ---: | ------- | --------- | ----: | -----: | ---: | ----: | --------: | -------------- | -------: | -------- |
|    1 | main    | completed |  7369 |    225 |    0 |     0 |       121 | Amazon Bedrock | 0.000849 | 02:04:44 |
|    2 | main    | completed |  8406 |    153 |    0 |     0 |        31 | Amazon Bedrock | 0.000917 | 02:04:48 |
|    3 | main    | completed |  9245 |    270 |    0 |     0 |       142 | Amazon Bedrock | 0.001059 | 02:04:53 |
|    4 | main    | completed |  9837 |    240 |    0 |     0 |       118 | Amazon Bedrock | 0.001104 | 02:05:00 |
|    5 | main    | completed | 10393 |    258 |    0 |     0 |       130 | Amazon Bedrock | 0.001168 | 02:05:07 |
|    6 | main    | completed | 10993 |    247 |    0 |     0 |       117 | Amazon Bedrock | 0.001223 | 02:05:12 |
|    7 | main    | completed | 11570 |    438 |    0 |     0 |       257 | Amazon Bedrock | 0.001376 | 02:05:17 |
|    8 | main    | completed | 12378 |    306 |    0 |     0 |       160 | Amazon Bedrock | 0.001391 | 02:05:23 |
|    9 | main    | completed | 13101 |    309 |    0 |     0 |       257 | Amazon Bedrock | 0.001465 | 02:05:30 |
|   10 | main    | completed | 15201 |    210 |    0 |     0 |       188 | Amazon Bedrock | 0.001625 | 02:05:37 |

### agent-terra-off

Run `d36b618c-4b4f-4e79-bc5a-4be1c551282a`, phase COMPLETED, control ACTIVE

| Step | Purpose | Outcome   | Input | Output | Read | Write | Reasoning | Provider |  Credits | At (UTC) |
| ---: | ------- | --------- | ----: | -----: | ---: | ----: | --------: | -------- | -------: | -------- |
|    1 | main    | completed |  3899 |     78 |    0 |     0 |        41 | Azure    | 0.004289 | 02:05:44 |
|    2 | main    | completed |  4667 |     46 |    0 |     0 |         0 | Azure    | 0.004897 | 02:05:53 |
|    3 | main    | completed |  5237 |     68 |    0 |     0 |        12 | Azure    | 0.005577 | 02:05:57 |
|    4 | main    | completed |  5669 |     48 |    0 |     0 |         0 | Azure    | 0.005909 | 02:06:02 |
|    5 | main    | completed |  6028 |     74 |    0 |     0 |        16 | Azure    | 0.006398 | 02:06:08 |
|    6 | main    | completed |  6461 |     52 |    0 |     0 |         0 | Azure    | 0.006721 | 02:06:15 |
|    7 | main    | completed |  6827 |     78 |    0 |     0 |        17 | Azure    | 0.007217 | 02:06:20 |
|    8 | main    | completed |  7268 |     49 |    0 |     0 |         0 | Azure    | 0.007513 | 02:06:26 |
|    9 | main    | completed |  7641 |     19 |    0 |     0 |         0 | Azure    | 0.007736 | 02:06:33 |
|   10 | main    | completed |  8895 |     18 |    0 |     0 |         0 | Azure    | 0.008985 | 02:06:38 |

## `aidash_model_tokens_total` (from `AIDASH_METRICS_LISTEN` inside the container; main port answers 404)

```
aidash_model_tokens_total{direction="input_cached_read",family="anthropic"} 68539
aidash_model_tokens_total{direction="input_cached_read",family="deepseek"} 59264
aidash_model_tokens_total{direction="input_cached_read",family="google"} 25540
aidash_model_tokens_total{direction="input_cached_read",family="openai"} 38905
aidash_model_tokens_total{direction="input_cached_write",family="anthropic"} 11785
aidash_model_tokens_total{direction="input_cached_write",family="deepseek"} 0
aidash_model_tokens_total{direction="input_cached_write",family="google"} 0
aidash_model_tokens_total{direction="input_cached_write",family="openai"} 6645
aidash_model_tokens_total{direction="input",family="anthropic"} 218835
aidash_model_tokens_total{direction="input",family="deepseek"} 90225
aidash_model_tokens_total{direction="input",family="google"} 71166
aidash_model_tokens_total{direction="input",family="openai"} 130205
aidash_model_tokens_total{direction="output",family="anthropic"} 5047
aidash_model_tokens_total{direction="output",family="deepseek"} 1045
aidash_model_tokens_total{direction="output",family="google"} 1954
aidash_model_tokens_total{direction="output",family="openai"} 1095
```
