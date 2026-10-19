# A Live preview is started by its Run, pinned to one Working Area revision, and outlives the Run only within limits

Decided on 2026-10-19 while grilling Live previews. An operation's Execution Pod ends with the operation, within at most 600 seconds, but a Live preview's Web server must keep running while a person looks at it. Only a Sandboxed tool call in a Run starts a Live preview, so the tool's Catalog Approval (ADR 0034) and the Run's authority govern the code it runs; a person may stop one but not start one. The Live preview belongs to that Run and may outlive the Run's completion until it is idle for its idle limit or reaches its absolute limit, and it stops when the Run is cancelled, the Run's authority is revoked or the Tenant is deleted. A Run has at most one Live preview, and starting another replaces it. It mounts the Working Area revision current when it starts and never publishes what its server writes; a newer revision marks it stale, and a later model turn restarts it.

## Considered Options

- **End with the Run.** Nothing would be left to look at once the Run completes. Rejected.
- **Belong to the Working Area, so that a person can restart it without a Run.** Code would run outside any Run's authority (ADR 0043). Rejected.
- **Sync later revisions into the running Pod for hot reloading.** What a viewer sees would no longer be one exact content, unlike Verification Evidence (ADR 0030). Deferred.

## Consequences

- The Deployment sets the maximum idle limit (15 minutes by default), absolute limit (2 hours by default) and resources, and a Tenant may only lower them, as with Test Limits.
- Seeing a Live preview again after it stopped takes a follow-up Run.
- The Live preview's Web server reaches the External services bound to its Run through the Egress gateway with a token that lasts as long as the Live preview and is revoked with it, so it keeps working after the Run completes.
- The model never receives an address for viewing a Live preview; viewing takes a person's ticket (ADR 0068).
- The Run's requester and whoever may cancel the Run may stop its Live preview; viewing alone does not allow it.
