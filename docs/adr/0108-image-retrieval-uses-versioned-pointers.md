---
status: proposed
---

# Image retrieval uses versioned pointers and an embedding's declared inputs

A Workspace index embeds text and eligible Working Area image Files in the same
vector space. An embedding definition declares its accepted inputs; text is
required for queries and image input must be explicitly enabled. Omitting inputs
means text only. Each current image File version is a separate point, with no
image bytes in the point.

Image ingestion records Pending, Indexed, Unsupported or Failed. Unsupported
has the reason `modality`, `too_large` or `limit`. The index's image count and
size limits apply in publication order. Existing images without a publication
order are evaluated by Area ID and manifest order. An existing image without its
owner's Run authority stays Pending until an authorized owner Run can supply it;
other work continues. Deletion or replacement releases the slot. A freed slot
is not automatically filled; a rebuild or republication re-evaluates candidates.

Image search returns a path, exact File version and score, never image bytes or
an Evidence citation. An image query is limited to the current Run's Working
Area. Opening uses `workspace_read`: an image-capable model receives a selected
image through the existing model-input path, while a text-only model receives
its name and metadata. The File version and authority are checked again on open.

Reference Document images remain private to their owning Node and Agent version.
Registration validates and embeds every image before committing the documents,
pinned embedding and collection together. Runs query that collection with the
pinned definition. Its results have a separate context section from Workspace
results; scores from the two stores are not merged. Image names appear in the
reference listing; original bytes are retained separately and are not loaded in
every Run. Original image bytes are stored as binary in a Tenant-isolated child table that cascades with the owning knowledge row. Registration
checks for an idempotent replay before calling the provider, closes the preflight
scope during the external call, and rechecks authority before the atomic write.
A missing definition, withdrawn Catalog Approval or provider model
not found requires a new Agent version; transient provider errors retry and never
silently degrade the reference context. Copy, installation and federation do
not transfer private image references and fail explicitly.
For generated Agents, every ancestor Generation Policy must approve the same
exact reference embedding; a mismatch is rejected without widening those policies.

Thread Attachment storage, selection, indexing and Image Descriptions follow
#602 and are outside this decision's initial implementation slice. The ADR
number is provisional until the concurrent branches are merged.

## Amendment: Thread Attachment images (#622 part 2)

An image Thread Attachment enters the same Workspace index when its message is
posted, in the posting transaction and under the poster's authority, whether a
person or an Agent posted it. A poster who may not write to the index, or a
Workspace without an enabled index, leaves it out; configuring or enabling the
index later enters earlier images under the configuring person's authority,
oldest message first. Its states and reasons are those of a Working Area image,
and it shares the index's one image limit: an attachment is admitted while
attachment and Working Area images together hold fewer slots than the limit,
and the File publication ledger counts admitted attachments against its own
admissions. A reconfiguration reconsiders them first: posted images keep their
posting order and fill the room Working Area images hold no slot for, and the
ledger then rebuilds into what remains. They never count against the text source
quota. Deleting the attachment, with its Thread, retires the entry and its
vector. An entry keeps its poster's saved authority, like a text entry put over
HTTP, so an image leaves the index when that authority can no longer be restored.

An image has an optional Image Description of at most 2,000 characters,
required when an Agent posts the image through `workspace_message`
(`image_descriptions`, by path). Only the poster writes or replaces it. It is embedded together with the image as one
input, so its words find the image, and its point records a digest of the
image bytes and the description: an edited description makes the old point
stale until the image is embedded again. Points still hold no bytes and no
description.

An attachment match names its message and attachment and carries the file
name, size, digest, Image Description and up to 1,000 characters of the message
text, never the bytes. The pointer, like the entry, is shown only to a reader
who can read the message now. When automatic context delivers an attachment match, the Run's recorded read
is rechecked like a message match: whoever reads its outputs must still be able
to read that message. A match is not a selection: the Agent opens it
with `workspace_read kind=attachment`, and only an explicit
`representation=model_input` selection puts the image into its next inference.
A model whose Registry modalities do not include images is not refused: opening
gives it the Image Description, file name and message text, and no selection.

A Thread Attachment is an image query only if the querying Run's own history
shows a completed `workspace_read` selection of that attachment with the same
bytes. A Run that only listed, read the metadata of, or was given the words of
an attachment has not seen it, and a selection a compaction has removed from
the history must be made again. The attachment is read under the Run's current
message authority before embedding and again after the vector search. The
same rule applies when the query also searches the agent version's private
Reference Document images.

`workspace_search` and `workspace_message` move to version 1.1.0 for these
inputs, and Agent versions registered earlier bind 1.1.0 too: `workspace_message`
1.0.1 and `workspace_search` 1.0.0 resolve as 1.1.0. This is an owner decision
(2026-11-13). Version 1.1.0 only adds inputs, so every earlier call behaves the
same, and without it an earlier Agent version could not post an image, because
each posted image needs a description. It deliberately departs, for these two
tools only, from #640's rule that a pinned tool does not change.
`workspace_read` keeps its pins, because its version decides whether an Agent
can select a Thread Attachment at all; `workspace_message` 1.0.0, which has no
`files` input, is also kept.
