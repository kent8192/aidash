# Live context recovery and memory supply — 2026-10-29

**Memory supply: works.** In live Runs, automatic and on-demand recall delivered every required Unit at its
current revision, and no Run read a superseded or deleted Unit (15/15 each).

**Context recovery: correct but rarely finishes.** Jev-driven pruning and the Summary Stage both ran. When a
Run finished, it kept the corrected constraints. Most Runs still paused, because of how the window, the
protected tail and Jev's probabilities interact. These are diagnostic results (n = 3 per task), not a quality
claim.

## Setup

- Node: a local `aidash-cloud-local` debug build at the #534 integration commit `137b8d22d1`, which is `develop/0.1.0`
  `42120ef2ee` plus #534. It runs with `RUST_MIN_STACK=16777216` (#597). Another live evaluation shared the
  Tenant at the same time.
- Main model: DeepSeek V4.1 Flash (`deepseek/deepseek-v4.1-flash-1017`) through OpenRouter, pinned to
  `deepinfra/fp8/priority`, with medium reasoning. The small window is 62,000 tokens with 1,024 output tokens.
- Decider: `bench-jev-compaction@1.0.0`. It is the built-in compaction Decider limited to its first Route,
  `typesafe/jev-1.14` through System One, with a threshold of 0.5 and at most 20 calls per Run.
- Summary model: the same DeepSeek model with `reasoning_effort: none`, at most 8 calls per Run.
- Approval: `external_compaction` was bounded with `cost_enforcement: "declared-reservation-accepted"`
  ([evaluation](../evaluation.md)).
- Total provider-reported spend: about USD 0.24 for inference and USD 0.005 for 57 Jev decisions.

## Memory supply (`memory-supply-v1`, SHA-256 `fa20ab85…`)

`scripts/evaluate-run-memory-supply.py` ran 5 cases × 3 arms × 3 repeats, with the standard 128k window.

| Arm         | Passed | Correct answer | Required Units read | Stale or deleted reads |
| ----------- | -----: | -------------: | ------------------: | ---------------------: |
| `auto`      |  15/15 |             15 |                  15 |                      0 |
| `on-demand` |  15/15 |             15 |                  15 |                      0 |
| `none`      |   3/15 |              3 |                   — |                      0 |

The control passes only `deleted-fact`, whose expected answer is UNKNOWN. Retrieval used the fixture's 8-dimension
hash embedding, which is keyword-driven, so these numbers do not measure semantic recall quality.

Findings:

- A debug Node overflows tokio's default 2 MiB worker stack on the first memory-bound Run (#597, fix PR #598).
- An Agent without a memory binding is offered `memory_recall`. Calling it pauses the Run with
  `execution authority denied` (#600). The control arm removes these builtins.

## Context recovery (`corpus-context-recovery.toml`)

The five long-context tasks of `fixed-task-corpus-v2` ran on the native arm. Live models request every reference
page in one Model Turn, and Native protects that turn as a whole, so no Decider is ever asked. The corpus
therefore tells the model to read one page per turn.

### v1: 8–12 pages of 6,000 bytes (SHA-256 `5fc3e772…`)

| Task                                    | Outcomes (n = 3)                            | Compactions |
| --------------------------------------- | ------------------------------------------- | ----------: |
| `actual-pruning-correction-retention`   | 2 success (constraints kept), 1 unreducible |          13 |
| `long-conversation-corrections`         | 3 unreducible                               |           4 |
| `correction-lost-in-pruned-tool-result` | 3 unreducible, 0 Decider calls              |           0 |
| `summary-recovery-correction-retention` | 3 `summary_invalid` (oversized, unproven)   |           0 |
| `terminal-context-overflow`             | 3 unreducible (expected)                    |           0 |

### v2: 20 pages of 3,000 bytes, Summary up to 4,096 tokens (SHA-256 `8d4ee36e…`)

| Task                                    | Outcomes (n = 3)                                                   | Compactions |
| --------------------------------------- | ------------------------------------------------------------------ | ----------: |
| `long-conversation-corrections`         | 1 success (constraints kept), 1 unreducible, 1 step limit          |           6 |
| `actual-pruning-correction-retention`   | 1 constraints kept but reply not JSON, 1 unreducible, 1 step limit |           8 |
| `summary-recovery-correction-retention` | 2 step limit after an adopted Summary, 1 `summary_invalid`         |           7 |
| `correction-lost-in-pruned-tool-result` | 2 unreducible, 1 infrastructure (`/api/state` 504)                 |           2 |
| `terminal-context-overflow`             | 3 unreducible (expected)                                           |           0 |

### v3: the same, with 40 steps (SHA-256 `8a8e0a55…`, release build `hc534r`)

| Task                                    | Outcomes (n = 3)                                                                                      | Compactions |
| --------------------------------------- | ----------------------------------------------------------------------------------------------------- | ----------: |
| `actual-pruning-correction-retention`   | 2 success (constraints kept), 1 unreducible                                                           |           9 |
| `summary-recovery-correction-retention` | 1 success (constraints kept), 1 step limit, 1 `summary_invalid` (`changed_item`); 4 Summaries adopted |          16 |
| `long-conversation-corrections`         | 1 unreducible, 2 step limit                                                                           |          13 |
| `correction-lost-in-pruned-tool-result` | 3 unreducible                                                                                         |          13 |
| `terminal-context-overflow`             | 3 unreducible (expected)                                                                              |           0 |

3 of the 12 recoverable Runs finished, and all 3 kept the corrected constraints. Six paused with
`context_unreducible` after 2–6 compactions. Two `long-conversation` Runs and one `summary-recovery` Run
reached 40 steps: after pruning, the model read the dropped pages again. Jev made 57 decisions for USD 0.0061.

### Why Runs pause

1. **The protected tail fills the window.** `preserve_recent` is at least 6 Model Turns, and six 6 KB results
   fill almost all of a 62k window. Each step then has about one pruning candidate. A single "keep" answer leaves
   nothing to remove, and the Run pauses with `context_unreducible`.
2. **Jev's answers cluster around the threshold.** In v2, 126 of 460 keep probabilities were ≥ 0.5. Per task,
   the range ran from 0.30 to 0.76. Most answers sit between 0.40 and 0.55, so whether a page is dropped often
   depends on a few hundredths.
3. **Jev ranks the history, but it does not read it.** The compaction state withholds tool result bodies; Jev
   sees the tool name, its input, the status and the character count. In the two tasks whose correction sits on
   page 2, that page still received the highest keep probability in most decisions (up to 0.76). The likely cue
   is its different length, not its content.
4. **Summary needs room and a non-reasoning model.** With reasoning enabled, the 1,024-token summary was
   truncated. A summary above `max_tokens` is rejected as `oversized`, and an unproven verification as
   `unproven_outcome`. With 4,096 tokens and reasoning off, Summaries were adopted.

The ranking in finding 3 suggests a rule that drops candidates in ascending keep probability until the request
fits. The present per-candidate threshold leaves no room for that. This is a proposal, not a measured result.
