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
every Run. A missing definition, withdrawn Catalog Approval or provider model
not found requires a new Agent version; transient provider errors retry and never
silently degrade the reference context. Copy, installation and federation do
not transfer private image references and fail explicitly.
For generated Agents, every ancestor Generation Policy must approve the same
exact reference embedding; a mismatch is rejected without widening those policies.

Thread Attachment storage, selection, indexing and Image Descriptions follow
#602 and are outside this decision's initial implementation slice. The ADR
number is provisional until the concurrent branches are merged.
