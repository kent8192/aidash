# Deliverables live in Node Drives

Decided on 2026-11-26 in the [#1019](https://github.com/kent8192/aidash/issues/1019) grilling. The Drive contract is in [#1020](https://github.com/kent8192/aidash/issues/1020), and the Artifact and Working Area removals are in [#1023](https://github.com/kent8192/aidash/issues/1023).

## Context

The Artifact table duplicates the final answer a Task already posts to its Thread. Files the Task wants to keep live in a Working Area tied to one person, Agent and Thread. A Node needs its own durable collection for work it shares with other departments.

## Decision

Every Node owns one Drive. People in the Tenant can read an open Drive. Closing a Drive restricts reads to people who can give work to the Node and its administrators. A person's private folder is visible to that person and Node administrators. The same checks apply to file previews, historical versions, and search results. People who can give work to the Node can publish and trash files they may write. A Subnode publishes to its creating Node with its creator's requester authority.

A Task may publish at any point. It selects Scratch Space files, gives a destination prefix and supplies a base version for each path it writes or removes. A concurrent change to any of those paths refuses the whole publication and returns the conflicting paths. Uploads transfer in parts and reuse tenant-scoped SHA-256 blobs. A publication changes the affected manifest entries in one transaction. The version names its author and, for an Agent publication, the Task. A publication estimated to take longer than a minute resumes from its saved plan and reports progress.

Direct uploads reuse the Reference upload, chunk and commit flow. Repeating the start request returns the saved byte offset, and repeating a chunk is safe when the bytes match. The server verifies the complete digest before it publishes a Drive File. The backing storage determines available capacity.

Every file version has an interval of Drive versions in which it is visible. Trash retains a deleted file for 30 days. References from a Deliverable or Correction Request retain the versions they need. Departed members' private folders enter the trash. During the Node deletion grace period, an administrator may move the Drive to another Node. The move preserves its ID and version history. The receiving Node must have an empty Drive. A restore of the original Node leaves the moved Drive with its new owner.

Drive publication carries an explicit index selection. A person's upload checks indexing by default. Drive search uses the Tenant's active embedding setting. Indexing splits text into passages of roughly 2,000 characters with 200 characters of overlap and line ranges. The index can extract text from PDF and Office formats, embed supported images, and match path and name for other files. The Tenant can reuse embeddings for the same digest and model. A publication with estimated indexing spend over USD 10 waits for the requester to confirm. The files are readable while indexing waits. Trash entries are absent from search.

The Completion Message includes the Task's published Drive versions, any Scratch Space files attached when it ended, and Outside Versions. Other Nodes receive results as Completion Message attachments. The Reference set stays pinned to its Agent version. Drive and Scratch Space storage are measured per Node. Charging it against Budgets is tracked in [#1156](https://github.com/kent8192/aidash/issues/1156).

## Consequences

Node files can outlive a Thread and be found by meaning. The Drive's history stores changed paths and retains the immutable content needed by Deliverables. A Task may publish to the Node Drive without waiting for a Goal verdict. A reader must still have access to the Drive and folder when opening a historical result.
