# Parallel read batches

When one model response contains several independent read calls, Aidash can
run them at the same time as a **Tool Batch**. Batching is opt-in, and every
call outside a batch keeps the sequential path.

## Enabling

Two ceilings must both exceed one:

- **Run ceiling**: the Agent definition's `tool_parallelism`, 1–16, default 1.
  It is fixed with the Run's Binding snapshot, so editing the Agent never
  changes a Run that has already started.
- **Process ceiling**: `AIDASH_TOOL_PARALLELISM`, 1–4, default 1. All workers in
  one server or worker process share it; each replica has its own. Each
  concurrent call holds one database connection for its own transaction, so
  startup rejects a ceiling that leaves no room in the database pool beside the
  four worker slots.

```json
{
  "schema_version": 1,
  "model": {"id": "model", "version": "1.0.0"},
  "instructions": "Research the working files.",
  "tool_parallelism": 4
}
```

A Tool Batch runs at most min(Run ceiling, process ceiling) calls at once. Calls
wait their turn for process capacity, and a call never holds capacity while
waiting for more.

## Eligible calls

A provider must declare a tool concurrency-safe. Read-only effect or replay
safety alone does not make a tool eligible. The initial set:

| Tool          | Resource claim               | Output bound                          |
| ------------- | ---------------------------- | ------------------------------------- |
| `file_read`   | Working Area, shared         | `max_bytes`, at most the Node read limit |
| `file_search` | Working Area, shared         | Node search byte limit                |
| `skill_list`  | Pinned Skills, shared        | Page size × Skill metadata            |
| `skill_load`  | Pinned Skills, **exclusive** | Skill package limits                  |

`file_read` with `representation: "model_input"` stays sequential, because
selecting model media changes the next request. `skill_load` updates the Run's
loaded-Skill markers, so two loads, or a load and a list, never share a batch.

Registry configuration can only lower this declaration. A descriptor or
Binding may set `"narrow": {"concurrency": "sequential"}`. A claim of
`shared_read` for a tool whose provider is Sequential is rejected.

## How a batch forms

The batch starts at the first call that has not run yet. It extends in the
model's order and stops before the first call that:

- is not concurrency-safe, or whose resource claims or output bound cannot be
  derived from its arguments;
- conflicts with a claim already in the batch (exclusive access to the same
  resource);
- would exceed either ceiling; or
- could not fit the request budget if every earlier call in the batch returned
  its worst-case result.

Calls are never reordered: a write between two reads ends the batch. A batch
needs at least two calls; otherwise the call runs alone. Batches form only in
ordinary execution. They never form during run-message catch-up, while
selected model media is pending, or before required message reads are done.

The budget check is conservative. Each result is measured as if every bounded
byte were a control character escaped in the request encoding. With default
Node limits, large-window models batch reads, and small windows mostly fall
back to sequential calls.

## Durability and recovery

- Admission is one transaction. It records the batch range in the Run and
  journals every call as started, before any call executes, and only after it
  confirms the worker lease and that no newer Run input exists. Each call keeps
  its original key, `{run}:{response epoch}:{index}`.
- Each call records its result when it finishes. After every call has finished,
  the results are added to the Context Journal in call order, even when calls
  finished out of order.
- After a restart, completed results are reused and unfinished calls run again.
  Every eligible tool is read-only and replay-safe, so the uncertain-effect
  path cannot occur.
- Current authority is checked for every call before admission. All calls of a
  batch run under the Run's single authority lease, so a revocation waits until
  the batch ends. A batch resumed after a restart rechecks authority, and a
  revoked call records its denial instead of running.
- Cancelling the Run during a batch drops calls still in flight, and none of the
  batch's results are adopted. Losing the worker lease stops the step, as it
  does for a sequential call.
- An error in one call does not stop the others. After all calls finish, the
  first failure in call order fails the step, and recovery reuses the results
  that completed.

## Read receipts

Each `file_read` and `file_search` result carries the Working Area revision and
generation it actually read. Calls in one batch can observe different
revisions, and a batch never claims a consistent snapshot across calls or
external services.
