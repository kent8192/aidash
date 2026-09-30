---
status: accepted
---

# Stop Runs whose semantic sources have changed

For [Issue #75](https://github.com/kent8192/aidash/issues/75), deleting or changing a Semantic source revision stops dependent remote Runs and denies ordinary access to their dependent journals, Artifacts and Messages on both Nodes. Substituting fresh search results cannot prove that old information has disappeared from prior inference, summaries or outputs, so work using changed sources starts a new Run without carrying over the invalid context. Audit records remain under restricted access and the separately approved retention policy.

A transient outage may recover after current authority and every dependency are revalidated. Recovery from withdrawn authority requires explicit authorized resumption and the original bindings to remain valid; a new credential or grant cannot silently replace the original one. This controls subsequent use and visibility, not recall of information already delivered to an external model or reader.
