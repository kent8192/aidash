---
status: accepted
---

# Impose no fixed decision size or cardinality limits

The user explicitly rejected fixed ceilings for decision-state, request, response, and evidence sizes, question and batch counts, and request concurrency. Aidash therefore preserves the complete permitted decision and its evidence without adding those ceilings, while actual provider constraints, separately approved call allowances, and current authority still govern execution. This favors complete decisions over uniform local resource bounds and requires honest handling of provider, storage, and runtime failures without partial context application or silently truncated evidence.
