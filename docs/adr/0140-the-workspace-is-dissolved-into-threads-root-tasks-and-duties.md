# The Workspace is dissolved into Threads, Root Tasks and Duties

Decided on 2026-11-17 and 2026-11-18 in the agent activation grilling (umbrella Issue #1007). A Workspace was created from a person's goal (`POST /api/conversations`), every Task had to live in one, and parent and dependency Tasks had to share it. The goal text was read only as the first message, the first Task's description and a 512-character observation. Agents woken by events and standing Duties have no goal to start from, and a Tenant run like a company needs larger units — departments and standing responsibilities — than one goal.

## Decisions

- **A person's request creates a Thread and a Root Task, and no Workspace.** The goal text becomes the Root Task's description. The Tasks an Agent splits it into hang below the Root Task.
- **A Root Task chooses the Scratch Space; its child Tasks inherit it.** Tasks a Duty starts use the Duty's Scratch Space.
- **Access is decided per Thread and per Duty.** A Thread's creator and the Tenant's policy decide who may read or act in it, and its Root Task and child Tasks inherit that. A Duty's requester and the Tenant's policy decide for the Duty, its Thread and its Tasks. Node administrators see everything in their Node.
- **A Subnode works in the Task tree given to it and its Thread**, not in a Workspace of its own.
- **The Thread takes over what the Workspace carried.** Every Task belongs to exactly one Thread; a Duty owns its own Thread and its Tasks belong there. The Working Area, the semantic index, memory banks, events, Node purge and retention key on the Thread. Parent and dependency Tasks share a Thread, which may hold several Root Tasks over time. Today's reply threads become top-level Threads, and messages are flat inside one.
- **Node administrators are a policy role.** A new `node.administer` action, held by Tenant Administrators by default, lets a person see everything in a Node and edit its Node Instructions.
- **The dashboard lists Threads, Duties and an org chart** instead of Workspaces.

## Considered Options

- **Keep the Workspace as a container above Threads.** Rejected: it would remain a goal-shaped box that event-woken and standing work does not have, and a second access unit beside the Thread.
- **Make the Node the only container.** Rejected: a department holds many unrelated requests, each needing its own access and desk.

## Consequences

- `workspace.*` policy actions, the `workspace` resource kind, `/api/workspaces` routes and Workspace-centric screens are replaced; Aidash is not deployed, so no compatibility layer is kept and existing Workspace data is migrated into Threads and Root Tasks or dropped.
