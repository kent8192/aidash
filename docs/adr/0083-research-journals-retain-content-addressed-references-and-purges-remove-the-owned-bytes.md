# Research journals retain content-addressed references and purges remove the owned bytes

Decided on 2026-10-23 for #331. This amends ADR 0046 and ADR 0058: a Run is never deleted, so an immutable Context Journal cannot own an unbounded-lifetime copy of Research provider content. A Context Journal entry and a serialized Context Projection now contain an immutable Research reference instead of that content.

## Decision

A reference records its contract (`aidash.research.reference/1`), Run-local invocation identity, SHA-256 digest, Research operation, Source identities and Document identities. Search and Fetch use the same invocation identity as the charged Research attempt. Find and View also have an invocation identity even though they charge no provider attempt. The reference contains no provider titles, passages, snippets or rendered image bytes.

The Research store owns the exact JSON bytes of each delivered result variant in `research_journal_results`, scoped to the Tenant and Run and keyed by invocation identity and digest. Bytes are stored as binary, avoiding JSONB number normalization. The original envelope and a shortened or elided projection are separate content-addressed variants with the same provenance. Replacing a value with an expiry, revocation or unavailable placeholder keeps its reference identity. An elision is a distinct fitted variant, so recovery cannot accidentally expand it back into the original result.

`HistoryEntry.event` is an ephemeral execution value. A newly delivered result is visible to fitting and the model as before. Loading a projection or a journal range resolves each reference, verifies the payload digest and reconstructs that value. Durable `HistoryEntry` serialization always writes the reference, including after hydration or pruning. Model and summary requests explicitly use the resolved events. Remote tool observations also carry references rather than copying the provider content.

## Purges, revocation and concurrency

Account-terms expiry removes every owned variant whose invocation, Sources or Documents came from the expired provider result. Evidence Revocation removes every variant of the Run. Both clear binary contents while retaining identities, digests and the reason. Orphan variants no longer present in the latest projection are included. A late worker cannot replace a tombstone or freeze a new variant from already expired or revoked provenance.

Ordinary snapshot retention keeps delivered fragments and page images under ADR 0053. Journal variants follow those delivered results: ordinary snapshot retention alone does not remove them. Account-terms expiry and Evidence Revocation remove them and all compressed copies that absorbed them. The existing mutable attempt, invocation, event and evidence stores still perform their corresponding purge.

All Research writes, expiry, revocation and projection saves lock the Run row before its Research advisory lock. This establishes one order for a worker save and a purge. Within that fence, a save revalidates its projection, discards expired/revoked content, and refuses an adoption whose Execution Summary changed during the provider call. It freezes only values still retained. Reads do not take the exclusive writer fence; after resolving payloads they recheck the current expiry/revocation state, and execution retains the final pre-dispatch checks in ADR 0058.

## Integrity and recovery

The append-only Context Journal hashes its canonical event containing the reference. Summary range digests likewise commit to references, independent of their ephemeral materialized value. The journal's per-event digest is verified on range reads. Purges never rewrite a journal event, its digest or the source commitment of a previously adopted summary.

A summary that absorbed expired/revoked Research content is removed. Its source range is restored from the same journal references, validated with the original range commitment, and projected as placeholders. Compaction Attempt records retain digests and outcomes rather than candidate text. A reference with retained bytes resolves to the exact delivered JSON value; a payload whose bytes fail its digest raises an integrity error. A missing payload resolves to an explicit unavailable placeholder and is never reconstructed from a mutable snapshot or from another attempt.

## Replays, exports and the evidence viewer

- Execution replay resolves retained references to their exact delivered variant, including a shortened or elided result. After expiry or revocation it receives the corresponding placeholder. A missing unpurged payload is explicitly unavailable.
- Serialized Run projections and journal exports contain immutable references only. An export does not create a second plaintext archive or promise that a reference will remain readable. A consumer needs authorized access to the owning Research store to resolve retained contents.
- The existing evidence viewer continues to use the citation API. An account-expired citation keeps its identity/digests and reports expiry without content, including page images. A revoked citation follows ADR 0046's uniform unavailable response. Published answers remain under the existing ADR 0046 policy.

## Development compatibility

This changes durable Research history and exported Run JSON. Aidash has no deployment; its migration policy permits recreating development databases. Databases containing inline Research journal events must be recreated. The new reader/writer refuses such journals rather than silently rewriting append-only events, inventing retention provenance or invalidating old summary commitments. This is a breaking change and targets the development line. A future deployed upgrade needs a separately designed, explicit compatibility procedure.

## Considered options

- Keep inline copies and overwrite journal results during purges: rejected because it changes journal/range commitments and contradicts append-only recovery.
- Reconstruct results from the current snapshot: rejected because fitting, paging and provider fields can differ from the value the model saw.
- Put only a digest in the journal without durable provenance: rejected because Find/View and shortened orphan variants still need to be purged with their originating provider result.
- Keep an export-specific plaintext archive: rejected because it recreates the unbounded-lifetime copy this decision removes.
