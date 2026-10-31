---
status: accepted
---

# Thread Attachments reach a model only when its Agent selects them

A file posted with a Thread message is never pushed into any Run. Every reader of the Thread sees that it is there, and an Agent's model sees its content only after that Agent selects it through `workspace_read` (`kind=attachment`, `representation=model_input`), under its own Run's authority and model capability. We rejected pushing media into "receiving" Runs because the Thread has no recipients: inventing them (the conversation's target Agent, or every running Run) would bill media to Runs that never asked for it and would still fail silently for Runs whose model cannot read the format. Selection keeps media consistent with how Agents already read Thread text, and an unreadable format becomes an explicit tool error the Agent can report instead of a silent gap it papers over.

A federated Run follows the same rule from the other side: its own Node first confirms its model can read the format, then fetches the bytes from the source Node, which rechecks the delegation and the Peer Mapping's authority on every fetch. Only the direct delegate may fetch; relays through intermediate Nodes wait for multi-hop delegation (#636).

## Consequences

- `POST /api/runs/{id}/message` with media stays as the one explicit push, for a person handing media to one Run.
- Revoking a delegation, disabling a Peer Mapping or deleting the message takes effect on the next selection or fetch; a Run that already selected deleted media is told it is unavailable.
