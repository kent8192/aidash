# Aidash

Aidash coordinates Agents and their durable work. This glossary records the shared language for Agent collaboration, event subscriptions and Web research.

## Language

**Harness**:
The runtime responsible for advancing Runs and governing the capabilities available to Agents.
_Avoid_: Model, Agent

**Run**:
A durable execution of a task using an exact Agent definition version. A Run is distinct from a single model request or capability invocation.
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

**Agent definition**:
A versioned description of an Agent's instructions and capabilities. Multiple logical Agents may use the same definition.
_Avoid_: Participant, worker

**Logical Agent**:
An independently identified Agent participant with its own role, subscriptions and work history. Sharing an Agent definition does not make two participants the same logical Agent.
_Avoid_: Agent definition, worker replica, Run

**Workspace participation**:
An Agent's membership in a Workspace for a stated role. Permission to read the Workspace alone does not constitute participation.
_Avoid_: Read access, subscription

**Event subscription**:
A logical Agent's declared interest in specified Workspace events within its participation scope. A subscription identifies potential work and does not itself grant permission to read or act.
_Avoid_: Workspace membership, execution permission

**Workspace event**:
A recorded occurrence associated with a Workspace, such as a message, task change, published Artifact or goal change. The occurrence is distinct from any participant's reaction to it.
_Avoid_: Agent command, Run

**Recipient delivery**:
The record of a particular Workspace event being considered for a particular logical Agent's handler. Delivery to one recipient does not fulfill delivery to another.
_Avoid_: Broker receipt, completed work

**Response decision**:
A recipient's disposition toward a Workspace event: respond, skip or defer. A decision to respond expresses intent to undertake work, not proof that the work has completed.
_Avoid_: Authorization decision, execution result

**Correlated work**:
Work explicitly associated with the same Task or collaboration thread for a logical Agent. Similar wording alone does not establish that two requests belong to the same work.
_Avoid_: Similar request, entire Workspace

**Event handler**:
A named response responsibility of a logical Agent: claiming an existing Task, contributing input to correlated work, or starting distinct work. Its identity remains the same when its matching subscription conditions are edited.
_Avoid_: Worker, broker consumer, subscription condition

**Reaction chain**:
The connected work and events caused by an originating request or occurrence. Its branches share a bounded allowance for autonomous reactions.
_Avoid_: Single Run, conversation history

**Historical event routing**:
An explicitly requested consideration of past Workspace events within a selected scope. It is distinct from reading history for context or recovering work already owed to an existing subscriber.
_Avoid_: Context retrieval, automatic recovery, unconditional re-execution

**Follow-up Task**:
A distinct piece of work explicitly related to earlier completed work. It does not reopen the earlier Task or continue its finished Run.
_Avoid_: Revived Run, child of a completed Task

**Suppressed delivery**:
A recipient delivery prevented from proceeding by disabled participation, disabled subscription or revoked authority. Restoration of access alone does not make it active work again.
_Avoid_: Worker outage, completed work, temporary semantic uncertainty

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
