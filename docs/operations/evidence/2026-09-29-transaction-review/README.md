# Transaction review repair evidence

This focused Kubernetes run uses two Nodes and repeats the corrected peer-recovery lifecycle three times. Each trace records transport disablement, an unsuccessful recovery attempt after removing the process fault, restoration of the same peer with admission trust still disabled, and successful convergence. The revised oracle rejects old API state after any participant exposes the new revision. This is not full release acceptance, and no new k3s result is claimed.

`summary.json` records the image, source fingerprint, driver/query-generator hashes, local checks and maximum convergence after required service restoration. `source-inputs.json` matches the repaired runtime files in this PR. The image was built from uncommitted repairs above the recorded merge revision; that base commit alone is not the tested implementation.

`cases.jsonl.gz` preserves the original case records. Its decompressed SHA-256 is recorded in the summary. No fixture credential values are included. Earlier records and their limitations remain in [the evidence register](../../transaction-acceptance.md).
