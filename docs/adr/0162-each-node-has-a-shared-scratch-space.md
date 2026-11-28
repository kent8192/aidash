# Each Node has a shared Scratch Space

Decided on 2026-11-26 for #1020. Amends [ADR 0111](0111-agents-work-in-a-shared-scratch-space-and-keep-only-what-they-register.md).

Each Node has one default Scratch Space. Its Threads, Duties and Subnodes use it by default. A Run executing for another Home uses the receiving Node's default space. Everyone who may give work to the Node and its administrators may see its contents. Deployment Administrators have no Tenant authority to read it. A person may create a named space with explicit members and select it when opening a Thread.

Tasks start in `/work/threads/<thread>`. A Task started by a Duty works in `/work/duties/<duty>`. People and agents can keep common clones, caches and tools in `/work/shared`. Tasks run concurrently and coordinate their own file changes. The default space has 8 CPU, 32 GiB of memory, 500 GiB of storage and 32 foreground command slots. Named spaces start with 4 CPU, 16 GiB, 100 GiB and 8 slots.

## Considered options

- A separate space for each Thread repeated repository checkouts, caches and build output. A named space lets people isolate work when they need to.
- An idle wipe could erase files while an open Task waits for a person or another Task. Explicit clearing gives each owner control over when to remove files.
- The Working Area queue serialized Tasks that can work concurrently in their own directories. A shared Scratch Space provides filesystem semantics and leaves ordering to the work itself.

## Consequences

- Scratch files persist until a person clears them or the Node is purged. The Execution Pod may still stop while idle. A Node administrator clears the default space after reviewing the affected running Tasks. The creator may clear or delete a named space. Thread participants may clear their Thread's directory, and deleting the Thread removes that directory.
- A copy into a Scratch Space requires read authority on the source for every person who may use that space. Files can be read into model context under the Run's own authority. Thread Attachments and Drive files remain available through those authorized reads.
- Publish work that needs File versions to the Node Drive. Attach files to a message to convey results directly. Reference sets remain pinned to the Agent version and available to the file tools.
- Command and approval journals keep an execution scope for current authority and receipts.
