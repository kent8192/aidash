# A Run waits out a service that does not answer and goes on by itself

Decided on 2026-11-11 in #875 (theme C of #872), with bug #906. Amends ADR 0007, 0013, 0020, 0059, 0066 and 0117, and corrects ADR 0070 and ADR 0116.

Many outages used to end in a pause that only a person could lift, even when the outage was short and had nothing to do with authority. Unattended and scheduled Runs then sat until someone noticed. This ADR replaces those pauses with one shared behavior, the **Outage Wait**, and lets advisory context go on degraded by default.

## Decision

**What stops and what waits.** Only a confirmed narrowing of authority (an Authority Change) stops a Run. A Node or service that does not answer is not an Authority Change, so the Run waits. This is the same line #751 drew between "cannot confirm" (`503`) and "refused" (`403`).

**Outage Wait.** A Run that needs a service which does not answer:

1. Retries the same step inside its lease at 2, 4, 8, 16 and 32 seconds, as before. So an outage of about a minute behaves as it did.
2. Then releases its worker and lease and waits as a Timer does, retrying the same step every 5 minutes. No separate health probe exists; the retry is the probe.
3. Goes on by itself at the first retry that gets an answer.
4. After 24 hours without an answer, pauses as before, so that only a person can resume it, and sends the normal pause notice. Runs and schedules have no deadline of their own today, so the bound is the same 24 hours for all.

Attempts made during an Outage Wait count against no call limit (such as `max_calls_per_run`) and no charge. The dashboard marks the Run "Outage Wait: <service>". No notice goes out on entry, only when the bound turns the wait into a pause.

These enter an Outage Wait:

- Every Decider Route failing (ADR 0066, formerly a `decider_unavailable` pause).
- Required semantic context whose embedding service or vector store does not answer, and optional context whose sources chose to wait (ADR 0059).
- An index that is not complete yet, where `Unavailable` used to spend retries (ADR 0020). Required-Home context waits for the complete index.
- Another Node not answering after a call already had its effect (ADR 0117, formerly a `run.node_unreachable` pause). A call that has not had its effect still returns to the model as a Rejected Call after its retries.
- A Read Recheck whose Node does not answer (#906). The check now records the miss as unconfirmed instead of an error. A Live preview withholds its responses (`503`) while unconfirmed, serves again once a check is confirmed, and stops as `node_unreachable` at the bound or at its own limit, whichever comes first. A running command still goes on to its own limit (ADR 0116).
- An exhausted plan allowance (ADR 0007). The Run is no longer failed. It waits and retries once an hour, because OpenAI gives no reset time.
- Native memory of a Home that is closed because a write outcome is unknown (ADR 0083). The Home stays closed for reads too, to keep the never-rewind guarantee. Only the way the Run stops changes.

The database wait (ADR 0113, ADR 0124) is the same kind of wait. It keeps its own 1-second cadence and has no bound, because nothing can be recorded about the Run while the database is away.

**Degraded context by default.** `on_unavailable` on a Workspace source now defaults to `degrade`. An Agent may still set `pause`, which now means an Outage Wait. Optional Workspace context whose index is incomplete returns its READY part as Degraded context with the existing typed marker, which is shown in the dashboard and stated in the model's context. Inference, authority decisions, write targets, required-Home context and native memory are never advisory and never degrade.

**Unloaded capabilities.** Revoking a capability that a deferred Run has neither loaded nor discovered no longer stops the Run. The capability leaves the Run's candidates, and an event stays visible to Deployment Administrators. Revoking a loaded or discovered capability still stops the Run at its next boundary (ADR 0013).

## Considered Options

- **Keep a pause per ADR.** Rejected: each ADR paused for consistency with the others (ADR 0066 cited 0057 and 0059), not for a risk, and the pauses cost unattended Runs the most.
- **Wait without a bound.** Rejected: a Run nobody watches should not wait for weeks. 24 hours covers a night's outage.
- **Probe each service for health.** Rejected: each service would need its own probe, and retrying the step answers the same question.
- **Retry every second, as the database wait does.** Rejected for external services: it loads the service and may cost money.
- **Notify on entering the wait.** Rejected: a notice per short outage teaches people to ignore notices.
- **Open native memory read-only during an unknown write outcome.** Rejected: reading from a ledger whose publication is not proven complete can read older memory and break the never-rewind guarantee.
- **Make the bound a Run or schedule deadline.** Deferred: neither exists, and adding one belongs to schedules (#557).

## Consequences

- A paused Run now means a person is needed: either authority narrowed, or a service stayed away for 24 hours.
- Outage retries no longer use up `max_calls_per_run`, so a long Decider outage does not exhaust a Run's budget.
- A Run in an Outage Wait holds no worker, so other Runs keep running during a long outage.
- Optional Workspace context degrades unless an Agent says otherwise. A Run may answer from partial context, and the marker says so.
- The judgment of a Remote Grant at the strictest Data Protection Level inside one Deployment (ADR 0117) is not an outage question and moved to #872.
