# Transaction review repair evidence

This focused Kubernetes run uses two Nodes and repeats the corrected peer-recovery lifecycle three times. Each trace records transport disablement, an unsuccessful recovery attempt after removing the process fault, restoration of the same peer with admission trust still disabled, and successful convergence. The revised oracle rejects old API state after any participant exposes the new revision. This is not full release acceptance, and no new k3s result is claimed.

`summary.json` records the image, source fingerprint, driver/query-generator hashes, local checks and maximum convergence after required service restoration. `source-inputs.json` matches the repaired runtime files in this PR. The image was built from uncommitted repairs above the recorded merge revision; that base commit alone is not the tested implementation.

`cases.jsonl.gz` preserves the original case records. Its decompressed SHA-256 is recorded in the summary. No fixture credential values are included. Earlier records and their limitations remain in [the evidence register](../../transaction-acceptance.md).

## Archived records

The data files below left the working tree with #858. They stay in Git history at commit `268497a929`; each link opens the file there, and the SHA-256 is of the file as committed.

| File                                                                                                                                                                               | SHA-256                                                            |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| [`cases.jsonl.gz`](https://github.com/kent8192/aidash/blob/268497a929bf5e32cc866939fef6b5e134950793/docs/operations/evidence/2026-09-29-transaction-review/cases.jsonl.gz)         | `0625a4f35c2aa1231e49f7589905791102532ebbb04913ae7c4e5a2831bd354f` |
| [`source-inputs.json`](https://github.com/kent8192/aidash/blob/268497a929bf5e32cc866939fef6b5e134950793/docs/operations/evidence/2026-09-29-transaction-review/source-inputs.json) | `396d9bdce6024f802a5350741eb38042d4e33fa2b64176a89943ce20a1f3049d` |
| [`summary.json`](https://github.com/kent8192/aidash/blob/268497a929bf5e32cc866939fef6b5e134950793/docs/operations/evidence/2026-09-29-transaction-review/summary.json)             | `fdb59c057e11abbeca12d35ad7281c30b34a16b00a35a2ca0170c926d8978c55` |
