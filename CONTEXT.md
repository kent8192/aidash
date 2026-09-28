# Aidash

Aidash coordinates Agents and their durable work. This glossary records the shared language of Agent execution, federation and evidence gathering.

## Language

**Harness**:
The runtime responsible for advancing Runs and governing the capabilities available to Agents.
_Avoid_: Model, Agent

**Run**:
A durable execution of a task by an exact Agent version. A Run is distinct from a single model request or capability invocation.
_Avoid_: Model request, tool call

**Core capability**:
A facility provided and governed by the Harness independently of Registry integrations. Availability and permission to use a capability are distinct.
_Avoid_: Registry integration

**Registry integration**:
An externally supplied, versioned capability referenced through the Registry.
_Avoid_: Core capability

**Execution policy**:
The effective authorization and resource constraints governing a Run's capabilities and actions.
_Avoid_: Agent preference, Skill instruction

**Web search**:
The discovery of public Web sources relevant to an Agent's query. Search results identify potential evidence; they do not establish that the source contents have been read.
_Avoid_: Page reading, verified answer

**Page reading**:
Retrieval and examination of the contents of an identified Web source. It is distinct from discovering that source through search.
_Avoid_: Search result, search snippet

**Source citation**:
A reference connecting an Agent's statement to the Web source used as its evidence.
_Avoid_: Search rank, provider-generated answer

**Search service operator**:
The node operator responsible for the external search-service account and its usage costs.
_Avoid_: Requesting user, Agent

**Research evidence**:
The source identity, retrieval time and bounded source passages used to support an Agent's answer. Retained evidence describes what was examined at that time, even if the live page later changes.
_Avoid_: Live page, search ranking

**Web disclosure approval**:
An eligible person's authorization to send a particular search query or page request outside the node when the Run contains nonpublic or unclassified information. It is distinct from enabling Web search for the Agent.
_Avoid_: Capability enablement, blanket network permission

**Source record**:
The identity of a candidate Web source discovered or supplied for a Run. A source record does not imply its contents have been examined.
_Avoid_: Read page, verified evidence

**Document snapshot**:
The fixed contents obtained from one retrieval of a Web source. A later retrieval creates a different snapshot even when the source address is unchanged.
_Avoid_: Live page, search snippet

**Evidence fragment**:
An identified passage from a document snapshot made available to the Agent and eligible to support a source citation.
_Avoid_: Unread passage, generated summary

### Federation

**Node**:
An independently operated Aidash participant with its own identity, resources and authority.
_Avoid_: Worker, Agent

**Home node**:
The Node that owns a Workspace and its Tasks and published Artifacts, including when another Node performs the work.
_Avoid_: Execution node, central server

**Execution node**:
The Node responsible for carrying out a Run. In remote execution it is the receiving Node, distinct from the Home node.
_Avoid_: Home node, Workspace owner

**Subject chain**:
The originating subject and the Agents on whose behalf work is delegated. Delegation is constrained by every member's authority.
_Avoid_: Peer identity, Agent version

**Remote grant**:
The Home node's recorded, time-bounded authorization for a specified remote Task, subject chain and executor. It is distinct from the receiving Node's consent to perform the work.
_Avoid_: Bearer token, Receiver admission

**Receiver admission**:
The Execution node's recorded acceptance of a Remote grant for a particular local identity and exact Agent version. Acceptance is distinct from starting the Run.
_Avoid_: Remote grant, Activation

**Activation**:
The transition from an accepted remote assignment to a runnable execution.
_Avoid_: Receiver admission, Task completion

**Home command**:
A remote Run's request to read or change Home-node resources within its authorized scope.
_Avoid_: Unrestricted Workspace access, local working-area operation

**Pinned definition**:
The exact version and contents of an Agent or execution dependency to which an authorization applies.
_Avoid_: Latest version, compatible replacement
