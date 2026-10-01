# Graph View and agent relationships

Open an agent from **Agents** or **Registry**. Its detail dialog includes an
agent-centered relationship graph. The existing Mesh topology, Run controls and
Task details remain available.

## Relationships

Solid edges show configuration: the agent's exact model, tools, skills and cluster
references, and a cluster's explicitly configured coordinator. They do not imply
that a model or tool has been called. Dashed edges show recorded local
Agent → Run → Task relationships. Repeated execution remains separate Run nodes;
configuration dependencies and execution records are never merged.

Entity identity includes Node, kind, ID and version. A reference to an unavailable
version is shown as **Not available in this view**, not silently replaced with a
newer version. This label does not distinguish absence from restricted access.

Only the current authorized State snapshot is used. Discovery metadata is not
joined into this graph. Private reference documents, document names, document
contents and `agent_knowledge` are not requested or projected. Existing server-side
authorization remains authoritative. If a refreshed snapshot removes the focused
agent, the dialog removes its graph and old metadata.

## Navigation

- Drag the background to pan, use the wheel or zoom buttons to zoom, and drag a
  neighbor to position it. The focused agent stays anchored. **Fit entire graph** and
  **Center selection** adjust the viewport without changing any execution state.
- Select a node and choose **Expand neighbors** to explore another recorded hop.
  **Open details** opens the existing entity, Run or Task dialog. Missing references
  cannot be opened or expanded.
- Filter by node type or disable runtime activity. **Reset exploration** returns
  to the initial neighborhood. The root remains visible regardless of type filters.
- **Relationship list** provides native buttons and a table for keyboard and
  screen-reader users, including edge direction, relationship type and status.
  English and Japanese labels are provided.

Node positions are retained during snapshot, status and membership updates in the
active graph. There is no continuously running force simulation or per-message
animation. Switching away from the graph/list or opening another detail may reset
the viewport.

## Agent relationship implementation and limits

`agent-graph/model.ts` is a pure projection and layout module. `canvas.tsx` is a
replaceable SVG renderer matching the existing dashboard's rendering approach;
this feature adds no graphics dependency and does not replace the global Mesh.

A view displays at most 120 nodes and 360 edges. Configuration relationships are
prioritized; a spanning tree is retained before additional edges are capped.
Omission counts are shown rather than implying completeness. These are rendering
limits, not server pagination: the projection still processes the supplied State.
Graph View uses a separate authorized, paginated projection for direct Peers;
this agent-centered dialog remains local.

Run `npm run test:graph` in `web` with Node 22.16 or newer for dependency-free graph
tests. `npm test` runs these tests followed by the existing Playwright suite,
including `tests/agent-graph.spec.ts`. The browser suite requires the normal built
application at `AIDASH_E2E_URL` (default `http://127.0.0.1:18080`).

## Workspace Graph View

Open **Graph View** from the collaboration sidebar. The workspace selector scopes
workspace, goal, task, conversation, artifact and recorded run activity. Authorized
registry configuration remains available for context. The new canvas uses the
existing Cytoscape dependency and Lucide icons. The user-provided Mesh + Accent
logo kit supplies the sidebar, compact mark, sign-in wordmark and favicon; the
original SVG/ICO files are kept in `web/public/brand` without redrawing.

Five perspectives share the same authorized projection:

| Perspective     | Organization                                                                                                                                                       |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Agent mesh      | People and goals above tasks; agent clusters, tools, artifacts and discovered peers in separate groups.                                                            |
| Collaboration   | A workspace at the center of its people, conversations, goals, agents, tasks and artifacts.                                                                        |
| Knowledge graph | Goals, tasks and artifacts with their explicit producers, tools, models, skills and conversations.                                                                 |
| Execution flow  | Request authors, delegating agents, task cards, executing agents and produced artifacts. Current task counts and a time-scaled event timeline accompany the graph. |
| Topology        | Local and directly authorized peer nodes, separate agent clusters and configured tool links. Available to authorized Subjects and operators.                       |

**Agent relationships** retains the previous agent-focused exploration and topology
controls. Agent-focus links open this perspective so exact entity selection stays
intact across navigation, reload and browser history. The existing agent detail
dialog is also unchanged.

### Controls and details

- Search names and resource IDs. Matching nodes and their immediate neighbors are
  retained within the active type and relation filters.
- Filter node types and relation types, focus the selected neighborhood, switch to
  the accessible relationship table, or reset the filters.
- Select structured, force or circle layout. Drag nodes, pan, zoom, fit, center the
  selection, navigate with the minimap, or use fullscreen. Polling retains positions
  and camera state; changing perspective or layout computes a fresh arrangement.
- Select a node to inspect its overview, connected tasks, recorded events and
  directed connections. Existing entity/task dialogs and workspace navigation are
  reachable from this panel. Artifact contents are expanded only on request.
- Select an activity window (hour, day, week, month, or all available records).
  It filters completed runs and events; active runs and current task/configuration
  state remain visible. Timeline points are spaced by recorded timestamps, with
  exact dates in tooltips and labels. Select a point to open its referenced task
  or artifact; points without an available target are disabled.
- On compact screens, filters are collapsible and the inspector becomes a bottom
  sheet. Native buttons, focus styles, labeled controls and the relationship table
  support keyboard navigation. Both Japanese and English are supported.

### Fit entire graph

Choose **Fit entire graph / 全体表示** beside the zoom controls to recover an
overview after panning or zooming. The Scan icon, visible label and tooltip stay
available on compact screens, including with the inspector open. Focus the button
and press Enter or Space to activate it.

One activation centers all current nodes, displayed labels, edges and group
boundaries in the measured area clear of the inspector, minimap and controls.
It includes disconnected and off-screen elements and already-loaded authorized
remote projections within the current filters and rendering limits. Selection,
Workspace, filters, expanded Peers and node positions are retained.

Fit caps magnification at unit scale and permits an overview below the usual
minimum zoom for large graphs. Manual zoom remains gradual at that scale. Padding
is 36 CSS px, or 16 CSS px in compact usable areas, reduced further only when the
available area is extremely small. The button is disabled with an accessible
explanation when the graph is empty or cannot be measured safely.

The workspace mesh, agent neighborhood and agent detail graph share this behavior.
Initial display and deliberate perspective/layout changes frame the graph;
ordinary polling, filtering, expansion, resizing and opening the inspector retain
the camera until an explicit fit. Temporary empty filter results also retain the
camera. Fit does not load resources or rerun a layout. See the
[fit design and local validation](design/2026-09-30-issue-96-entire-graph-fit.md).

### Data boundaries

`collaboration/mesh-model.ts` projects only explicit, version-pinned configuration
references and recorded activity. It does not scan free text or arbitrary JSON for
relationships. Goals represent the existing workspace goal; humans are explicit
non-agent task/artifact/conversation principals. Registry IDs include origin,
kind and version. Missing exact references stay unavailable.

Graph View asks the local Node for directly authorized Peer boundaries. Subjects
can use Topology when their local policy grants `federation.graph.read`; named
operators can select one authorized receiving tenant. Expand a Peer to fetch a
versioned, bounded projection from that Node. **Configured** describes a Peer
boundary, not its health. An authorized empty projection, a denial, an
unavailable Peer and an older Peer without the graph endpoint have distinct
states. The next page control replaces the current remote window, keeping
browser memory bounded. Collapse removes that Peer's projected data without
resetting other expanded Peers. Reload starts with boundaries only.

The receiving Node decides which Registry, Workspace, Goal, Task, Run, Artifact
and Conversation nodes, exact relationships and activity markers the viewer
may see. A Goal inspector shows the full current body only after Workspace
read authorization. Remote Task descriptions, Artifact contents, message bodies,
human requests and raw configuration are not sent. Remote inspector and search
use only projected fields, never matching local IDs as a fallback. A local Run
can connect to a remote Agent only when that Agent is in the current authorized
projection. A selected local Workspace limits remote operational records to
explicitly related resources; selecting all Workspaces allows the receiving
Node's own authorized Workspaces. Peer expansion does not traverse onward to a
third Node.

Expanded projections refresh within 30 seconds while the view is visible.
Denial, connection loss, expiry, collapse, context change and reload remove
remote names, relationships, Goal body, inspector and search data. A continuation
page rechecks both Nodes' current authority and must match the projection
generation. See [the authorization guide](authorization.md#federated-graph-view-authority)
for the grant and endpoint contract.

The renderer receives display metadata, not agent configuration, credentials,
message bodies or artifact content. Node details use the existing authorized State.
Success rate, CPU usage, latency, approval status, document version history and
health are not fabricated when the current API does not supply them. The activity
chart is a histogram of recorded related events, not a telemetry estimate.

A perspective renders at most 180 nodes and 600 edges, after filtering. Omitted
counts are shown. The timeline displays the most recent 80 records with an omission
count; the event detail tab displays the most recent 100. These limits apply to the
supplied snapshot, not server pagination. Compound containment is represented by
its group frame. In dense views, edge labels are emphasized for the selection;
the relationship table retains every displayed relationship and its direction.

### Verification

From `web`, run `npm run test:graph`, `npm run test:collaboration`, and
`npm run test:ui` after the normal frontend build. `AIDASH_UI_PORT` selects an
isolated preview port for the UI suite (default 18082). `tests/mesh-scene.mjs`
contains synthetic records for tests and local visual QA only; production never
imports or inserts them. The mesh browser tests cover the five perspectives,
three layouts, filters, inspector navigation, snapshot preservation/revocation,
subject isolation and compact viewports, and save visual captures in the ignored
`web/test-results` directory.

The fit tests additionally check full visual containment and centering within
1 CSS px, recovery from an off-screen view, native keyboard activation in both
languages, inspector/resize geometry, single/empty/large graphs and loaded remote
projections. They compare camera and graph state independently of the fit helper.
