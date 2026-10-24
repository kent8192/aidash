# An authorized manual retry opens a new Retry Cycle of the same memory operation

Decided on 2026-10-17 (#291). A native-memory model operation of an Ordered Run keeps its operation ID for as long as its Retrieval Key holds (ADR 0020), and a remote Run's operation keeps the remote semantic operation ID across a resume. Its retry ledger therefore stayed spent after an authorized manual retry, and the Run paused again as `RetriesExhausted` even after the provider recovered. The caller now passes the operation's Retry Cycle to the ledger: a native Run's monotonic cycle, which an authorized manual retry of a semantic pause increments, or the remote journal record's existing `cycle`. Each attempt records its cycle, and `max_retries` counts only attempts of the current cycle. A completed attempt from any cycle is still served from the ledger, a live attempt from any cycle inside its call deadline still makes a replay wait as `Pending`, and the operation's durable call, token and cost totals accumulate across every cycle.

## Considered Options

- Put the cycle into the operation key. Rejected: every manual retry would start a new operation row with zero durable totals, so earlier charged attempts would stop counting against the operation's budgets; ordinals that had already completed would be called and charged again; and abandoned rows would consume the Bank's model-operation capacity.
- Add a Run column to the operation ledger and reset that Run's rows on resume. Rejected: a Home serving a remote Run's recall has no local Run to key on, so native and remote would need separate reset paths, and deleting or rewriting attempts would erase the charge history.

## Consequences

- Automatic retries within one cycle still replay the same operation, so memoized results stay cached and the ledger still bounds them.
- A Legacy Run already derives a new context-recall operation ID on every save, so the cycle changes nothing there; it still renews the ledger of a memory tool recall, whose key a Run replays.
- Engine jobs have no Run and stay in cycle 0; a spent engine-job ledger still fails the job.
- Because durable totals span every cycle, repeated manual retries eventually exhaust the operation's budget, which no unchanged retry can cure. This pauses local and remote Runs with `durable_memory_budget` and offers only a follow-up Task, whose Run gets a new operation (#332). Engine jobs still fail when their durable budget is spent.
