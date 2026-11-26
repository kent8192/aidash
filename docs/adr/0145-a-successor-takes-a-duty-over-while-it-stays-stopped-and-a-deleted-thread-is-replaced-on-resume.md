# A successor takes a Duty over while it stays stopped, and a deleted Thread is replaced on resume

Decided on 2026-11-24 in the #991 grilling. Since #1014 a Duty opens its own Thread when it is registered (ADR 0140), and its Tasks, memory bank and conversation with its Requester live there. Deleting that Thread stops the Duty (`thread_deleted`), but nothing could replace the Thread: `Patch` has no Thread field, and editing or resuming checks rights and the definition against the deleted Thread. A Duty waiting for a successor (`requester_left`) was worse off, because ADR 0134 let the successor take it over only by resuming it, and resuming refused on the deleted Thread. The only way out was deleting the Duty and losing its ticks, Budgets, triggers, webhook and scripts.

This ADR replaces the successor rule of ADR 0134 ("the Schedule runs again only when the successor resumes it as their own action"). The rest of ADR 0134 stands.

## Decisions

- **Taking a Duty over is its own action, and the Duty stays stopped.** Only the named successor takes over, and only a person of the Tenant. Taking over makes them the Requester, clears the named successor, changes the halt from `requester_left` to `paused`, drops the Duty's event bookings and, if the Duty's own Thread still exists, makes them its owner. It checks neither Thread rights nor the definition and does not change the credential; resuming does. Nobody is notified, because it is the person's own action.
- **Only the Requester edits and resumes.** Resuming no longer takes a Duty over: a successor first takes over, may edit, then resumes. The dashboard may offer one "take over and resume" button that makes both calls. #557 decision 9 (only the Requester edits) is unchanged.
- **A deleted own Thread is noticed when the Requester next edits or resumes, not through the halt.** The halt keeps saying only why the Duty stopped; a paused Duty whose Thread is deleted stays `paused`, and one waiting for a successor stays `requester_left`. The Duty's page says its Thread was deleted and that resuming opens a new one.
- **The Requester's first edit or resume after the deletion opens a new own Thread.** It opens in the deleted Thread's Node and Scratch Space (a deleted Thread keeps its row), titled with the Duty's title, with the Duty's mission as its first message plus one line naming when the previous Thread was deleted, sent by that Requester. The Requester must be allowed to open a Thread there as anyone would: use the Node (`node.use`), create a Thread (`thread.create`) and use the Scratch Space as its creator or a member. Taking a Duty over grants none of these, so a successor who is not a member of that Scratch Space is refused and asked to get added. If that Scratch Space can no longer be used, the edit or resume is refused and says so; the Duty is never moved to another Scratch Space silently. Rights and the definition are then checked against the new Thread. An edit alone leaves the Duty stopped with its new Thread.
- **Memory does not carry over.** The memory bank of the deleted Thread stays with it and is removed with it; the new Thread starts with empty memory. The Duty's ticks, Budgets, triggers, webhook and scripts are kept.
- **Duties from before #1008** (no origin Thread, working in the Thread they were registered in) follow the same rules: when that Thread is deleted, the next edit or resume opens an own Thread for them, and the deleted Thread becomes their origin so that a later takeover hands the new Thread over like any own Thread.

## Considered Options

- **Refuse deleting a Duty's own Thread while the Duty exists.** Rejected: wiping a Thread, for example one holding a leaked secret, would require deleting the Duty and its history, which is the loss #991 set out to avoid.
- **Let an edit point a Duty at another Thread.** Rejected: a Duty owns its Thread (ADR 0140), and borrowing someone else's Thread would bring back the ownership questions #1008 removed.
- **Let the successor submit a replacement definition while taking over.** Rejected: it adds a second way to edit a Duty, by someone who is not yet its Requester.
- **Record a deleted Thread by overwriting the halt with `thread_deleted`.** Rejected: it would erase `requester_left` and with it the named successor.
- **Move the old memory bank to the new Thread.** Rejected: the person deleting a Thread usually wants its content gone, and memory can hold the same content.

## Consequences

- No state of a Duty is a dead end: its Requester, or a successor after taking over, can always edit, resume or delete it.
- `resumption` judges one rule (the Requester resumes), and the successor branch moves to taking over.
- A Duty whose Thread was deleted loses its memory on purpose.
