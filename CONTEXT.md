# Aidash

Aidash coordinates Agents and their durable work. This glossary records the shared language used in its capability and transaction designs; it is not an implementation specification.

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

## Distributed transactions

**Transaction manifest**:
The fixed definition of an Aidash atomic operation, identifying its coordinator, participating Nodes and intended resource changes.
_Avoid_: Mutable work request, execution plan

**Coordinator submission**:
The coordinator's durable acceptance of a transaction request for coordination. It is distinct from each participating Node's acceptance of its obligations.
_Avoid_: Participant admission, completed transaction

**Participant admission**:
A participating Node's durable acceptance of the exact obligations defined for it by a transaction manifest, under the authority checked at acceptance.
_Avoid_: Coordinator submission, temporary permission check

**Transaction trust**:
A Node operator's explicit permission for a peer to request participation in new Aidash transactions. It is distinct from peer authentication and subject permission to change a resource.
_Avoid_: Peer credential, subject authorization

**Transaction visibility barrier**:
The boundary that withholds ordinary access to a participating Node while a transaction's outcome is not yet safe to expose.
_Avoid_: Completed commit, resource permission

**Transaction completion**:
The state in which all participants have finalized the coordinator's durable decision and released their transaction visibility barriers.
_Avoid_: Request accepted, commit decision recorded

**Admitted transaction obligation**:
A Participant's responsibility for its fixed resource changes under an accepted manifest, which remains valid for recovery after the admitting subject or transaction trust is revoked.
_Avoid_: Current read permission, reusable execution grant

**Transaction decision**:
The coordinator's durable choice of commit or abort for one immutable transaction manifest. A commit decision is distinct from completed application and safe visibility at every participant.
_Avoid_: Transaction completion, delivery acknowledgment

**Transaction authority revocation**:
Withdrawal of authority to admit new transaction obligations, distinct from cancellation of obligations already admitted. A pending revocation has denied new work but has not yet resolved every admission that raced with it.
_Avoid_: Transaction abort, completed revocation while admission is uncertain

**Manifest disclosure authority**:
Explicit permission to disclose a transaction manifest's contents to its named recipient Nodes. It is distinct from permission to mutate a resource or trust a peer to participate.
_Avoid_: Transaction trust, blanket peer access
