# Third-party notices

Aidash contains material adapted from the projects below. That material keeps
its own license, and each notice is kept verbatim under `third_party/`.
Dependencies that Cargo and npm resolve are not listed here; their notices
belong to the licensing work in [#124](https://github.com/kent8192/aidash/issues/124).

Each file list says how a file was found. **Lineage** means that the file's
lines descend from a file recorded as adapted. **Content** means that the file
was found by comparing it with the upstream source.

## fast-jev-compaction

- Source: <https://github.com/tamaratran/fast-jev-compaction>
- Revision: `e3f262a7f4d42bd8dd32ced30d26176f7cb545b0`
- License: MIT, in [third_party/fast-jev-compaction/LICENSE](third_party/fast-jev-compaction/LICENSE)

Files that contain adapted material:

- `crates/aidash-application/src/context/compaction.rs`: compaction defaults, state fitting and the keep, shorten or drop decision (lineage)
- `crates/aidash-application/src/context.rs`: Jev answer validation (lineage)
- `crates/aidash-integrations/src/compaction.rs`: the Jev client and its request limits (lineage)
- `crates/aidash-integrations/src/compaction/tests.rs`: tests of the Jev client (lineage)
- `server/src/apps/execution/services/context/jev.rs`: the port that asks Jev (lineage)
- `crates/aidash-application/src/decision/state.rs`: the compaction rubric and the per-call question pair, rewritten rather than copied (content)

### How the files were traced

The root `LICENSE` used to name `src/context/compaction.rs` and
`src/context/jev.rs`. The workspace split (#102, commit `095b71090`) removed
both paths. On `develop/0.1.0` at `68c27f135`, `git blame -w -C -C -M` ran over
every Rust file that mentions Jev, compaction or decisions. A file is listed
when lines trace to one of the two paths and those lines are more than generic
code. Imports, the `reqwest` client builder and single signatures count as
generic, which excludes `crates/aidash-integrations/src/decision.rs`,
`crates/aidash-broker/src/lib.rs`, `crates/aidash-application/src/execution.rs`
and `server/src/apps/execution/generation/services/compaction.rs`.

The content search compared upstream `src/compact.ts` and `src/state.ts` at the
pinned revision with the tree. It searched for the upstream defaults (0.5, 6,
25,000, 30,000 and 300), the input caps (1,000, 200 and 60), the state text and
the question structure. It found the decision rubric in addition to the files
above.

## Hindsight

- Source: <https://github.com/vectorize-io/hindsight>
- Revision: `b89ce287464fe10790d67ba86bfabe29d8926253`
- License: MIT, in [third_party/hindsight/LICENSE](third_party/hindsight/LICENSE)

Native memory is a semantic translation of Hindsight's documented contracts and
engine, not a copy of its Python API.

Files that contain adapted material:

- `crates/aidash-domain/src/memory.rs`: memory unit, evidence and link contracts (lineage)
- `crates/aidash-domain/src/memory/recall.rs`: four-arm fusion, graph and temporal recall (lineage)
- `crates/aidash-domain/src/memory/graph.rs`: causal and semantic link creation (lineage)
- `crates/aidash-domain/src/memory/extraction.rs`: batch-local link resolution (lineage)
- `crates/aidash-domain/src/memory/consolidation.rs`: observation groups (lineage)
- `crates/aidash-application/src/memory.rs`: retain, recall, consolidate, maintain and reflect (lineage)
- `server/src/apps/knowledge/repositories/engine_jobs.rs`: durable consolidation jobs (lineage)
- `server/src/apps/knowledge/repositories/engine_jobs/consolidation.rs`: observation regeneration (lineage)
- `server/src/apps/knowledge/repositories/learning.rs`: Run learning candidates (lineage)
- `server/src/apps/knowledge/services/memory_models.rs`: engine model calls (lineage)
- `server/src/apps/knowledge/repositories/native_memory.rs`: canonical unit storage (lineage)
- `server/src/apps/knowledge/repositories/postgres_vector.rs`: vector and keyword search storage (lineage)

### How the files were traced

The revision and the file mapping come from
`crates/aidash-domain/src/memory/UPSTREAM.md`. That file was added in
`c7aec0ed4` (#125) and removed in `8d9465051`. Every Rust surface it mapped is
listed above, and each one still exists at `68c27f135`. The notice at the
pinned revision matches `third_party/hindsight/LICENSE` byte for byte.

The content search looked for "Hindsight" across the tree and found no further
adapted material. The other matches name the `hindsight_rust` engine or label
it in the UI.
