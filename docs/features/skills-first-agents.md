# Skills-first agent creation

Register reusable Skills, then select their exact versions when creating an agent.
The agent's additional instructions are optional when at least one Skill is selected.
Existing agents with custom instructions and no Skills remain valid. Generation
policy templates also accept Skills without additional instructions.

## Importing existing Skills

In Registry, choose `skill` in the registration form. Enter a public GitHub
repository, root tree (`/tree/<ref>`) or Skill directory (`/tree/<ref>/<path>`),
`SKILL.md` blob or raw URL, or upload a local `SKILL.md`. A
`skills.sh/<owner>/<repo>/<skill>` page URL loads
the registry's complete snapshot; a `skills.sh/<owner>/<repo>` URL lists
Skills from its GitHub repository. Repository URLs list discovered Skills for
selection.
The GitHub importer pins the source to a commit and copies files from the
selected Skill directory (up to 64 files and 256 KB total). Binary assets are
stored as base64 and oversized packages are rejected. Root-level Skills load
`SKILL.md` and common `references/`, `scripts/`, `assets/`, and `templates/`
directories. The importer
validates YAML `name` and `description` fields, preserves the complete Markdown
(including license metadata), and lets the operator review all imported text
before saving. Private repositories, arbitrary hosts, packs, and archive URLs
are not supported by this importer.
The UI links to the Anthropic and OpenAI Skills repositories; it does not bundle or
relicense their content. Register a new version when adopting an upstream update.

At runtime, an Agent under the legacy Exposure policy sees the Skill instructions
and the paths of its bundled files. It can read those files on demand with
`skill_read`, bound to its exact registered Skill version. Reads preserve UTF-8
character boundaries, and an unknown file path is returned as a recoverable tool
error. Bundled scripts remain text and are not run. Configure the tools a Skill
needs separately; do not infer tool permissions from `allowed-tools`.

An Agent registered with `"exposure": {"version": "deferred@1"}` loads Skills
on demand instead. Each bound Skill appears as a Discoverable capability with a
`skill_` Model alias. The capability index in the instructions lists it, and
`capability_search` and `capability_describe` show its description and packaged
files. `capability_load` makes its instructions resident in later requests,
within the Agent's `skill_bytes` budget. `capability_unload` removes them, and
a Skill Binding with `"exposure": "eager"` is resident from the first request.
Bundled files are read with `skill_asset_read`, using the Skill alias and
digest, the file path, and optional `offset` and `max_chars`. Like `skill_read`,
a page that would not fit the remaining request budget is cut short, and its
`next_offset` continues from the cut. Loading a Skill
never runs its scripts or grants permissions. See
[deferred capability exposure](../operations/registry-capabilities.md#deferred-capability-exposure).

Workspace and Skill text reads count Unicode scalar values in `offset`,
`max_chars`, `next_offset`, and `total_chars` wherever those fields are present.
Japanese characters and emoji each count as one scalar value; a combining mark
counts separately. Pinned Skill reads and `skill_asset_read` accept `max_chars`.
Encoded-byte resource limits still cap the returned text independently of that
character quota.

Format reference: <https://agentskills.io/specification>.
Sources: <https://github.com/anthropics/skills>, <https://github.com/openai/skills>.

## Personal reference documents

The agent form accepts PDF, Excel `.xlsx`, and `.txt`/`.md`/`.csv` files. It extracts
text in the browser, displays a preview, and allows removal before registration.
PDF text retains page numbers; Excel values retain sheet names and cell addresses.
It does not perform OCR, recalculate formulas, preserve formatting or images, or
store the original binary. Old `.xls` files must first be converted to `.xlsx`.

Limits: eight files, 10 MiB per file, 200 PDF pages, and 64 KiB of extracted UTF-8
text across all files. The selected model's context budget can impose a lower limit.
No content is silently truncated. Files without extractable text are rejected.
PDF.js and ExcelJS are loaded only when their respective file types are selected.

`POST /api/agents/personal` is operator-only and accepts an `entry`, `documents`,
and a required `Idempotency-Key` UUID. Registration, private document storage, and
the registration event share a database transaction. Retries preserve the assigned
agent ID; changing documents with the same key is rejected. The server validates
extracted text and bounds independently of the browser.

Documents live in `agent_knowledge`, bound to the exact agent ID/version. Registry
entries contain only their SHA-256 digest. Registry reads, discovery, package
metadata, and registration events do not include document names or contents.
At inference time, the original agent loads its documents as reference data in the
user context, separate from Skill and additional instructions. Execution sends this
data to the configured model provider and may produce derived content in artifacts.
Anyone authorized to run the agent can therefore cause it to use its attached data.

Private documents remain on their owning node; they are not transferred through
package installation, cloning, or federation. Copies fail explicitly if their
private documents are unavailable rather than executing without the references.
Generation templates cannot contain a personal agent's document digest. To use
personal documents, run the registered agent on its owning node. The first version
has no separate document update/delete endpoint; use a new agent version to change
its reference set.
