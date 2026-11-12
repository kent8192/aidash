# An Authority Change raises a flag, and new work checks it at its entrance

Decided on 2026-11-07 (#768). An Authority Change returns only once nothing admitted under the old rules is still running (ADR 0010), so it waits on row locks that running work holds. A waiting `FOR UPDATE` does not stop new `FOR SHARE` requests, so while work overlaps the change never gets its turn. Now a change raises a flag for its Tenant, an exclusive advisory lock its transaction holds, and then waits on the row within a limit. New work holds no advisory lock. At its entrance it tests the flag with `pg_try_advisory_xact_lock_shared`, which answers false while a writer waits or holds. If so, it gives its connection back and tests again about every 200 ms. Code inside an open hold never tests.

## Considered Options

- **Keep the row locks alone, with a `lock_timeout`.** The change fails sooner but does not pass. Rejected.
- **Commit at once and let running work finish.** This reverses ADR 0010 and means reviewing every lock that relies on it. Rejected.
- **New work holds a shared advisory lock, so that it queues behind a waiting writer.** A step that holds it and opens another connection waits behind the writer, and the writer waits for the step. PostgreSQL cannot see that cycle, and at least seven code paths have this shape, including the start of a step and tool dispatch. Waiters also hold a connection, and the API process has 16, so one Tenant's wait starves the others and leaves none for a stop. Rejected.

## Consequences

- The entrances are named in code: an API request, a Run's first take and re-takes, an Egress request from a command, starting a Live preview, and the next page of an SSE stream. A missed entrance only lets that work pass a waiting writer, so anything else does not check.
- Work that is finishing or checking something already begun does not wait, because several of those paths have short timers: a periodic SSE authority check closes its stream after 5 seconds, and an Egress usage debit is given up after 5 seconds. Stops do not wait either, so a person can always stop work while a change waits. A route that can stop or resume work tells the two apart from the request at the boundary, before the visibility lease begins: a resume waits there like new work, because a wait under the lease would keep one of the few visibility connections.
- A peer from another Deployment does not check, because a call back from work this Node is running cannot be told from a new request. A Live preview's Egress requests do not check, because a preview's start holds the step's lease until the preview answers.
- A change that cannot finish within the limit answers Policy Busy, `503` with `Retry-After`, and nothing is applied. For three times the limit afterwards, further changes in that Tenant are refused at once, so repeated attempts keep new work waiting at most a quarter of the time. That record is per process, like the HTTP limits.
- A subject no policy could allow the action takes the ordinary lock instead, so its refusal stalls nobody. `PolicyBundle::may_allow` ignores resources and conditions, except that a deny which holds for every resource and under every condition ends the question, so it can only err towards waiting. It is decided on a read taken before the lock, so the lock checks it again: if the policy it locked now allows the action, the change answers `409` as for a stale revision instead of replacing the policy under a shared lock.
- Each entrance costs one more statement. A waiting request still keeps its HTTP admission permit and a waiting Run its worker slot, so one Tenant's wait can delay others by up to the limit. #800 measures this.
- Amends ADR 0010.
