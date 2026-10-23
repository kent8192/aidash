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

Decided in the 2026-10-23 design rounds for [Issue 388](https://github.com/kent8192/aidash/issues/388).
See the [completion design](../decisions/issue-244-completion.md).
