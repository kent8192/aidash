# Skills-first agent creation

Register reusable Skills, then select their exact versions when creating an agent.
The agent's additional instructions are optional when at least one Skill is selected.
Existing agents with custom instructions and no Skills remain valid. Generation
policy templates also accept Skills without additional instructions.

## Importing existing Skills

In Registry, choose `skill` in the registration form and import `SKILL.md`.
The importer validates YAML `name` and `description` fields, preserves the complete
Markdown (including license metadata), and lets the operator review it before saving.
The UI links to the Anthropic and OpenAI Skills repositories; it does not bundle or
relicense their content. Register a new version when adopting an upstream update.

This is an instruction-file importer, not a complete execution environment for
arbitrary Skill packages. Scripts, referenced files, and assets are not imported or
run. Configure the tools a Skill needs separately. A Skill that requires a shell,
Python, or missing companion resources will need those capabilities before its
workflow can be followed. Do not infer tool permissions from `allowed-tools`.

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
