# Registry capability glossary

Aidash coordinates Agents and their durable work across independently operated Nodes. This glossary defines the capability contract vocabulary. See [Registry capability operations](registry-capabilities.md) for the current implementation boundary and remaining cutover work.

## Language

**Node**:
An independently operated Aidash participant with its own identity, resources, and authority.
_Avoid_: Worker, Agent

**Agent definition**:
A versioned description of an Agent's instructions and declared capabilities. A definition is distinct from a participant or an execution of its work.
_Avoid_: Run, worker

**Run**:
A durable execution of a Task using an exact Agent definition and its fixed execution dependencies. A Run is distinct from a model request or an individual capability invocation.
_Avoid_: Model request, tool call

**Execution policy**:
The effective authorization and resource constraints governing a Run's capabilities and actions. Declaring a capability does not grant permission to exercise it.
_Avoid_: Agent preference, Skill instruction

### Registry capabilities

**Registry**:
The catalog of immutable, versioned declarations that identify capabilities available for Agent work. Catalog membership is distinct from approval or permission to execute a capability.
_Avoid_: Execution grant, Marketplace

**Node provider**:
A Node's implementation of an identified capability contract. A provider is distinct from the versioned declaration through which an Agent references its operations.
_Avoid_: Descriptor, package, inference model

**Provider contract**:
The guarantees, operation meanings, and authority requirements a Node provider supports. Its identity is distinct from the Node release or a particular provider implementation.
_Avoid_: Node version, Agent instructions

**Tool descriptor**:
An immutable Registry declaration of a callable capability, its provider operation, and applicable constraints. Its existence does not authorize invocation.
_Avoid_: Tool implementation, execution grant

**Qualified descriptor identity**:
The identity of one exact capability declaration within its registering Node. The same name and version registered by another Node identify a different declaration.
_Avoid_: Unqualified tool name, model alias

**Core capability**:
A capability whose implementation and governing authority belong to the Node operator. A Core capability is declared through the Registry as a Built-in tool or a Host-distributed tool.
_Avoid_: Capability outside the Registry

**Registry integration**:
A declared capability served by an external service or Agent through an integration transport. Registry integration describes the capability's origin, rather than exclusive membership in the Registry.
_Avoid_: All Registry tools, Core capability

**Built-in tool**:
A system-origin Tool required by Aidash's own features and declared as an immutable capability of its Node. Its availability is distinct from authority to read or change resources.
_Avoid_: Marketplace package, ungoverned operation

**Host-distributed tool**:
A Node-operator capability distributed as a Marketplace package for explicit tenant installation and approval. Installing it does not confer authority over its target resources.
_Avoid_: Third-party integration, automatically authorized builtin

**Binding**:
An Agent definition's reference to a declared capability, with its chosen name and additional applicable restrictions. A Binding expresses availability independently of permission to use the capability.
_Avoid_: Permission grant, mutable installation selection

**Required Binding**:
A capability dependency that must be available for the Agent's declared execution or support workflow. Its required availability is distinct from permission to perform each operation.
_Avoid_: Unconditional execution grant, removable preference

**Default Binding**:
A capability included by the Node's default Agent declaration unless the Agent explicitly removes it. A recorded default is distinct from an explicit capability request or a future change to the Node's defaults.
_Avoid_: Latest runtime configuration, implicit permission

**Binding snapshot**:
The fixed account of the exact capability declarations and restrictions selected for one Run. The snapshot is distinct from the current authority governing their use.
_Avoid_: Latest configuration, permanent execution grant

**Model alias**:
The name by which a model addresses a bound Tool. An alias is distinct from the Tool's identity and authority.
_Avoid_: Tool identity, catalog resource

**Tool bundle**:
A versioned group of separately declared Tool operations distributed and bound together. A bundle is distinct from an executable operation.
_Avoid_: Multi-operation invocation, execution grant

**Admitted operation**:
An asynchronous capability invocation accepted under an exact provider contract and execution scope, with an identity that survives waiting and recovery. Its admission is distinct from completed effects or permission for a later caller to inspect or cancel it.
_Avoid_: Tool alias, completed effect

### Context dependencies

**Agent memory scope**:
The retained-memory boundary of an exact Agent definition within its Home node and Workspace. It is distinct from a particular Run's reasoning context or permission to read and write that memory.
_Avoid_: Run-private memory, globally shared memory

**Context source**:
An identifiable source of information explicitly declared for an Agent's reasoning within an allowed scope. A source is distinct from its current contents or a delivered observation.
_Avoid_: Prompt text, unqualified search result

**Private reference source**:
An immutable reference attached to an exact Agent definition within its owner's authorized scope. It is distinct from a publicly distributed package asset.
_Avoid_: Public package file, current mutable document

**Source observation**:
A durable account of exact source contents and revisions delivered at one reasoning boundary. It records what was read independently of the source's later state and continuing disclosure authority.
_Avoid_: Binding snapshot, permanent read permission

### Execution placement

**Home node**:
The Node that owns a Workspace and its shared Tasks, Messages, and published Artifacts, including when another Node performs the work.
_Avoid_: Execution node, central server

**Execution node**:
The Node responsible for carrying out a Run. In remote execution it is distinct from the Home node.
_Avoid_: Home node, Workspace owner

**Receiver admission**:
The Execution node's acceptance of exact remote work and its declared dependencies under the applicable authority and supported contracts. Acceptance is distinct from starting or completing the Run.
_Avoid_: Remote grant, activation

### Distribution

**Marketplace package**:
A versioned distribution of declared capabilities with its owner, provenance, permissions, and dependencies. A package is distinct from an installation or permission to execute its contents.
_Avoid_: Installed capability, execution grant

**Tenant installation**:
A tenant's installed package and local configuration. An installation is distinct from approval to use its exact contents.
_Avoid_: Node provider, catalog approval

**Installation revision**:
An immutable configuration of a tenant installation for an exact package version. A revision is distinct from the package publication or its current approval.
_Avoid_: Package version, current configuration of every Run

**Tenant catalog approval**:
A tenant's explicit admission of exact capability declarations for authorized use. Approval is distinct from installation and the authority required for each operation.
_Avoid_: Installation, subject permission, execution grant

**System catalog approval**:
A Node's admission of an exact system-origin builtin declaration for use by its own features. It is distinct from a subject's authority over the operation's target resources.
_Avoid_: Tenant resource grant, Marketplace installation

**Approval set**:
The exact pending installation revisions and permission closure a tenant's approver reviews and admits together. A set is distinct from packages that become available after that review.
_Avoid_: Latest defaults, automatic package activation
