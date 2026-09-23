# Channel allowlist defaults and revocation

A channel takes its initial agent allowlist from the account's selection at creation. Thereafter the channel maintains its own selection: later account additions or removals do not propagate automatically. An authorized user may explicitly review and apply account changes to a channel. Editing the account default is not a global revocation of agent or credential authority.

Removing an agent's use permission from the effective channel allowlist prohibits new work and stops ongoing work dependent on that permission at safe, persisted boundaries. It is not merely a restriction on future assignments. Removing a generation-policy opt-in likewise stops the generated agents and descendant work that depend on that opt-in; unrelated work is not stopped solely by that removal.

Revocation also terminates history and stream access granted through the affected delegation. Independently granted read permissions and permissions in other channels are evaluated separately, but cannot be substituted for the removed execution eligibility. A remaining budget reservation or a previously assigned task does not override the revocation. Credential and operation-authority revocation remain binding regardless of account or channel defaults.

Do not remove Registry definitions or historical work merely because eligibility is revoked. Preserve in-flight external-effect outcomes, cost settlement, and records required to prevent duplicate execution; already performed effects are not undone. The ability to record or reconcile an existing outcome does not authorize further agent work or renewed channel observation.

Copying defaults prevents an account-level convenience edit from silently widening the set of agents that may receive an existing channel's information. Applying channel revocation to active work makes removal an effective stop-use decision rather than an implicit permission to finish every task already assigned.
