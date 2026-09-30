# Aidash

Aidash coordinates Agents and their durable work. This glossary records the shared language for execution, capabilities, transactions, Agent collaboration, event subscriptions, federation, evidence gathering, Web research and Marketplace distribution; it is not an implementation specification.

## Language

**Harness**:
The runtime responsible for advancing Runs and governing the capabilities available to Agents.
_Avoid_: Model, Agent

**Run**:
A durable execution of a task using an exact Agent definition version. A Run is distinct from a single model request or capability invocation.
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

**UI event stream**:
An ordered view of recorded mesh activity available to a human observer under their current authority. Observing an event does not accept an Agent's responsibility to act on it.
_Avoid_: Agent event subscription, recipient delivery, Run activation

**UI event replay**:
Continuation of an observer's recorded event history after a previously received position, subject to current authority. Replay does not repeat Agent handling or external effects.
_Avoid_: Historical event routing, Run recovery, re-execution

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

## Marketplace distribution

**Marketplace package**:
A versioned distribution of an Agent, Tool or Skill definition with its author, declared permissions and dependencies. A package is distinct from its installation or permission to execute it.
_Avoid_: Installed Agent, execution grant

**Package owner tenant**:
The tenant to which a Marketplace package belongs. Ownership is distinct from authorship credit or an individual subject's permission to operate on the package.
_Avoid_: Author, package publisher

**Package publisher**:
The authenticated actor responsible for a package publication on behalf of its owner tenant or within the operator's legacy scope.
_Avoid_: Author, package owner tenant

**Package author**:
The authorship credit displayed in a package's metadata. This credit is distinct from its authenticated publisher and owner tenant.
_Avoid_: Package publisher, authorization owner

**Package sharing**:
The owner tenant's explicit inclusion of another tenant in an exact package version's distribution audience. Audience membership is distinct from a subject's permission to browse, read or install the package.
_Avoid_: Public access, execution permission

**Package identity**:
The identity of a package within its repository Node and owner tenant, distinct from its display name and individual published versions.
_Avoid_: Unqualified package name, author name

**Package version**:
An immutable publication belonging to one qualified package identity. Another version is a separate distribution resource rather than a replacement for an earlier publication.
_Avoid_: Latest package, installation revision

**Package summary**:
The discovery description of a package, distinct from its complete definition and dependency contents.
_Avoid_: Manifest, installed definition

**Redistribution consent**:
The source owner's explicit permission to distribute an exact package version onward to a stated audience. It is distinct from the recipient's permission to install or use the package.
_Avoid_: Installation permission, catalog approval

**Tenant installation**:
A tenant's installed package and local configuration, available to that tenant's subjects according to their permissions. An installation is distinct from tenant catalog approval.
_Avoid_: Node-wide installation, personal installation, execution approval

**Installation revision**:
An immutable configuration of a tenant installation for an exact package version. A new installation revision is distinct from a new publication of that package.
_Avoid_: Package version, catalog policy revision

**Pending installation revision**:
An installation revision awaiting the tenant's approval for use. It is distinct from any earlier approved revision that remains active.
_Avoid_: Active configuration, partially installed package

**Active installation revision**:
The approved installation revision selected for new Runs. It is distinct from an earlier revision still pinned by admitted Runs.
_Avoid_: Latest package version, configuration of every running Agent

**Installed definition**:
The exact Registry definition bound to a tenant installation revision, with that tenant's local configuration and dependency bindings. Its existence is distinct from approval for execution.
_Avoid_: Source manifest, node-wide override

**Legacy Marketplace resource**:
A preexisting operator-managed package or installation without tenant ownership. It is distinct from a tenant-owned copy explicitly adopted from that resource.
_Avoid_: Public package, default-tenant package

**Distribution withdrawal**:
The withdrawal of authority for further Marketplace disclosure or acquisition. It is distinct from the installing tenant's withdrawal of approval to use a previously installed definition.
_Avoid_: Uninstall, execution revocation

**Registry registration**:
The admission of a versioned definition into the Registry. Registration is distinct from Marketplace publication and permission to use the definition.
_Avoid_: Publish, catalog approval

**Tenant catalog approval**:
A tenant's explicit admission of an exact Registry definition for authorized use, including its installation revision when local configuration applies. Approval is distinct from installation and the subject permissions required for individual operations.
_Avoid_: Installation, execution grant


## Issue #74 implementation

Subject-scoped Marketplace routes now use qualified package identities, explicit
per-version audiences and live source consent, with pending immutable installation
revisions and separate operator approval/activation. Existing Runs retain exact
approved references; configuration never mutates the active definition. Scoped
HTTP/SSE handoff retains current authority through bounded serialization and queueing,
including browser sessions, while durable Runs remain independent of browser logout.

The additive migration starts the compatibility gate disabled. Legacy package and
node-wide overlay behavior remain explicit operator operations; adoption produces an
independent pending tenant copy. See
[operations and acceptance coverage](docs/operations/marketplace-authorization.md)
for rollout, actions, integration tests and English/Japanese dashboard checks.
