# Node deletion is recoverable and releases capacity before purge

A Tenant Administrator may delete a Node after moving its default role to another
live Node; the last live Node cannot be deleted. Deletion cancels its work and
closes admission, but keeps its data restorable for a Node-specific grace period
of 30 days by default (1–90 days, fixed when deleted). Only a Deployment
Administrator may bypass that period and purge immediately.

A deleted Node releases its Node-count slot immediately, allowing a Tier
downgrade without destroying recoverable data. Restore therefore checks the
current capacity and refuses if the Node would exceed it. Restore uses current
Tenant policy and current credential and registration state; it neither rolls
back intervening revocations nor restarts canceled Runs. Retained files stay in
the active Tenant's storage meter until purge, and deletion screens state this
cost. The existing storage exemption for a deleted Tenant remains applicable.

Purge removes only Node-owned data, retains the Node name permanently and keeps
separable audit history under the existing finite audit retention setting.
Tenant-shared Registry definitions, Provider Credentials, Tenant Secrets, user
subjects and shared policy revisions survive. Registrations owned by another
Tenant or Deployment stay under that receiver's control.

A Workspace Event follows its Workspace owner. Its scalar emitter Node metadata
references the permanent same-Tenant name, allowing finite Node catalog audit to
expire without deleting or rewriting a surviving Workspace's Event body. This
metadata does not confer current Node admission or access authority.

Current registrations needed by retained atomic journals are recovery state,
separate from finite audit history. Retain only the exact local coordinator to
remote participant and local participant to remote coordinator pairs validated
from immutable manifest, transaction ID, digest and permanent Node ownership.
Retain terminal COMMITTED and ABORTED dependencies too: neither terminal phase
proves the remote coordinator received the reply, and Finish also fetches the
coordinator decision through the reverse route. Keep the current keyset, both
revisions, quarantine, authority and endpoint unchanged under Tenant ownership.
Anchor these rows to permanent Node names so catalog and deletion history can
expire without reopening liveness. Other current registrations and registration
history follow their own purge/audit classification. No audit deadline expires a
still-replayable journal's trust; no historical key fallback or new protocol
acknowledgement is introduced. Missing or inconsistent ownership, manifest,
digest or required current trust blocks erasure. An absent Node remains closed;
only the exact existing journal-bound recovery path may use retained trust.

Deletion records exact work identities in a durable cancellation queue in the
same transaction. Restore retains that stop intent; mutable Run timestamps cannot
stand in for an immutable deletion-time snapshot.

A sibling Node's source-read provenance survives source purge. Source existence is
checked when recording the read; a source lifetime foreign key must not make valid
cross-Node reads retain source data forever or deadlock two purges. Existing
source-deletion cache invalidation removes derived receipt bodies while retaining
content-free read identities and unrelated sibling data.

Decided in the 2026-10-23 design rounds for [Issue 388](https://github.com/kent8192/aidash/issues/388).
See the [completion design](../decisions/issue-244-completion.md).
