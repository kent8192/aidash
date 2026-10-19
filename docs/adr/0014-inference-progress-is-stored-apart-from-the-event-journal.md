# Inference Progress is stored apart from the event journal and delivered on a Run-scoped stream

Decided on 2026-10-09 (#120). Inference Progress (text deltas and tool-call identity, name and argument size) is written to a dedicated bounded table keyed by Run, Inference Attempt and sequence, with its own retention. It is delivered over a new Run-scoped SSE endpoint that reuses the existing connection, ReadLease, per-frame authority and backpressure code. Only low-rate Progress Outcome markers (`inference.started`, `inference.accepted`, `inference.discarded`, `inference.interrupted`) enter the existing `events` table and its NATS outbox. Inference Attempts get a public identifier; the worker lease token stays internal.

## Considered Options

- Write delta batches into `events`. Rejected: every insert takes the global sequence advisory lock, every event is relayed through the NATS outbox, `events` has no retention, and the web and desktop clients refetch eight query keys on every event. Deltas would also become a second execution journal.
- Carry deltas only in memory or over NATS. Rejected: writer, execution and SSE run as independent processes, so reconnect replay without duplicated text could not be guaranteed.
- Expose the worker lease token as the attempt identifier. Rejected: it fences durable writes and must not reach observers.

## Consequences

- Clients that do not understand Inference Progress never open the new endpoint and still receive the current final result, so compatibility is additive.
- The adapter assembles and validates the stream and hands the harness only an Accepted Response; Inference Progress has no executable form, so partial or salvage-parsed tool arguments cannot be dispatched.
- The endpoint is `GET /api/runs/{run}/inference/stream`. Its cursor is a per-Run `progress_seq` allocated by the lease-fenced writer, so no global lock is taken. Without a cursor it starts at the latest Inference Attempt. Frames are `inference.delta`, `inference.tool_call`, `inference.outcome` and `gap`, in CloudEvents of type `aidash.inference.progress.v1`.
- Delta rows are pruned 15 minutes after their Inference Attempt's Progress Outcome marker; Accepted Responses already survive in Run state and messages. The markers are kept as long as other events. A reader whose cursor predates retention receives an explicit `gap` frame.
- Bounds: 1 MiB assembled response (as for non-streamed bodies), 16 KiB per progress row, flush every 250 ms or 4 KiB, a fixed-capacity sink that coalesces instead of blocking inference, and a 120 s stall timeout in which provider comment lines do not count as liveness.
- The worker rechecks Run inference authority at most once per second while it writes progress, and aborts the provider call as interrupted on failure. Readers still recheck authority per frame, so the interval only bounds extra writes, not disclosure.
- Interrupted, stalled or mid-stream-failed attempts leave usage uncertain and keep the full reservation charged. Mid-stream overflow or truncation errors are normalized to the same typed errors as non-streamed responses and enter the #113 pipeline.
- `ModelConfig.streaming` selects the streamed path. The non-streamed path remains for configurations that set it to `false`.
- Every Inference Attempt ends with exactly one Progress Outcome marker. A worker that loses its lease cannot write, so the next claimant, or the existing reconciler for Runs that will never be claimed again, marks its pending attempts `interrupted` with reason `lease_lost` before any new attempt starts.
- A Progress Outcome never changes once written. When new input discards an Accepted Response before tool admission, the existing `run.message_received` transition records that; the progress stays `accepted`.
- New user input during inference aborts the provider call like a cancellation. Its usage is then uncertain and the full reservation stays charged.
- Cloud streaming depends on the Credential Broker (#137) passing streamed responses through; #120 itself stays transport-agnostic.
