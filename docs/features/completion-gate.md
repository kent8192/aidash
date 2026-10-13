# Completion Gate

A model's final answer is a **Completion Candidate**, not proof that the task is
done. When an Agent definition names a **Completion Policy**, a Run completes
its Task only when current **Verification Evidence** supports the candidate, or
when no **Check** applies to it. A sentence such as "all tests pass" in the
answer is only text; only a Check that Aidash ran counts.

## Enabling

The policy is part of the immutable Agent definition. It is copied into the
Run's Binding snapshot and fixed for the Run, like `context_policy` and
`exposure`. An Agent definition without `completion` requires no verification,
and its Runs complete exactly as before.

```json
{
  "schema_version": 1,
  "model": { "id": "model", "version": "1.0.0" },
  "instructions": "Fix the failing parser tests.",
  "completion": {
    "version": "checks/1",
    "checks": [
      { "name": "tests", "capability": "shell", "command": "cargo test" }
    ]
  }
}
```

| Field                      | Default           | Bounds                      | Meaning                                                            |
| -------------------------- | ----------------- | --------------------------- | ------------------------------------------------------------------ |
| `checks`                   | required          | 1–8, distinct names         | Checks in the order they run                                       |
| `checks[].name`            | required          | `[a-z0-9][a-z0-9_-]{0,63}`  | Name shown to the model and people                                 |
| `checks[].capability`      | required          | `shell`                     | The bound operation the Check runs under                           |
| `checks[].command`         | required          | 1–65,536 bytes              | The exact command; the model can never change it                   |
| `checks[].timeout_seconds` | node default      | 1–600                       | The Check's own timeout, within the node's operation ceiling       |
| `checks[].when`            | `content_changed` | `always`, `content_changed` | `content_changed` runs only when the Run changed the mounted files |
| `repair_allowance`         | 3                 | 0–16                        | Repairs per accepted human input before a person is asked          |
| `verification_seconds`     | 1,800             | 1–3,600                     | Total Check time for the whole Run                                 |
| `exception`                | `false`           | boolean                     | Whether a Completion Exception may be allowed (not yet available)  |

The Agent must also bind the shell capability. A Check passes the same Binding,
narrowing and `tool.invoke` authorization as the model's own shell calls, keeps
the shell's effect classification and approval rules, and draws Execution Pod
time like any other operation.

## What a Check verifies

Evidence is bound to the digest of every file mounted in the Run's Working Area
(scope, path and file digest), not to the Area revision and not to the answer's
text. The revision increases on every publication, even when no file changed,
so it cannot identify content. Evidence counts only for the same Run, Check
definition, capability definition and content. A reworded answer over
unchanged files reuses the evidence; any change to the files needs the Check to
run again.

A Check runs on exactly that content in an Execution Pod whose files are never
exported back: build output and test artifacts are discarded, the Working Area
and its revision stay unchanged, and only the tail of the Check's output
(4 KiB) is kept as its diagnostics. While a Check runs, the Area is busy, so
the model's own shell calls are refused as busy until it finishes.

`content_changed` compares the mounted content with the content the Run started
from, so a conversational Run that changed no files completes without running
anything. A node that cannot run Checks cannot read the content either, and
pauses every candidate rather than assuming nothing changed.

## Outcomes

| Outcome       | Meaning                                         | What happens                                                          |
| ------------- | ----------------------------------------------- | --------------------------------------------------------------------- |
| `passed`      | The command exited 0                            | The next Check runs; after the last one the Task completes            |
| `failed`      | The command ran and exited non-zero             | The model gets the diagnostics and repairs; one unit of the allowance |
| `timed_out`   | The command ran past its timeout                | As `failed`                                                           |
| `unavailable` | The Check could not run, or its result was lost | Completion Review                                                     |
| `denied`      | Authority refused the Check                     | Completion Review                                                     |

A repair is a `completion_repair_required` event in the Context Journal with the
Check name, outcome, exit code and diagnostics. The Run goes back to the model,
which can fix the files and answer again. Each accepted human input renews the
Repair Allowance; the Run's `verification_seconds` are never renewed.

A **Completion Review** is a request for a person, raised when a Check is
unavailable or refused, the Repair Allowance is spent, or the verification time
is spent. Until a dedicated request kind exists it is an `INFORMATION_REQUEST`
whose prompt starts with `Completion Review:`. Answering `retry` (or
`{"retry": true}`) runs the refused Checks again on the same content; any other answer raises a new
review. Sending a new message also ends the review: the candidate is stale and
the model continues with the correction. Cancelling the Run stops it.

## Freshness and recovery

- The gate runs before the final-completion fence, so a message sent while a
  Check runs is accepted rather than rejected. It makes the candidate stale;
  the running Check finishes, and its evidence is reused if the files did not
  change.
- No candidate is formed while another operation runs in the Working Area. A
  Check whose Area changed or became busy after its content was read starts
  again on the new content without spending the allowance.
- Each Check attempt has an idempotency key derived from the Run, the Check,
  the content and the attempt number. After a process cut, starting the same
  attempt again returns the operation that already exists.
- Completed Runs record their **Completion Basis**: `verified` when Checks
  passed, `not_required` when none applied.

## Not yet available

Completion Exceptions, the dedicated Completion Review request kind, the
Completion Basis on Tasks, Artifacts and the API, Checks for remotely executed
Runs (they always pause for a review today) and the Completion decider hook
follow in later changes to #112.
