# Transaction acceptance records

These local runs began on 2026-09-28 UTC. The [evidence register](../../transaction-acceptance.md) states the implementation and release boundaries. This bundle does not certify full FR-TX-001 acceptance.

`summary.json` records the implementation commit, deployment/image identities, source-input fingerprints, actual outcomes and per-case convergence. The two fingerprint-named JSON files list the individual Rust/Cargo/Dockerfile input hashes. The committed runtime inputs match the final-image fingerprint; the earlier main image differs in the peer-recovery audit record in `src/transactions/api.rs`.

Each `.jsonl.gz` file preserves a run's original append-only case records, including synthetic transaction and event IDs, ordinary HTTP observations, durable participant/revision/barrier rows, fault/restoration timestamps and failures. Decompress without modifying the archive using `gzip -dc <archive>`. Compare the decompressed SHA-256 with `uncompressed_sha256` in the summary. Credential values and protected manifest payloads are excluded from these records.

The main runs used an earlier driver. The final-image supplements verify Worker Pods reach zero before the next case; the Kubernetes three/sixteen-Node rerun also uses that assertion. Earlier setup failures and interrupted runs remain separate from successful repetitions. A setup failure with no case file supplies no protocol evidence. One successful preliminary six-repetition run is retained as a partial inventory, not a complete acceptance run.

The implemented inventory has 35 cases, each repeated three times per distribution. A successful rerun does not overwrite a failed record, and the union of passing repetitions across separately identified images does not establish release acceptance for one exact candidate. All convergence limits start after fault removal and required service restoration; they are not production RTOs.

Review limitation: these historical cluster traces predate the corrected monotonic API visibility oracle and actual peer transport disablement. Their peer-recovery cases revoked admission trust only. See [the evidence register](../../transaction-acceptance.md#review-findings-affecting-historical-evidence) for the scope correction and separate review-repair evidence.
