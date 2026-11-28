# Time-limited Background processes hold the Execution Pod

Decided on 2026-11-25 in the long-running execution grilling (#1137 and #1144).
An Agent needs to run a backtest or conversion that takes hours and may produce
the input it needs to continue. A Space's idle timeout used to stop that work
after 30 minutes with no foreground command or Live preview viewer.

## Decisions

- **A Background process started on purpose has a time limit.** The Agent sets
  `background: true` and `timeout_seconds` on `shell`. The Deployment ceiling
  defaults to four hours and cannot exceed 24 hours. A Tenant may lower it. An
  over-limit request names the effective ceiling in its error.
- **A time-limited Background process holds its Execution Pod.** The Runner
  derives the hold from the Space journal. The supervisor enforces the command
  deadline even if the Runner goes away. The Runner backs it up 60 seconds
  after the deadline. Processes left by foreground commands have no time
  limit and do not hold the Pod.
- **Run cancellation stops its Background processes.** Normal Run completion
  permits them to run to their limit. Authority revocation still stops them.
  The Runner records the stop reason for the seven-day process listing.
- **Completion is an input when requested.** With `notify_on_exit: true`,
  the Run's input ledger receives the status, exit code, stop reason and
  last 4 KiB of output. Input activates a sleeping Task through its existing
  Stint lifecycle (ADR 0141). A completed Task keeps the operation record.
- The Runner accepts foreground limits up to 24 hours, subject to the
  Deployment's separate foreground ceiling.

## Considered Options

- **Use the ordinary idle clock for all processes.** This interrupts useful
  work during long stretches without user or Agent traffic.
- **Allocate a second Pod for Background processes.** This adds storage and
  admission work and divides the Space across two runtimes. Revisit it if CPU
  measurements show contention in the shared Pod.
- **Wake on every exit.** Opt-in avoids waking Tasks whose processes were
  meant to run independently.

## Consequences

- A Space can continue to consume Pod time while its Agent sleeps.
- Run cancellation and successful Run completion have different effects on
  running processes. This amends ADR 0111 and the Run-cancellation decision
  in #697.
