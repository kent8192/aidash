# Graph View Node execution regions and Workspace shared data

Status: Q1-Q10 accepted on 2026-09-30. The user excluded arrow work in Round 1,
accepted the other Round 1 recommendations, and accepted all Round 2
recommendations. No product decisions remain open. This is the agreed design;
local implementation and synthetic browser verification are complete. A live
authorized three-Node deployment has not been checked.

This document records the design interview for
[Issue #95](https://github.com/kent8192/aidash/issues/95). Accepted decisions are
design agreements, not evidence of implemented behavior. The user's scope
clarification takes precedence over the Issue's arrow requirements and the
reference image's illustrative arrows.

## Verified starting point

The source checkout and `origin/develop/0.1.0` were inspected at
`3cb803544c37ebbeb413bc8554accb0a06d916fa`. Design work uses branch
`feat/issue-95-node-execution-regions`, created with `wtp` in its own worktree
and renamed for implementation.
The original checkout remains unchanged.

- [Architecture](../architecture.md) assigns authoritative Workspace Tasks,
  Messages and Artifacts to the Home node. Each Execution node owns its Run
  journal and uses the Home API for remote Workspace reads and mutations.
- [The existing Graph View](../agent-relationship-graph.md) has five perspectives,
  three layouts, Workspace and time selectors, filters, a minimap, an inspector,
  peer expansion/retry and an accessible relationship table. The separate
  Agent-centered relationship view has its own navigation and renderer.
- `web/src/collaboration/mesh-model.ts` identifies Registry resources by Node,
  kind, ID and version. Registry configuration currently remains broadly visible
  even with one Workspace selected; recorded operational resources are scoped.
  A distinct Message vertex is not part of the current graph resource kinds.
- `web/src/collaboration/mesh-canvas.tsx` currently creates Agent/Cluster and
  selected resource-type groups for the mesh and topology perspectives.
  Infrastructure Nodes remain vertices. Other perspectives do not use these
  compound groups.
- `web/src/collaboration/federated-graph.ts` merges bounded authorized Peer
  projections. Existing projection fields can identify their source Node and
  resources, but do not expose a complete Agent-to-Agent result-sharing record.
  That gap is outside this task after the user's Q1 clarification.
- Local graph construction can represent a remotely executing Run under a
  Home-qualified display key. Placement must use the recorded executing Node
  rather than assuming that every existing graph key identifies the executor.
  Coexisting representations of one Run need reconciliation without merging
  different executing Nodes merely because a resource ID matches.
- [Federated graph authority](../authorization.md#federated-graph-view-authority)
  requires current authorization on both Nodes. Peer connectivity does not grant
  transitive authority. Remote body content remains excluded, and stale or
  unauthorized projections are removed from the canvas, search and inspector.

## Accepted scope and meanings

### Q1: Exclude arrow work

The user answered that arrows are not the subject of this task and are
unnecessary. Do not add or complete delegation, result-sharing or Home-access
arrows, and do not expand the API to support those arrows. Preserve existing
relationship controls and supported resource relationships as part of the
existing Graph View. The original Issue's new communication-arrow acceptance
criterion is superseded for this design; arrow work is not a completion gate.

### Q2: Result sharing terminology

The recommended definition was accepted: result sharing requires an explicit
communication/response record identifying the sending Agent, receiving Agent
and result. Task completion or Artifact creation alone does not establish it.
This is a glossary agreement for future work, not an arrow implementation
requirement in this task.

### Q3: Workspace-related resources

For a selected Workspace, show Agents with a verified relationship through
authorized Task, Run, Conversation or explicit participation records, together
with their explicitly referenced configuration. Being registered or connected
alone does not establish Workspace participation.

Use only relationships available in the current authorized data; this decision
does not authorize adding a participation subsystem, mining arbitrary text or
requesting private content. Compute Workspace relevance separately from the
displayed vertex/relationship filters so hiding a Run does not silently change
the meaning of Workspace scope. If relevance cannot be established, do not
infer it from matching names, unqualified IDs or Peer connectivity.

### Q4: Shared grouping principles in five modes

Apply the same distinction between execution placement, Home ownership and
configuration references in Agent mesh, Collaboration, Knowledge graph,
Execution flow and Topology. Preserve each perspective's resource selection and
purpose. A perspective without shared-data resources, such as Topology, has no
shared-data group. Preserve the separate Agent-centered relationship view.

### Q5: Agent and Run identity

Distinguish graph Agents by execution Node, Agent ID and definition version.
Keep each Run as a separate vertex in the corresponding execution region.
Do not merge different Nodes or versions based on an Agent's display name.
Preserve the existing activity-window rules, including active Runs and filtered
completed history. This graph identity does not redefine Logical Agent or
Harness worker identity.

### Q6: Placement by resource meaning

| Resource                                                 | Placement                                                        |
| -------------------------------------------------------- | ---------------------------------------------------------------- |
| Agent and Run                                            | Corresponding Node execution region                              |
| Workspace, Goal, Task, Conversation/Message and Artifact | Separate shared-data group with authoritative Home attribution   |
| Tool, Model, Skill and Cluster                           | Configuration reference group, with registering Node attribution |
| Human                                                    | Outside the execution and data groups                            |

Node execution regions are peers; a Workspace does not contain Nodes. Shared
data is a logical ownership grouping alongside execution regions, not a new
storage service. Clusters retain their explicit membership relationships
instead of becoming execution-containment parents. The Message row describes
ownership; Q9 retains the existing Conversation representation and excludes
individual Message vertices from this change.

Agreed terminology is recorded in [the glossary](../../CONTEXT.md). The
architectural boundary is recorded in
[ADR 0010](../adr/0010-separate-graph-execution-placement-and-data-ownership.md).

## Design tree

- Accepted: placement and ownership are independent; arrow work is excluded.
  - Accepted: Workspace relevance, five-mode scope, Agent/Run identity and
    configuration/human placement.
  - Accepted: Q7 Peer states, Q8 multiple Workspaces, Q9 conversation
    representation, Q10 region controls and layout behavior.
  - Frontier: empty after the user's acceptance of all Round 2 recommendations.
- Deferred: new communication arrows, result-sharing projection and associated
  API additions. These are not dependencies of this implementation.

## Round 2: accepted decisions

The user accepted all four recommendations on 2026-09-30.

### Q7: Unexpanded, empty or unavailable Peers

Keep authorized Peer expansion, state, next-page and retry controls in the
existing Peer control area. Draw an execution region when that Node has visible
Agents or Runs in the current scope; do not create empty execution regions merely
because a Peer is configured. An unavailable projection removes its old resources
under the existing expiry/revocation rules. A still-authorized Peer control can
remain with its state, while loss of permission to know that Peer removes the
control too. An empty current page or filter result does not prove zero activity.

### Q8: All Workspaces

Create a separate shared-data group for each authorized `(Home node, Workspace)`
pair. Share a Node execution region across the displayed Workspaces and preserve
the distinct Agent/Run identities already accepted in Q5. Identify the Workspace
in resource labels or inspector context where needed. Never merge Workspaces
based on their title or bare resource ID, or infer a Home owner from the renderer's
local Node. Resources without sufficient current ownership evidence do not
acquire a guessed owner.

### Q9: Conversations and Messages

Use the existing Conversation vertex to represent the conversation within the
Home-attributed shared-data group. Do not add individual Message vertices or
fetch message bodies for this task. Preserve existing authorized details and
navigation. The ownership language can mention Messages without promising a
new Message graph API or per-message rendering.

### Q10: Regions, layouts and interactions

Keep Structured as the default layout, with peer-level execution regions and a
separate shared-data area, plus referenced configuration and Humans outside
those regions. The reference image guides the hierarchy and compact icon style;
the number of Nodes and resources remains data-driven. Force and Circle arrange
the same semantic groups without mixing their membership. Normal refresh
preserves positions and camera state; a requested layout change or reset can
arrange them again.

Keep resource selection and the existing inspector; selecting a region header
can use the existing Node details. Treat region borders as presentation, not
additional domain resources. The Node visibility control toggles execution
frames/labels without overriding the separate Agent and Run filters. Search,
focus and resource filters operate on resources, after Workspace relevance has
been established; frames follow their remaining children. Fit and the minimap
account for the full region bounds. Keep the relationship list, keyboard access,
compact-screen controls and Japanese/English labels.

## Consequences for implementation

These are consequences of the accepted behavior, not additional API or domain
features:

- Infrastructure Nodes become labeled region boundaries in these five
  perspectives instead of standalone graph vertices. An authorized Home label
  remains meaningful even if the Home has no currently displayed execution
  region. Region borders do not grant visibility to hidden resources.
- Determine placement and ownership independently. A resource's Home attribution
  is not its execution placement, and `remote` describes the viewer's location
  relative to a Node rather than who owns every related resource.
- Workspace relevance must survive perspective, relation and resource-filter
  changes. The existing server omits Run candidates for some perspectives and
  kind filters, so display filters cannot be the sole input to collecting the
  authorized evidence needed to establish that relevance. Use existing
  authorized data paths; absence of proof must not fall back to the full Peer
  Registry or inferred participation.
- Reconcile duplicate presentations of a Run only when the existing authorized
  records prove the same executing Node and Run identity. Keep the original
  resource references usable for selection and details; do not rewrite public
  resource identity or merge same-ID resources from different Nodes.
- A page or resource-filter change can remove all children of a region. Removing
  that empty frame must preserve the Peer controls and must not suggest that the
  Node has no work anywhere. Any counts describe the displayed data, subject to
  existing limits and pagination.
- Preserve existing public API and authorization semantics, projection bounds,
  explicit expansion, stale-data removal and nontransitive Peer access. No
  schema migration, storage-ownership change, per-message projection or new
  communication-arrow feature is part of this design.

## Acceptance matrix

These checks describe acceptance evidence. The focused unit and browser tests
use authorized synthetic records; they do not replace the real three-Node
runtime check.

| Scenario                                               | Required evidence                                                                                                                                                                                                             |
| ------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Authorized Workspace on three Nodes                    | Real UI shows each Agent/Run in its execution region; Home-owned data is in a separate Home-attributed group; no Workspace contains Nodes and no standalone infrastructure Node vertices remain in these perspectives.        |
| Home with no visible local execution                   | The shared-data group still identifies its Home; no empty execution region is invented merely to provide that label.                                                                                                          |
| Configuration and Human context                        | Referenced configuration stays outside execution/shared-data groups with registering Node attribution; Humans remain outside; existing Cluster relationships remain available.                                                |
| Selected Workspace and hidden Run kind                 | Unrelated Registry Agents are omitted; hiding Runs or relation lines does not change established Workspace relevance.                                                                                                         |
| All Workspaces                                         | Separate groups use Home-qualified Workspace identities; execution regions and identical Agent/Run identities are shared without per-Workspace duplication.                                                                   |
| Equal names/IDs and different Agent versions           | Different Nodes, Workspaces and definition versions remain distinct; the same proven Run is not duplicated through both Home and execution projections.                                                                       |
| Time window and repeated Runs                          | Active Runs retain existing visibility, completed history follows the selected window, and each retained Run stays distinct in the correct execution region.                                                                  |
| Unexpanded, empty or missing projection                | Peer controls remain available under current authority; a current page with no visible execution resources has no execution region and does not claim zero global activity.                                                   |
| Unavailable, denied, unsupported or expired projection | Old remote resources, search entries and inspector data are removed under existing rules; retry/status remain only where current Peer visibility permits them.                                                                |
| Revocation, next page and context switch               | No stale selection, metadata or membership survives; replaced remote pages do not accumulate; a connected third Node is not treated as authorized transitively.                                                               |
| Five perspectives and three layouts                    | Resource meaning and group membership stay consistent; Structured is the default; Force/Circle preserve semantic groups; ordinary refresh retains camera and positions.                                                       |
| Controls and accessibility                             | Region-header and resource inspection, independent frame/Agent/Run filters, search/focus, pan/zoom/fit, full-region minimap, relationship list, keyboard access, compact viewports and Japanese/English labels remain usable. |
| Conversation representation                            | Existing Conversation vertices appear in Home-owned shared data; no individual Message vertices, new body fetches or invented communication arrows are introduced.                                                            |

Focused projection/layout tests and the existing graph, collaboration and
browser suites check the local implementation. A real authorized three-Node UI
capture, including a missing or unavailable projection, remains to be done.
The Issue's original communication-arrow criterion is excluded by the user's
Q1 decision; all retained criteria above still require implementation evidence.

## Documentation delivery

The agreed design, glossary and ADR are saved in the dedicated worktree.
The existing `CONTEXT.md` and older ADRs are tracked even though `.gitignore`
contains design-document exclusions. ADR 0010 is added explicitly to preserve
the decision with this implementation. No ignore settings were changed.

Product implementation was requested after the interview. The frontend now
scopes Workspace relevance before presentation filters, renders Node execution
regions and separate Home-owned data/configuration regions, and keeps Peer
expansion and resource inspection. The browser test uses a synthetic A/B/C
projection with the same Agent and Run names/IDs on B and C to check distinct
execution regions and one Home-owned shared-data group. Live deployment remains
separate from that test; no GitHub Issue update or PR was requested.
