# PostgreSQL with pgvector and PGroonga is the canonical memory store and retrieval backend; Qdrant is removed

Status: **proposed** by the #126 documentation worker on 2026-10-11, filed locally by the batch lead. Awaiting the user's approval. Not yet accepted. PR #209 carries the matching documentation.

Context. The 2026-09-20 technology selection specifies PostgreSQL with pgvector and PGroonga for combined full-text and vector search, with reranking when needed. The Semantic Memory documentation at develop/0.1.0@d75d1c04 instead described PostgreSQL as the source authority and Qdrant as the vector index, and the configuration and backend adapter accepted Qdrant. PR #130 (merged 2026-10-07, closing #125) replaced the JSON memory with a native Rust Hindsight provider on PostgreSQL and removed the Qdrant adapter and deployment service. The two documents disagreed, and the implementation had changed the backend without an approved decision.

Decision (proposed). PostgreSQL 17 with pgvector and PGroonga is the approved backend, and it is the canonical store for memory units, their provenance, and their deletion and revision records. pgvector performs semantic ranking, and PGroonga provides multilingual keyword retrieval. The Qdrant adapter and deployment service are removed and are not an approved reference backend. The migration history keeps the old Qdrant constraint as frozen history; it does not describe a supported backend.

Hybrid retrieval. Full-text and vector retrieval are integrated and implemented. Recall combines semantic, keyword, graph and temporal rankings into one fused candidate list (implemented in the recall path of the application layer, per #130).

Reranking. A reranker is a required policy role with two explicit options: a registered model, or the explicit `Rrf` passthrough. A provider failure is an error and never falls back to `Rrf`. The "when needed" wording of the 2026-09-20 selection is not yet resolved: whether reranking stays a required role or becomes optional depends on #122 evidence and is the user's decision. Live retrieval quality is unverified.

Ownership of source text and vectors. PostgreSQL owns the source text and the derived vectors and indexes. Vector cleanup on deletion is durable and recorded in PostgreSQL. The external managed recovery directory (#125) holds the epoch anchor, the serving gate and the permanent deletion fences, and it is never rewound with the database.

Operational implications. Deployments need the pgvector and PGroonga extensions, which the bundled image provides, and persistent storage for the PostgreSQL data directory. Full PostgreSQL authority and control rewind and HA or failover are outside the managed recovery profile. The embedding default is `google/gemini-embedding-2` through OpenRouter at 3072 dimensions with Zero Data Retention routing; production models, prices and quality thresholds require separate approval.

Relationship to the earlier selection. This ADR confirms the 2026-09-20 selection and records its implementation; it does not change any requirement. Removing Qdrant is not a weakening of retrieval or authority requirements.

## Considered Options

- Keep Qdrant as the approved reference backend, with PostgreSQL as the source authority. Rejected: it duplicates the source of truth, and the native memory implementation no longer uses it.
- Keep Qdrant as an explicitly temporary implementation with a removal date. Rejected for this record: nothing in the current code depends on it, and the removal is already merged.
- pgvector and PGroonga as the approved backend, with Qdrant removed (this proposal).

## Consequences

- No document, configuration or deployment may describe Qdrant as available.
- Retrieval quality and the reranking default remain unverified until #122 measurements are approved.
- The frozen migration constraint and dated history records keep their original text.
- Owners: #126 (decision record), #130 (implementation), #110 (source ingestion and semantic retrieval contract), #118 and #141 (knowledge extraction and review contract), #122 (evaluation).

## Open item for the user

Whether reranking is a required policy role or an optional one (M-04 in `docs/memory-traceability.md`). The implementation requires the role; the 2026-09-20 selection says "when needed".
