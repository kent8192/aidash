# Request Estimates use exact tokenizers or calibration snapshots pinned per step, bounded by the byte estimate

Decided on 2026-10-09 (#116, #179). The Request Estimate covers every rendered message, the tool schemas, the media allowances, the framing and the output reservation. It comes from one of three estimators:

- **Exact**: a tokenizer the model definition names explicitly.
- **Calibrated**: a ratio of reported `prompt_tokens` to the byte estimate, kept per `(model, projection version)` over the latest 200 complete Usage Records. It takes the 99th percentile, adds a 10% safety margin and is capped at the byte estimate. With fewer than 20 samples it falls back to bytes.
- **Conservative**: today's byte estimate, used for unknown models.

Each step records the estimate snapshot it used: estimator, version, ratio and margin. A resumed or replayed step reuses that snapshot.

## Considered Options

- Enforce with bytes only and use calibrated values for diagnostics. Rejected: the byte estimate overstates typical text several times over, which wastes most of the context window.
- Enforce with a live calibrated value. Rejected: the estimate would drift between a step and its resume, so fit and compaction decisions would not be reproducible.
- Calibrate per Tenant. Rejected: tokenization is a property of the model, and per-Tenant samples would converge more slowly without making anything safer.

## Consequences

- A calibrated estimate is never less conservative than today, because it is capped at bytes.
- Calibration data is node-wide. Diagnostics expose only aggregates, never per-sample Run or Tenant identifiers. A remote Run uses the executing node's calibration.
- A Context Overflow (#113) that passed preflight is an estimator defect. Its Usage Record keeps the estimate that was used, so it can be measured.
- New estimators are enabled per model definition version, because they can change when compaction happens.

## Revised 2026-10-13

A design review of #179 against ADRs 0028, 0029, 0038 and 0040 changed the following:

- **Exact is deferred.** Calls go only through OpenRouter, and only OpenAI families publish a tokenizer. A tokenizer cannot see chat templates or tool-schema rendering, so it is not exact for a whole request, and calibration reaches 20 samples within one Run. The port keeps the `exact` confidence; the tokenizer itself is a follow-up issue.
- **"Node-wide" now means Deployment-wide.** A Node now sits inside one Tenant (ADR 0028), and Registry identifiers are scoped to a Tenant (ADR 0029). The calibration pool is therefore Deployment-wide and keyed by the upstream model slug and the Projection Version, not by a Registry definition.
- **Only the byte basis is scaled.** The byte basis is the serialized request body plus the cache-salt reserve. The media allowance, output reservation and framing are added unscaled. A sample is `prompt_tokens / byte basis`, taken only from complete, media-free Inference Attempts of Run steps. Decision Attempts and Summary Stage requests are excluded.
- **Samples live in a Deployment-wide table.** It holds no Run, Tenant or attempt identifiers and is written in the same transaction that completes the Usage Record. This makes the aggregates-only rule structural and needs no Deployment Scope under row-level security (ADR 0040).
- **A Context Overflow demotes the Run.** If an overflow follows a calibrated estimate, the step's Estimate Snapshot becomes conservative for the rest of the Run, in the same save as #113's recovery state. #113's window shrink still applies.
- **Run-fixed quotas stay in bytes.** The Ordered stable quota and the semantic budget never follow the estimator, so the Stable Prefix and the Retrieval Key never change with calibration.
- **Opt-in through the model definition.** A model definition opts in with `request_estimator: calibrated`. When the field is omitted the estimator is `bytes`. Registration writes `calibrated` into new versions.

## Revised 2026-10-17

A design review of #276 reversed the deferral of the exact estimator and renamed it **Tokenized**. Two premises of the 2026-10-13 deferral were wrong. Calls no longer go only through OpenRouter: Plan Connections (#159) call OpenAI directly, and Keyless endpoints (ADR 0036) can serve open-weight models. OpenAI is also not the only family with a published tokenizer: Google's own `google-genai` SDK maps Gemini 3.1 and later to the Apache-2.0 Gemma 4 tokenizer.

- **Token Encodings form a closed, code-defined list.** A model definition names one with `request_estimator: {"tokenized": "<encoding>"}`. Registration checks only that the name is on the list. It never compares the name with the model slug, because Keyless slugs are free-form and a slug check would be inference from the identifier. The web form offers the encoding as an optional choice and never preselects one.
- **One engine, two encodings.** Counting uses the Hugging Face `tokenizers` crate, built in pure Rust (`fancy-regex`, no `onig` and no C++ dependencies). The first encodings are o200k, from the Apache-2.0 gpt-oss `tokenizer.json`, and Gemma 4, for Gemini 3.1 and later. Gemma 3 and Llama tokenizers are excluded while their files are gated. Further families are added one list entry at a time when they are used.
- **The files are embedded.** Each `tokenizer.json` is pinned by SHA-256, compressed with zstd, embedded in the server binary and decompressed on first use. No network or file layout is needed at runtime, which also fits the egress rules of ADR 0054. Its Apache-2.0 notice is in `THIRD_PARTY_NOTICES.md`.
- **A tokenized model always counts.** The Calibrated estimator does not replace it once samples exist. The calibrated p99 ratio follows the worst content in the Deployment, and the count does not. Calibration samples are still collected.
- **What is counted.** The count covers the strings the model sees: content, decoded once, tool names, tool-call arguments and tool-schema JSON. Per-message and per-tool constants are added on top of the fixed framing reserve, because a counted body no longer absorbs the framing implicitly the way the byte basis does. The estimate stays capped at the byte estimate, and its confidence is `tokenized`, not `exact`.
- **Adversarial input.** Text longer than a fixed cap (16 KiB, set by benchmark) is counted in pieces of at most the cap, split where whitespace starts, so no single pass sees more than the cap. Each piece's count is cached, so an unchanged piece is not counted twice. The estimate stays capped at the byte estimate and deterministic across resumes. A time budget was rejected because it would make the Estimate Snapshot depend on timing. The review first chose to count an over-cap string as its byte length. Implementation showed that this would defeat the estimator: a Legacy or Ordered context is a single string far larger than any useful cap. The benchmark also found no superlinear input; 1 MiB of adversarial text took 0.1–0.4 s at any piece size.
- **Byte quotas still use a ratio.** The Ordered stable quota and the volatile budget convert tokens to bytes at the key's calibrated ratio, or 1:1 without one, and that ratio is pinned in the snapshot. Only the final fit of the complete request uses the token count.
- **Demotion widens.** A tokenized or calibrated snapshot becomes conservative (bytes) for the rest of the Run in two cases. After a Context Overflow, it applies from the same step. When the reported `prompt_tokens` exceed the input side of the estimate, it applies from the next step. The second case is the runtime check for a wrong or outdated encoding. A tokenized snapshot demotes to bytes, not to calibrated, because content that defeats the count can defeat the ratio too.
- **Missing encodings on resume.** If a resumed step's pinned encoding or encoding version is absent from the running binary, that step runs on bytes. The next step pins the current version.
- **Verification.** Fixtures compare counts with the reference implementations: `tiktoken`'s `o200k_base` and `google-genai`'s `LocalTokenizer`. Generated outputs are committed, so CI needs no Python. Credential-gated live tests on the OpenAI and Gemini routes check that the input side of the estimate is at least the reported `prompt_tokens`, and they also tune the per-item constants. Per-estimator error reporting moves to #181.
- The memory-policy tokenizer role (`TokenizerConfig::Utf8UpperBound`) stays model-independent and is out of scope.
