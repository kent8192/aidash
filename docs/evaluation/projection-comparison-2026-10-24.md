# Projection comparison evidence — 2026-10-24

**Decision: inconclusive; keep Legacy.** No live-provider experiment, deployment confirmation or default switch was performed. Scripted fixtures exercise delivery, accounting and oracle behavior; they do not measure model quality, real provider tokenization, cache efficiency or price savings.

## Exact source and configuration

- Tested clean source: `4020e14ebc804620fb16e214d25d43352e2de2c7`.
- Executable SHA-256: `64f1e6ffd2c02e9dddfb4a8f621e7949c4ccdf01e8309ad27717bc8eac5455d0`.
- Corpus: `fixed-task-corpus-v2`, SHA-256 `b25486371e8165f241e7283236321984547ab7b89d205687b733d299177ee99b`.
- Evaluator: `2.1.4`, SHA-256 `19a92df85ea1ab034a846b8244615d670ea39b37555a4f5f8812301cfc6c6526`.
- Campaign configuration SHA-256: `d69922b99893667bc81027355d28641d8e31d7eecadf56f8bf7103d5298336e2`.
- Configuration, prompt-free Run evidence, corpus/campaign file fingerprints and the fixed-evaluator digest: [JSON artifact](projection-comparison-fixture-evidence.json).
- Protocol: [accepted decisions](../decisions/projection-comparison-protocol.md). Campaign workflow and migration/compatibility details: [comparison guide](../projection-comparison.md).

```sh
python3 scripts/evaluate-projections.py plan \
  --output private/evaluation/issue-181-pilot-plan-v5.json
python3 scripts/evaluate-fixed-tasks.py deterministic \
  --binary /path/to/manage --comparison projection --comparison explicit-projection \
  --repeats 1 --parallel 4 --skip-memory --skip-rust-evidence --run-timeout 600 \
  --output private/evaluation/issue-181-fixture-matrix-v8.json
```

The fixture execution uses one repetition and four concurrent Runs with a 600-second limit. Earlier shorter-limit trials were interrupted and are not pooled; their artifact hashes and diagnostic status are retained in the JSON artifact. Request-level estimator variance below describes heterogeneous synthetic requests, not task-level repeat variance. Task-level repeat variance is unavailable; ten repetitions per task/arm remain required for a live pilot. The frozen live configuration uses Bytes, medium reasoning, seed 181, all three automatic-cache arms and all five explicit-cache arms. The explicit treatment preserves the same task window/output limits; it does not replace the small-window task with the standard window.

Both v2 fixture and live Agents use the recorded `minimal-task-support/1` builtin policy: removable unrelated builtins are excluded, while bound tools, required core tools and Skill support remain. The archived v1 corpus retains its historical defaults. A server-reported mandatory-context block is cancelled and observed through terminal acknowledgement; its original local overflow cause is preserved. This is not a provider context rejection.

## Deterministic task outcomes

| Comparison          | Arm              | Attempted | Outcome counts                                                                                 | Compactions observed | Terminal overflow outcomes |
| ------------------- | ---------------- | --------: | ---------------------------------------------------------------------------------------------- | -------------------: | -------------------------: |
| explicit-projection | legacy           |        11 | false_completion=1, overflow_failure=3, policy_compliant_escalation=1, success=5, unresolved=1 |                    0 |                          3 |
| explicit-projection | native           |        11 | false_completion=1, overflow_failure=3, policy_compliant_escalation=1, success=5, unresolved=1 |                    0 |                          3 |
| explicit-projection | native-explicit  |        11 | false_completion=1, overflow_failure=3, policy_compliant_escalation=1, success=5, unresolved=1 |                    0 |                          3 |
| explicit-projection | ordered          |        11 | false_completion=1, overflow_failure=3, policy_compliant_escalation=1, success=5, unresolved=1 |                    0 |                          3 |
| explicit-projection | ordered-explicit |        11 | false_completion=1, overflow_failure=3, policy_compliant_escalation=1, success=5, unresolved=1 |                    0 |                          3 |
| projection          | legacy           |        11 | false_completion=1, overflow_failure=3, policy_compliant_escalation=1, success=5, unresolved=1 |                    0 |                          3 |
| projection          | native           |        11 | false_completion=1, overflow_failure=3, policy_compliant_escalation=1, success=5, unresolved=1 |                    0 |                          3 |
| projection          | ordered          |        11 | false_completion=1, overflow_failure=3, policy_compliant_escalation=1, success=5, unresolved=1 |                    0 |                          3 |

The 88 attempted fixture Runs had 48 calibration check mismatches across 24 Runs; 64 Runs matched calibration. The command exited 1 for these mismatches, with no experiment-level infrastructure error. Expected and observed details are retained in the JSON artifact; the fixture matrix does not pass calibration. 16 planned Runs were explicitly skipped. No skipped case is counted as a pass.

Current mandatory-context blocks are recorded as local `overflow_failure` outcomes after confirmed cancellation. Original corpus expectations are retained, including `unresolved` long-context expectations; they were not changed after observing this trial. Any resulting calibration mismatch remains a prerequisite to resolve before using the campaign for adoption.

The automatic pruning and Summary-retention tasks require a production-routable bounded Decider and are not executed by the keyless Python fixture runner. Actual native Summary and generation-compaction Rust tests are separate evidence, not substituted live task outcomes. Per-task, per-Run outcomes are retained in the JSON artifact.

## Fixture usage and estimator measurements

Every number below is reported by the local scripted fixture and the usage ledger. It is not a production-provider measurement. The fixture manifest carries inference Usage Records, not a complete six-table evidence export: total provider-work cost and auxiliary-work completeness remain unknown. The charge column totals only observed inference records. Cache counters are breakdowns and are not added to input/output totals; upstream charges and reservations are not added to billed costs.

| Comparison          | Arm              | Usage records | Input  | Output | Cache read | Cache write | Observed inference fixture charge (nanocredits) |
| ------------------- | ---------------- | ------------: | ------ | ------ | ---------- | ----------- | ----------------------------------------------- |
| explicit-projection | legacy           |            36 | 626919 | 3845   | 0          | 0           | 64614400                                        |
| explicit-projection | native           |            34 | 570174 | 3743   | 0          | 0           | 58888900                                        |
| explicit-projection | native-explicit  |            34 | 575700 | 3743   | 0          | 371972      | 68740800                                        |
| explicit-projection | ordered          |            35 | 577686 | 3794   | 0          | 0           | 59665600                                        |
| explicit-projection | ordered-explicit |            35 | 582693 | 3794   | 0          | 393707      | 70008975                                        |
| projection          | legacy           |            36 | 625883 | 3845   | 0          | 0           | 64510800                                        |
| projection          | native           |            34 | 569266 | 3743   | 0          | 0           | 58798100                                        |
| projection          | ordered          |            35 | 576727 | 3794   | 0          | 0           | 59569700                                        |

Selected and shadow estimate observations are reported separately; identical Bytes shadows are not additional independent requests.

| Comparison          | Arm              | Estimator/role    | Known errors |          Unknown errors | Mean signed input error | Sample variance    |
| ------------------- | ---------------- | ----------------- | -----------: | ----------------------: | ----------------------- | ------------------ |
| explicit-projection | legacy           | bytes/selected    |           36 |                       0 | 900.1388888888889       | 1059.6087301587302 |
| explicit-projection | legacy           | bytes/shadow      |           36 |                       0 | 900.1388888888889       | 1059.6087301587302 |
| explicit-projection | legacy           | calibrated/shadow |            0 | not configured/measured | unknown                 | unknown            |
| explicit-projection | legacy           | tokenized/shadow  |            0 | not configured/measured | unknown                 | unknown            |
| explicit-projection | native           | bytes/selected    |           34 |                       0 | 988.0294117647059       | 1879.362745098039  |
| explicit-projection | native           | bytes/shadow      |           34 |                       0 | 988.0294117647059       | 1879.362745098039  |
| explicit-projection | native           | calibrated/shadow |            0 | not configured/measured | unknown                 | unknown            |
| explicit-projection | native           | tokenized/shadow  |            0 | not configured/measured | unknown                 | unknown            |
| explicit-projection | native-explicit  | bytes/selected    |           34 |                       0 | 861.0882352941177       | 2598.628342245989  |
| explicit-projection | native-explicit  | bytes/shadow      |           34 |                       0 | 861.0882352941177       | 2598.628342245989  |
| explicit-projection | native-explicit  | calibrated/shadow |            0 | not configured/measured | unknown                 | unknown            |
| explicit-projection | native-explicit  | tokenized/shadow  |            0 | not configured/measured | unknown                 | unknown            |
| explicit-projection | ordered          | bytes/selected    |           35 |                       0 | 1015.7428571428571      | 1084.9613445378152 |
| explicit-projection | ordered          | bytes/shadow      |           35 |                       0 | 1015.7428571428571      | 1084.9613445378152 |
| explicit-projection | ordered          | calibrated/shadow |            0 | not configured/measured | unknown                 | unknown            |
| explicit-projection | ordered          | tokenized/shadow  |            0 | not configured/measured | unknown                 | unknown            |
| explicit-projection | ordered-explicit | bytes/selected    |           35 |                       0 | 907.7428571428571       | 1084.9613445378152 |
| explicit-projection | ordered-explicit | bytes/shadow      |           35 |                       0 | 907.7428571428571       | 1084.9613445378152 |
| explicit-projection | ordered-explicit | calibrated/shadow |            0 | not configured/measured | unknown                 | unknown            |
| explicit-projection | ordered-explicit | tokenized/shadow  |            0 | not configured/measured | unknown                 | unknown            |
| projection          | legacy           | bytes/selected    |           36 |                       0 | 900.1388888888889       | 1059.6087301587302 |
| projection          | legacy           | bytes/shadow      |           36 |                       0 | 900.1388888888889       | 1059.6087301587302 |
| projection          | legacy           | calibrated/shadow |            0 | not configured/measured | unknown                 | unknown            |
| projection          | legacy           | tokenized/shadow  |            0 | not configured/measured | unknown                 | unknown            |
| projection          | native           | bytes/selected    |           34 |                       0 | 988.0294117647059       | 1879.362745098039  |
| projection          | native           | bytes/shadow      |           34 |                       0 | 988.0294117647059       | 1879.362745098039  |
| projection          | native           | calibrated/shadow |            0 | not configured/measured | unknown                 | unknown            |
| projection          | native           | tokenized/shadow  |            0 | not configured/measured | unknown                 | unknown            |
| projection          | ordered          | bytes/selected    |           35 |                       0 | 1015.7428571428571      | 1084.9613445378152 |
| projection          | ordered          | bytes/shadow      |           35 |                       0 | 1015.7428571428571      | 1084.9613445378152 |
| projection          | ordered          | calibrated/shadow |            0 | not configured/measured | unknown                 | unknown            |
| projection          | ordered          | tokenized/shadow  |            0 | not configured/measured | unknown                 | unknown            |

| Comparison          | Arm              | First-attempt cache read/write | Later-attempt cache read/write |
| ------------------- | ---------------- | ------------------------------ | ------------------------------ |
| explicit-projection | legacy           | 0 / 0                          | 0 / 0                          |
| explicit-projection | native           | 0 / 0                          | 0 / 0                          |
| explicit-projection | native-explicit  | 0 / 31744                      | 0 / 340228                     |
| explicit-projection | ordered          | 0 / 0                          | 0 / 0                          |
| explicit-projection | ordered-explicit | 0 / 25782                      | 0 / 367925                     |
| projection          | legacy           | 0 / 0                          | 0 / 0                          |
| projection          | native           | 0 / 0                          | 0 / 0                          |
| projection          | ordered          | 0 / 0                          | 0 / 0                          |

| Comparison          | Arm              | Runs with known Prefix transitions | Total observed transitions | Runs with unknown transitions | N/A Runs |
| ------------------- | ---------------- | ---------------------------------: | -------------------------- | ----------------------------: | -------: |
| explicit-projection | legacy           |                                  0 | N/A                        |                             0 |       11 |
| explicit-projection | native           |                                 11 | 2                          |                             0 |        0 |
| explicit-projection | native-explicit  |                                 11 | 2                          |                             0 |        0 |
| explicit-projection | ordered          |                                 11 | 2                          |                             0 |        0 |
| explicit-projection | ordered-explicit |                                 11 | 2                          |                             0 |        0 |
| projection          | legacy           |                                  0 | N/A                        |                             0 |       11 |
| projection          | native           |                                 11 | 2                          |                             0 |        0 |
| projection          | ordered          |                                 11 | 2                          |                             0 |        0 |

Input estimation error is `estimated_tokens - output_reservation - reported_input_tokens`; it never compares the output reservation to input usage. The artifact retains selected estimator/version/confidence and every emitted shadow snapshot. Calibrated/Tokenized measurements require applicable, actually configured frozen snapshots; absent snapshots are unknown, not zero error. The local fixture token counts are synthetic and cannot establish estimator accuracy against a real provider.

Stable Prefix identity transitions can be reconstructed per Run from `cache_identity` and timestamps in the artifact. Legacy has no Stable Prefix identity. Identity transitions are not provider cache hits; first attempts are not asserted cold. Provider-rejection rows, terminal overflow outcomes and Run-observed compactions remain distinct. Cumulative local fitting failures and omitted compaction journal details remain unknown in this fixture manifest.

## Live-provider evidence

| Version/treatment        | Task quality and variance | Input/output/read/write tokens | Provider cost | Error per estimator | Prefix identity changes | Overflow/compaction counts |
| ------------------------ | ------------------------- | ------------------------------ | ------------- | ------------------- | ----------------------- | -------------------------- |
| Legacy                   | unknown                   | unknown                        | unknown       | unknown             | N/A                     | unknown                    |
| Ordered (cache off)      | unknown                   | unknown                        | unknown       | unknown             | unknown                 | unknown                    |
| Native (cache off)       | unknown                   | unknown                        | unknown       | unknown             | unknown                 | unknown                    |
| Ordered + explicit cache | unknown                   | unknown                        | unknown       | unknown             | unknown                 | unknown                    |
| Native + explicit cache  | unknown                   | unknown                        | unknown       | unknown             | unknown                 | unknown                    |

No Tenant/credential/disclosure/source-attestation execution manifest or complete evidence export was supplied. No paid calls were made. The pilot has 390 automatic-route and 650 explicit-route planned Runs: 1,040 sequential Runs × 180 seconds = 187,200 seconds (52 hours), exceeding the shared 14,400-second ceiling. It must not start under this default configuration. The USD 50 ceiling is shared across preflight and all phases; phase reservations are not refunded from incomplete accepted-response events.

A bounded Decider declaration currently has no enforceable per-physical-request monetary receipt, so live auxiliary dispatch is refused. The default configuration also leaves independent-block readiness false. A future campaign must establish these prerequisites, fit the complete phase inside both ceilings, supply exact upstream/model identity evidence, and complete new frozen confirmation and deployment-estimator trials. Unknown reported endpoint identity cannot be filled from the requested route.

## Rollout and compatibility

No creation default or rollout setting changes. Legacy remains the default; existing definitions, historically omitted versions and pinned Runs retain their semantics. A later measured change needs its own supported-model creation scope and rollback controls.

The new optional persisted fields and additive migrations preserve existing JSON definitions. Public Rust construction APIs gain fields (`ModelConfig`, `UsageDispatch`, `ReportedUsage`); explicit struct literals need the new `None`/empty/default values. This PR is labeled as a Rust API breaking change.

## Verification

Verification results below concern implementation correctness only. Hosted CI and live acceptance are separate.

- `python3 -m unittest discover -s scripts -p test_fixed_task_evaluation.py`: 140 passed.
- `python3 -m unittest discover -s scripts -p "test_projection_*.py"`: 56 passed.
- Domain: 2,188 passed; inference adapters: 153 passed; Tokenized shadow checks: 3 passed.
- PostgreSQL Usage Ledger: 10 passed; registry persistence/constraint tests: 9 passed; generation-compaction tests: 7 passed; Native Summary boundary regression: 1 passed.
- Memory retry-ledger persistence: 3 passed with the CI settings `RUST_MIN_STACK=8388608 RUST_TEST_THREADS=2`.
- Scoped changed-file Trunk checks and `git diff --check` passed.
- The deterministic matrix above is a separate executable-system check; its calibration is not live model acceptance.

The initial Memory retry-ledger invocation aborted with a stack overflow under the default test-thread stack. The unchanged test binary passed all three cases with the CI stack setting above. This is a focused regression check, not a claim that the complete server library suite ran.

Issue #181 remains open. No unmeasured quality, token, caching, cost or default-readiness improvement is claimed.
