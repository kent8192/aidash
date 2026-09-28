# Aidash

Aidash coordinates Agents and their durable work. This glossary records the shared language for execution and Agent capabilities.

## Language

**Harness**:
The runtime responsible for advancing Runs and governing the capabilities available to Agents.
_Avoid_: Model, Agent

**Run**:
A durable execution of a task by an exact Agent version. A Run is distinct from a single model request or capability invocation.
_Avoid_: Model request, tool call

**Harness worker**:
A runtime executor that advances Runs on behalf of logical Agents. It is distinct from the Agent whose identity and definition a Run uses.
_Avoid_: Agent, Agent subscription

**Runnable Run**:
A nonterminal Run eligible to advance under its current execution constraints. Readiness to advance does not establish permission for every subsequent action.
_Avoid_: Received event, authorized effect

**Run activation**:
A request for Harness workers to consider advancing a Run whose durable state may permit progress. It does not confer execution ownership or create another Run.
_Avoid_: Agent selection, Run creation, process startup

**Activation generation**:
The ordered version of a Run's need to be reconsidered for progress. Handling an activation generation does not by itself establish that every accepted input has been processed.
_Avoid_: Run revision, observed input position

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
