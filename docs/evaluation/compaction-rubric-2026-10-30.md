# Compaction rubric and question wording — 2026-10-30

**`aidash.compaction/3` replaces the rubric and both question sentences, and puts the rubric first in the
state.** On held-out benchmark decisions, Jev's utility at the production threshold rises from 0.58 to 0.80,
and Clef's from 0.63 to 0.92. Neither model drops a correction any more, and Jev no longer drops whole
Model Turns, which made agents read pages again. These are offline replays of recorded decisions (#633), not
new benchmark Runs.

## Data

The #122 context-recovery Runs of 2026-10-29 and 2026-10-30 (`fixed-task-corpus` live v1–v3 and the #611
A/B) left 503 Decision Evidence records, all from `typesafe/jev-1.14` at threshold 0.5. State retention was
off, so the states were rebuilt from each Run's Context Journal. The branches of every applied decision were
replayed as `pruning::apply` does, and `build_stage` was ported. 331 rebuilt states match the recorded
`state_digest` byte for byte, and only those are used: 322 with questions and 2,287 candidates. The other 172
diverge after a `compaction.fit/1` truncation or an adopted Summary, or belong to
`terminal-context-overflow`, which has no correction page.

Labels come from the corpus, not from a model. In `correction-lost-in-pruned-tool-result` and
`summary-recovery-correction-retention`, page 2 carries the correction, so its full result must stay. Every
other page is repeated filler. For filler, the right answer is to keep the call and cut the result. A drop
removes the whole turn, the agent no longer sees that it read the page, and #122 recorded Runs that read
dropped pages again.

| Candidate  | keep | truncate | drop |
| ---------- | ---: | -------: | ---: |
| correction |  1.0 |      0.5 |  0.0 |
| filler     |  0.0 |      1.0 |  0.5 |

The score is the mean of the two class means. A truncated correction scores 0.5. Its head holds the
correction in this corpus, but nothing guarantees that in general.

The states were split by Run: train 212, val 63, test 47. Replaying the recorded requests reproduced the
recorded answers within Jev's noise: mean difference 0.013, standard deviation 0.03 across repeats.

## Findings on the recorded decisions

- Jev answered 80 percent of its probabilities between 0.41 and 0.59. It kept 50 percent of filler results
  verbatim and dropped 31 percent. It separated correction pages from filler with an AUC of 0.66, and
  dropped 7 of 244 correction results.
- **Key order matters.** serde_json sorts object keys, so builder 2 sent `goal`, `history`, then `rubric`.
  Moving the same rubric before the history lowered every probability by about 0.15. The evaluation script
  for ADR 0066 (`scripts/evaluate-decider-routes.py`) sent insertion order, so it measured a state
  the Node never sends. It now sorts keys.

## Variants (Jev, train / val)

| Variant                                         | AUC         | mean p(call) | mean p(result, filler) | score at 0.5      |
| ----------------------------------------------- | ----------- | -----------: | ---------------------: | ----------------- |
| builder 2                                       | 0.73 / 0.67 |         0.47 |                   0.50 | 0.568 / 0.584     |
| rubric first, same text                         | 0.70 / 0.69 |         0.31 |                   0.30 | 0.249 / 0.288     |
| new rubric                                      | 0.74 / 0.74 |         0.60 |                   0.60 | 0.511 / 0.517     |
| new question sentences                          | 0.78 / 0.76 |         0.63 |                   0.58 | 0.558 / 0.544     |
| **new rubric, new sentences, rubric first (3)** | 0.70 / 0.70 |     **0.91** |               **0.44** | **0.755 / 0.706** |

Only builder 3 separates the call answer from the result answer. A GEPA search seeded with it
(`gepa` 0.6, reflection by `openai/gpt-6.1-sol`, 82 iterations, banned corpus words) found candidates that
scored up to 0.780 on val. None beat builder 3 on train or test, so its val gain was selection noise.

## Held-out test (47 states, 2 repeats)

| Route (threshold) | Builder | AUC   | Score | Filler kept / truncated / dropped | Correction kept / truncated / dropped |
| ----------------- | ------- | ----- | ----- | --------------------------------- | ------------------------------------- |
| Jev 1.14 (0.5)    | 2       | 0.755 | 0.576 | 340 / 14 / 190                    | 49 / 1 / 2                            |
| Jev 1.14 (0.5)    | 3       | 0.826 | 0.801 | 149 / 395 / 0                     | 39 / 13 / 0                           |
| Clef (0.15)       | 2       | 0.904 | 0.631 | 259 / 1 / 284                     | 52 / 0 / 0                            |
| Clef (0.15)       | 3       | 0.950 | 0.918 | 89 / 454 / 1                      | 52 / 0 / 0                            |

Builder 3 is stable across thresholds: Jev scores 0.68 to 0.78 between 0.45 and 0.65 on train and val. The
Route limit of 0.5 is close to its best. On Clef, 0.15 scores 0.862 and 0.2 scores 0.869 on train and val,
so the Route keeps 0.15 ([thresholds](../decisions/decider-route-thresholds.md)).

## Limits

- Jev still cannot read result bodies. On Jev, builder 3 truncates 13 of 52 correction results on test and 70 of 135 on train and val. That is
  harmless here because the correction opens the page; a correction deep in a result would be cut.
- One corpus, one tool and two task shapes. On the six synthetic sessions of
  `scripts/decider_route_fixtures.py`, builder 3 keeps calls, cuts most results, keeps the results an
  answer depends on (aggregates, the release notes being summarized, crash logs), and drops an unrelated page.
  Builder 2 dropped almost all of them.
- Totals: USD 2.75 of OpenRouter spend, mostly GEPA reflection and Clef. Raw answers stay local, under
  `private/jev-rubric-633/`. The aggregates are in
  [`compaction-rubric-2026-10-30.json`](compaction-rubric-2026-10-30.json).
