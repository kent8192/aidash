# A repeated check adds no audit row, and what a Run has read is rechecked on its own schedule

Decided on 2026-11-07 for #758 and #752. One build of 11 Runs wrote 38 million Authorization Decisions, 15 GB and nearly the whole database, and platform time passed model time. Measured by which checks each commit contained, 56 % of the rows came from Egress, 24 % from the sweep over running commands, 19 % from worker boundaries, and 0.14 % from people alone. The cause was a getter: `run_for_interaction` walks everything a Run has read, and every Egress request and sweep pass called it. `decide` also always wrote, so checking and recording were one step.

We split the check in two. The Continuation Check runs on each Egress request and each pass over a running command, and asks only whether the Run may still run. The Read Recheck asks whether everything the Run read may still be read. It runs at each worker boundary and, for what runs between boundaries, every 30 seconds on a loop of its own. Inside one check nothing is looked up twice and rows are loaded together, so the statement count does not grow with the reads. Read Coalescing then records a repeated allow once per Run or Remote Grant, subject, action, resource and policy revision.

## Considered Options

- **Remember a verdict between Read Rechecks, until something changes.** A shared change counter would have to cover Catalog Approvals, Peer Mappings and others that keep their own revisions, and one missed bump leaves a revocation without effect. Rejected: no outcome carries over between checks, only the fact that a row exists.
- **Compress rows by time, such as one row a day.** A discarded row cannot be restored, and people produced 0.14 % of the rows. Rejected: what a person opens is still written on every request.
- **Keep what is already recorded in process memory.** It costs no statement, but a restart writes everything again, several processes write duplicates, and the rule becomes approximate. Rejected: the check reads the table in the transaction it already holds, and a unique index settles a race. A repeat still needs no new connection and no audit lock.
- **Run the 30 second check inside the existing sweeps.** The operation sweep handles every Tenant one operation at a time, so a slow check would delay other Tenants' commands, and the Live preview sweep ticks only every 60 seconds with retention work. Rejected.
- **Stop what is running when a Node does not answer.** Stopping a command would make installs fail under load, where a check can miss its limit with no Node down. A command therefore goes on to its own limit, and a revocation can take effect that late. A Live preview can serve for up to 24 hours, so it stops after five misses in a row, about 2.5 minutes, as `node_unreachable` and not as a revocation. Until #751 lets a check tell a Node that does not answer from a refusal, the server answers `Refused` in that case: the schedule has the five-miss rule, but nothing reports a miss yet, and a command is withdrawn and a Live preview stops as a revocation.

## Consequences

- From a revocation to a stopped request takes up to 30 seconds, plus one pass of the loop, plus up to 5 seconds for a request in flight. Egress used to refuse on the next request.
- A withdrawn command's requests answer `Forbidden`, and not that the operation expired, so the gateway reports them truthfully.
- The command's row is marked withdrawn, as `cancelling`, before the Runner is asked to stop it, so a Runner that is down does not keep its Egress open. Once it is marked, the operation sweep goes on asking the Runner, and the loop leaves it. A stop that fails for a Live preview is tried again after 5 seconds, then 10, up to a period.
- `authorization_decisions` gains `run_id`, `remote_grant_id` and `coalesced`, and the API returns `run` and `remote_grant`. Older rows name neither and are left alone.
- A missing row no longer means a read went unchecked. The decisions panel and `docs/authorization.md` say so.
- The audit lock is per Tenant. The only reader of the sequence cursor filters by Tenant, so it still never skips.
- `tool.invoke` and `model.infer` stay on the record for every call, so only which step re-read a resource is lost.
- A Continuation Check also keeps `workspace.read`, which costs no statement beyond the owner it already loads.
- The last check time lives in the loop's memory. A restart adds one check, and checks at once, so a revocation made during downtime is found immediately.
- Amends ADR 0054, ADR 0070 and ADR 0112.
