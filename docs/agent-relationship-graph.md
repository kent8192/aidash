# Agent relationship graph

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
  neighbor to position it. The focused agent stays anchored. **Fit graph** and
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

## Implementation and limits

`agent-graph/model.ts` is a pure projection and layout module. `canvas.tsx` is a
replaceable SVG renderer matching the existing dashboard's rendering approach;
this feature adds no graphics dependency and does not replace the global Mesh.

A view displays at most 120 nodes and 360 edges. Configuration relationships are
prioritized; a spanning tree is retained before additional edges are capped.
Omission counts are shown rather than implying completeness. These are rendering
limits, not server pagination: the projection still processes the supplied State.
A federated fleet-wide graph requires a separate authorized, paginated projection.

Run `npm run test:graph` in `web` with Node 22.16 or newer for dependency-free graph
tests. `npm test` runs these tests followed by the existing Playwright suite,
including `tests/agent-graph.spec.ts`. The browser suite requires the normal built
application at `AIDASH_E2E_URL` (default `http://127.0.0.1:18080`).
