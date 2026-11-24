# A limit names what it protects, counts at its scope and says so when hit

Decided on 2026-11-24 in the #996 grilling (umbrella Issue #1117). The Execution Pod let each Scratch Space run at most 16 Background processes. When they were all busy, a new Live preview or stdio MCP start waited without a reason and then failed with a generic error. Nobody had decided on 16. It was the size of a thread pool split off in a fix so that Background processes stopped starving foreground commands. It counted per Scratch Space, while the threads and `kubectl exec` streams it spent belong to the whole Runner. A sweep of the code found about 40 more numbers like it. Most of them have no stated reason, and many were added in one bulk commit. Some make callers wait without a reason, for example two knowledge searches for the whole process. Some cut data off without saying so, such as Tool Asks past 100, where the newest are dropped. Some report the wrong cause: a busy media decoder tells the person their file is broken. ADR 0143 removed the limits that only stop Agents from growing. This decision covers every other limit: threads, connections, counts, pages and waits.

## Decisions

- **A limit names what it protects**: a resource such as Runner threads, database connections, Pod memory or a model's context, or a cost a person must not pay unknowingly. "Keep it small" is not a resource. The reason is written next to the number, in a comment, an ADR or the operations docs.
- **A limit counts at the scope of what it protects.** A Runner-wide resource is counted for the whole Runner. A per-Pod resource is counted per Pod, and one Tenant's work is counted per Tenant. Where Tenants share a resource, one Tenant must not be able to take all of it without the limit saying so.
- **A limit says so when hit.** A caller over it gets a refusal, or a waiting state it can see, that names the limit and its value and, where it can act on it, what holds the limit. A read that stops early pages on or says it was cut. A limit is never reported as another cause, never waits without a reason, and never cuts silently.
- **A limit that cannot meet all three is removed or rebuilt.** Removing it is preferred when the resource is already bounded somewhere else, for example by the Pod's process limit, a connection pool or a Budget.
- **New and changed limits follow this rule.** Existing ones are brought in line through #1117 and its sub-Issues, one pattern at a time.

## Considered Options

- **Keep each limit and explain it in its refusal.** This was the first answer in #996, which kept the 16 and made a start over it say why. It was rejected: a well-worded refusal of a limit that protects nothing still stops work for nothing.
- **Review limits case by case without a rule.** Rejected. Without a rule, a limit is added whenever a number feels safe, and nobody can tell later whether it is still needed.
- **Make every limit configurable.** Rejected. A setting moves the guess to the operator without saying what the number protects.

## Consequences

- #996 removes the per-Scratch Space limit of 16 Background processes and watches them per Pod. The Pod's process limit stays, and a start that hits it says so.
- Limits that already refuse with a typed reason naming the limit, but lack a stated basis for the number, are left until their area is next touched.
- Reviews of new code ask the three questions of every new number that bounds work or data.
- Extends ADR 0143.
