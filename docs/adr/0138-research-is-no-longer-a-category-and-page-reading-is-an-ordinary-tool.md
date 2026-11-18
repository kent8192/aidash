---
status: accepted
---

# Research is no longer a category, and page reading is an ordinary tool

Decided on 2026-11-16 (#987, with #962). This supersedes ADR 0046 (Evidence Revocation), ADR 0048 and ADR 0055 (declared Research providers), ADR 0049 (Web disclosure approval), ADR 0058 (Provider results and Research account terms) and ADR 0083's Research journals, and amends ADR 0034 and ADR 0053.

Research was a category of its own: Web search, Page reading and searching within a fetched page, whose results Aidash kept as evidence (Source records, Document snapshots, Evidence fragments), whose answers it checked for Source citations, and whose stored copies Evidence Revocation could remove. The Context Journal already records every tool call and result of a Run, and the Egress records every outside connection, so the audit trail does not depend on that store. What the category added was the citation check, a full copy of each page kept under its own Retention Rules, and revocation. None of them is kept.

- **Brave Web search is removed** (#962). Aidash no longer holds a search account on a Tenant's behalf, so the Research account terms, the Research account holder and the retention of Provider results go with it. There is no built-in search. An Agent can use an OpenRouter `web_search` Provider tool at the No Restriction level (ADR 0137), or a Tenant's own MCP or HTTP tool, and then read the pages it found.
- **Page reading is an ordinary opt-in built-in.** It keeps its stored identity, `research.page@1` with operation `fetch` and the `research` tier, because renaming it would rewrite every stored descriptor, Catalog Approval, Binding and Run snapshot. Its definitions move to 2.0.0 and need a fresh Catalog Approval. It takes `url` and an optional 1-based `part`, fetches the page again on every call, and returns one bounded part of its text. It stores nothing beyond the Context Journal; a page that changes between parts is recorded as it was read.
- **Its approval is a Tool call approval.** Web disclosure approval goes. A page read names the host it contacts and the input it was made after, so a Run grant covers one host and ends at the next input from a person, as Web disclosure approvals did. Approval Rules lose the `web_search` and `page_reading` kinds; a rule for the page reading definition covers every host.
- **URL intents stay.** A person may still ask a Run to read particular pages; that is a request, never an approval.
- **PDF pages without text** are reported by page number. The page images of ADR 0053 are not rendered: they were Evidence fragments read through `view`, which is gone.
- **The data goes.** The Research tables are dropped, Web disclosure approval records deleted, and a memory's Journal proof keeps only the admitted range. A Run still waiting on a Web disclosure approval cannot resume and is cancelled. Removed definitions keep their registry rows so that pinned Agent versions resolve, as with #952, while their Catalog Approvals and draft Bindings are deleted.

## Considered Options

- **Keep the category with page reading as its only provider.** It keeps the citation check, which tells a quoted source the Run read from one the model made up. Rejected by the owner: the Context Journal is the audit trail, and the evidence store, its retention and revocation are a large surface kept for that one check.
- **Rename page reading to a `core.` identifier.** Rejected for now: every stored reference would need rewriting. The leftover `research.` name is cosmetic.

## Consequences

- An answer's sources are no longer checked. A quoted page can be found in the Run's Context Journal, but nothing marks a citation the Run never read.
- Removing a page's content after the fact means deleting the Run, as for any other tool result.
- Page reading is outside the Data Protection Level (ADR 0137), like any tool that is not an OpenRouter call.
