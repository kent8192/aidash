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

## Amendment (2026-11-20, implementation of #1008)

Calls made while implementing, recorded here so the decisions above are read with them:

- **A Duty's Thread opens when the Duty is registered**, not with its first Task: a Duty needs its Node, Scratch Space and a place for its events and notices before anything runs. It opens in the Node and Scratch Space of the Thread it was registered from, as its Requester, with the mission as its first message, and the Duty keeps both (`thread_id`, `origin_thread_id`).
- **Reply threads are dropped, not promoted.** Aidash is undeployed: the migration deletes them, their replies stay as flat messages of the Thread they were in, and the columns and Approval Modes that named them go. A Working Area is keyed on (home Node, Thread, Agent, owner), so Areas that differed only by a reply thread, conversation or Root Task are merged.
- **The Reinhardt app label stays `workspaces`.** The migration ledger stores (app, name), so the label, its migration directory and its module path keep the old name; the app now holds Threads, Tasks and Duties.
- **Stored identifiers keep their names where renaming would break immutable records**: the builtin `workspace_*` tools (saved Agent versions pin them; `thread_*` and `task_wait` are the new defaults), the Registry Binding source `aidash.workspace-observation` and its `allow_workspace_retrieval` key, the memory sharing scope value `workspace` and the read-dependency kind `workspace_events`.
- **A Thread's title is its only mutable attribute**, and a deleted Thread is marked (`deleted_at`) and refuses every later read and write; the Working Areas its owners kept stay theirs to list, download and clean up, but nothing is restored into it.
- **Node administrators read every Thread of their Node** and its events (`node.administer` on the Thread's Node), unless a policy denies them the Thread by name; it gives them no other action in the Thread except deleting it.
