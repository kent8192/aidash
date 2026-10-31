# Third-party notices

Aidash contains material adapted from the projects below. That material keeps
its own license, and each notice is kept verbatim under `third_party/`.
Dependencies that Cargo and npm resolve are not listed here; their notices
belong to the licensing work in [#124](https://github.com/kent8192/aidash/issues/124).

Each file list says how a file was found. **Lineage** means that the file's
lines descend from a file recorded as adapted. **Content** means that the file
was found by comparing it with the upstream source. **Copy** means that the
file is an upstream file itself, unchanged apart from compression.

## fast-jev-compaction

- Source: <https://github.com/tamaratran/fast-jev-compaction>
- Revision: `e3f262a7f4d42bd8dd32ced30d26176f7cb545b0`
- License: MIT, in [third_party/fast-jev-compaction/LICENSE](third_party/fast-jev-compaction/LICENSE)

Files that contain adapted material:

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

#108 (commits `8ae3d9d81` and `7b6880532`) replaced the compactor with the
compaction Decider. It deleted `crates/aidash-application/src/context/compaction.rs`,
`crates/aidash-integrations/src/compaction.rs` with its tests and
`server/src/apps/execution/services/context/jev.rs`, and removed the Jev answer
validation from `crates/aidash-application/src/context.rs`. The same blame on
the #108 head traces no lines to the two paths beyond generic code: the
`reqwest` client builder in `crates/aidash-integrations/src/decision.rs` and
imports and a route in its tests. `crates/aidash-domain/src/decision/pruning.rs`
carries lines from the deleted `context/compaction.rs`, but they were written
in that file after the split and do not trace to either path. The content
search found none of the upstream defaults or input caps outside the rubric.

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

## gpt-oss

- Source: <https://huggingface.co/openai/gpt-oss-120b>
- Revision: `b5c939de8f754692c1647ca79fbf85e8c1e70f8a`
- License: Apache-2.0, in [third_party/gpt-oss/LICENSE](third_party/gpt-oss/LICENSE)

Files that contain upstream material:

- `crates/aidash-domain/assets/token-encodings/o200k_base.json.zst`: the upstream `tokenizer.json`, compressed with zstd, which counts the `o200k_base` Token Encoding (copy)

The server embeds the file and checks it against the SHA-256 of the upstream
`tokenizer.json` at the pinned revision before it counts anything. The notice
is the upstream `LICENSE` at the same revision.

## Gemma 4

- Source: <https://huggingface.co/google/gemma-4-E4B-it>
- Revision: `ee0ef6023621cff504d758262d4e04895a5af4a2`
- License: Apache-2.0, in [third_party/gemma-4/LICENSE](third_party/gemma-4/LICENSE)

Files that contain upstream material:

- `crates/aidash-domain/assets/token-encodings/gemma4.json.zst`: the upstream `tokenizer.json`, compressed with zstd, which counts the `gemma4` Token Encoding (copy)

The server embeds the file and checks it against the SHA-256 of the upstream
`tokenizer.json` at the pinned revision before it counts anything. The model
card names its terms and links to them rather than shipping a notice file, so
the notice is the text published at <https://www.apache.org/licenses/LICENSE-2.0.txt>.

## Enterprise

These materials are shipped under `enterprise/` but are not part of the
Licensed Work. Each keeps its own terms, and the rest of `enterprise/` remains
under [enterprise/LICENSE](enterprise/LICENSE).

### IBM Plex fonts

- Source: [IBM Plex](https://github.com/IBM/plex), packaged as
  `@fontsource/ibm-plex-mono` and `@fontsource/ibm-plex-sans`
- Revision: `5.3.0`, from `web/package-lock.json`
- License: SIL Open Font License 1.1, in
  [enterprise/auth/static/assets/fonts/mono/LICENSE](enterprise/auth/static/assets/fonts/mono/LICENSE)
  and
  [enterprise/auth/static/assets/fonts/sans/LICENSE](enterprise/auth/static/assets/fonts/sans/LICENSE)

Files that contain upstream material:

- `enterprise/auth/static/assets/fonts/mono/` and `enterprise/marketplace/static/assets/fonts/mono/`: IBM Plex Mono web fonts and their `@font-face` styles (copy)
- `enterprise/auth/static/assets/fonts/sans/` and `enterprise/marketplace/static/assets/fonts/sans/`: IBM Plex Sans web fonts and their `@font-face` styles (copy)

The two copies of each family are identical. Each directory keeps the
`LICENSE` file that the font package ships.

### Firebase browser modules

- Source: [Firebase browser ESM](https://firebase.google.com/docs/web/alt-setup),
  from `https://www.gstatic.com/firebasejs/13.0.0/`
- Revision: `13.0.0`; `enterprise/auth/static/assets/vendor/provenance.json`
  records the upstream and vendored SHA-256 digests
- License: Apache-2.0, in
  [enterprise/auth/static/assets/vendor/LICENSE](enterprise/auth/static/assets/vendor/LICENSE)

Files that contain upstream material:

- `enterprise/auth/static/assets/vendor/firebase-app.js`: the Firebase 13.0.0 App browser module, with a relative App import in Auth, removed development source-map references and whitespace normalization (copy)
- `enterprise/auth/static/assets/vendor/firebase-auth.js`: the Firebase 13.0.0 Auth browser module, changed in the same way (copy)

The `README.md` and `provenance.json` in that directory describe the vendored
files and are Aidash material under `enterprise/LICENSE`.
