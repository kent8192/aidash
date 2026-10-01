# Explicit whole-graph fit

Status: implemented and verified locally on 2026-10-01. Round 1 was accepted in full on 2026-09-30
and Round 2 on 2026-10-01 (user response in each round: "全て推奨").
All twelve recommendations are agreed; no product decisions remain open.
The checks below cover the local frontend with synthetic authorized API fixtures;
they do not establish the version or behavior of a deployed application.

Issue: [#96](https://github.com/kent8192/aidash/issues/96).
Related presentation work: [#95](https://github.com/kent8192/aidash/issues/95).
Inspected baseline: `develop/0.1.0` at
`3cb803544c37ebbeb413bc8554accb0a06d916fa` on 2026-09-30; source findings
rechecked against the same baseline on 2026-10-01.
Worktree branch: `feat/issue-96-entire-graph-fit`.

## Requirements established by the issue

- Improve the existing fit action alongside zoom controls. Provide Japanese and
  English text, a recognizable icon, a tooltip and native keyboard activation.
- Fit all current graph elements, including off-screen and disconnected elements,
  loaded authorized remote resources, rendered labels, edges and grouping regions.
- Respect existing Workspace, perspective, search, type, relationship,
  authorization and expansion boundaries. Fit must not fetch or expand resources.
- Adjust zoom and pan using the current positions and the current usable viewport.
  Preserve selection, inspector state, filters, Workspace and graph arrangement.
- Handle empty and small graphs safely; a fixed zoom floor must not obstruct an
  overview of a large graph. Repeated activation must be stable.
- Ordinary graph updates must not repeatedly override the user's camera.
- Apply the behavior to graph canvases with camera controls, including agent views.
- Verify camera geometry and state preservation with automated and browser checks,
  including keyboard activation, compact layouts and recovery from off-screen pan.

## Inspected baseline evidence

These findings describe the baseline before implementation.

- [MeshCanvas](../../web/src/collaboration/mesh-canvas.tsx) uses Cytoscape, an
  icon-only `Scan` fit button and `cy.fit(undefined, 36)`. Its limits are 0.12–2.5.
  Node labels are separate HTML buttons in a transformed layer; they are not
  Cytoscape node labels. Group and edge labels are rendered by Cytoscape.
- Mesh initialization and perspective/layout changes already fit. Ordinary updates
  retain camera and positions except when the canvas was previously empty. The
  minimap's keyboard activation is another existing whole-graph fit entry point.
- [CytoscapeCanvas](../../web/src/collaboration/cytoscape-canvas.tsx), used by agent
  neighborhood exploration, has a text fit button and limits of 0.15–4.
- [GraphCanvas](../../web/src/agent-graph/canvas.tsx), used in agent details, has an
  SVG camera and an independent fit calculation. It uses an 800×460 viewBox, fixed
  position margins and the [model's](../../web/src/agent-graph/model.ts) 0.2–3 zoom
  clamp; its fit target already caps magnification at 1.
- [Mesh CSS](../../web/src/collaboration/mesh.css) places camera controls and a
  minimap over the canvas. The compact inspector is an overlaid bottom sheet.
  Actual intersections need measurement; nominal screen width is insufficient.
- [Mesh projection](../../web/src/collaboration/mesh-model.ts) limits the filtered
  result to 180 vertices and 600 edges by default and reports omissions.
- [Cytoscape documentation](https://js.cytoscape.org/#cy.fit) defines fit as zoom
  plus pan over the supplied collection and padding in rendered pixels.
  Its [bounding-box API](https://js.cytoscape.org/#eles.boundingBox) includes
  Cytoscape nodes, edges and labels by default. Separate HTML labels require
  additional bounds from the application.
- The interview used source-inspection findings rather than a reproduced browser
  defect. Subsequent local regression testing reproduced the fixed zoom floor
  leaving a large graph outside the viewport before the implementation.
- The installed Cytoscape 3.34.1 `src/core/viewport.mjs` clamps fit to `minZoom`
  and `maxZoom`. Its zoom-range setter changes limits without moving the camera;
  the application must define a floor that also preserves subsequent manual zoom.
- Baseline mesh browser checks require HTML label containment within `.mesh-stage`
  after perspective changes. They do not establish containment of all visual
  elements in the unobscured viewport after explicit fit activation.
- The mesh empty-state branch unmounts the canvas. Q6 therefore requires camera
  continuity across temporary empty filter results, not just avoiding an extra
  `fit()` call in a mounted renderer.

## Design tree

The issue establishes membership/state preservation and frontend-only scope.
Round 1 settles product behavior. Its answers unlock geometry, zoom-limit
lifecycle, exceptional viewport handling and the final verification contract.
Round 2 resolves those remaining branches. The accepted policies below are the
implementation contract; reversible implementation details can be decided within
these boundaries without reopening the interview.

### Round 1: accepted decisions

All six recommendations were accepted. Reopen them only if new evidence exposes
a contradiction or the user changes the requirement.

| ID  | Decision                                          | Accepted policy                                                                                                                                                                                                                               |
| --- | ------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Q1  | Meaning of "entire" when the projection is capped | Fit the current post-filter, post-limit graph, including off-screen members. Keep omission counts; do not expand the 180/600 limits or load other pages.                                                                                      |
| Q2  | Discoverability on compact screens                | Show `Scan` plus `全体表示` / `Fit entire graph`, retaining text on compact screens by wrapping/repositioning controls. Keep native Enter/Space activation and avoid introducing a new global shortcut.                                       |
| Q3  | Panels and controls versus fit area               | Preserve open panels and fit into a measured, unobscured rectangular area. Keep the fit control reachable above/outside a compact inspector; accept a smaller overview instead of hiding graph elements behind controls.                      |
| Q4  | Magnification policy                              | Cap automatic fit at scale 1; allow a finite positive fit scale below the existing zoom floor when necessary. Prioritize the overview over label readability at extreme sizes; preserve manual zoom-in for inspection.                        |
| Q5  | Camera transition                                 | Apply one immediate zoom/pan update without animation. Stable repeated activation and predictable reduced-motion behavior take priority over a cinematic transition.                                                                          |
| Q6  | Existing automatic camera behavior                | Retain framing on initial nonempty display and deliberate perspective/layout changes. Do not add refitting on ordinary refresh, filtering, expansion, inspector changes or window resizing; explicit fit uses the resulting current geometry. |

### Round 2: accepted decisions

All six recommendations were accepted on 2026-10-01. Preserve them unless new
evidence exposes a contradiction or the user changes the requirement.

| ID  | Decision                                           | Accepted policy                                                                                                                                                                                                                                                                                                                                                                                              |
| --- | -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Q7  | Padding in a small usable viewport                 | Use 36 CSS px normally and 16 CSS px when the usable rectangle's shorter dimension is below 360 CSS px. In extreme positive-size areas, cap padding at one quarter of that shorter dimension so the interior remains positive. Measure current panel/control intersections rather than using fixed inspector widths.                                                                                         |
| Q8  | No drawable graph or usable area                   | Keep the camera unchanged for an empty graph. Disable fit while there is no drawable graph or the visible fit area is zero; explain an unavailable viewport in localized accessible text. Reject non-finite geometry without partially fitting a subset. Re-enabling does not trigger fit automatically.                                                                                                     |
| Q9  | Manual zoom after an overview below the old floor  | Permit gradual zoom in/out around the fitted level without snapping to the former floor. Derive the effective floor from the original floor, the scale required by current fit geometry and the current zoom; never raise it above the current zoom or move the camera just to update a limit. Retain each renderer's current manual maximum.                                                                |
| Q10 | Rendering/update races and empty-filter continuity | Make fit one request completed against current committed geometry. Coalesce repeated pending requests and cancel stale requests after manual camera input, context changes or unmount. Preserve camera across transient empty filter results; only the first nonempty display in a new graph context gets initial framing. Do not schedule later refits for polling, font arrival or other ordinary updates. |
| Q11 | Common contract across renderers and #95           | Reuse each existing camera and renderer. Share a pure fit calculation fed by per-renderer visual bounds and the usable viewport; include actual rendered HTML/SVG text extents, edges/arrows and current group boundaries. Complete #96 independently of #95; future execution regions must contribute their visual bounds through the same contract.                                                        |
| Q12 | Proof that the action fulfills its name            | Assert complete containment and centering numerically with at most 1 CSS px rendering tolerance, not just element intersection or screenshots. Cover all three canvases, mesh perspectives/layouts, both languages, compact inspector, disconnected/remote/large/small/empty graphs, keyboard controls, repeated clicks, resizing and preserved state/no fit-triggered fetches.                              |

## Fit contract

### Membership and visual bounds

Take the currently committed graph as the input. Fit every member regardless of
its current screen position, including disconnected components and already-loaded
authorized remote projections. Existing projection limits and omission notices
remain unchanged. Fitting does not fetch another page, expand a Peer, reset a
filter or widen authorization.

Each renderer contributes the visual bounds of its current nodes, node labels,
edges, edge labels, arrowheads, selected-resource styling and grouping regions.
Mesh must combine Cytoscape bounds with the separate HTML label layer. The SVG
renderer must account for its actual rendered text and curves rather than adding
fixed margins to node centers. Include what is rendered, including an ellipsized
label's displayed extent; hidden full text in a tooltip is not a graph element.
Use current positions, including manual node moves. Do not run a layout as part
of fit.

Normalize measured bounds into the renderer's camera coordinate system so they
do not depend on the previous pan or zoom. If any required geometry is non-finite
or cannot be measured, leave the camera unchanged and explain why fit is
unavailable; do not silently drop that element from the result.

### Usable viewport and camera calculation

Measure the current canvas and intersecting panels and controls at activation.
Choose a deterministic unobscured rectangle inside the canvas, accounting for
the inspector, minimap and camera controls where they overlap it. The rectangle
selection and control placement are implementation details subject to Q3, Q7
and the containment checks. Keep the fit button and its text reachable when the
compact inspector is open.

For a usable rectangle with shorter dimension `s` in CSS pixels, use
`padding = min(s / 4, s < 360 ? 16 : 36)`. Measure padding in CSS pixels even
when the renderer uses a different coordinate system. The SVG adapter must map
the measured viewport and padding through its current screen transform rather
than treating the fixed viewBox dimensions as screen pixels.

The common pure calculation selects the largest positive finite scale that
contains the complete visual bounds inside the padded rectangle, capped at the
renderer's unit camera scale. It maps the center of those bounds to the center
of that rectangle and returns zoom and pan together. The existing renderer
applies the result in one immediate camera update without animation. Repeated
activation with unchanged geometry must produce the same framing within the
rendering tolerance.

An empty graph, a zero-size usable rectangle or invalid geometry produces no
camera update. Fit is unavailable with a localized accessible explanation.
Becoming available again does not itself trigger a fit.

### Zoom limits and request lifecycle

The effective manual zoom floor must allow the required overview scale and must
never exceed the current camera scale. Updating a limit must not itself move
the camera. Manual zoom after fit proceeds gradually from the fitted scale;
preserve the current manual maxima of 2.5 for Mesh, 4 for neighborhood exploration
and 3 for the SVG agent graph. Empty or unavailable geometry retains the current
camera and usable limits rather than generating an invalid zoom value.

A click or native Enter/Space activation creates one fit request. If the DOM and
renderer geometry are still being committed, complete it against the current
committed geometry; coalesce pending repeated requests. Manual camera input,
an explicit graph-context change or unmount invalidates an older pending request.
Do not schedule additional fits when fonts arrive or ordinary graph data changes.
Route existing explicit whole-graph fit entry points, including minimap keyboard
activation, through the same behavior.

Initial framing remains available for the first nonempty display of a graph
context and deliberate perspective/layout changes. Preserve camera continuity
across a temporary empty filter result by retaining the mounted renderer.
Ordinary polling, search/type/relationship filtering, remote expansion, selection,
inspector changes and resizing do not introduce automatic framing. Their updated
geometry is used by the next explicit fit.

## Verification contract

These requirements were agreed during the interview; local results follow below.
Use independent containment and state assertions rather than computing the
expected result with the same helper being tested.

| Area                | Required evidence                                                                                                                                                                                                                                                   |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Pure geometry       | Every contributed visual bound lies inside the padded usable rectangle; the complete bounds are centered; camera values are finite with positive zoom; fit never exceeds unit scale; identical inputs are stable.                                                   |
| Graph shapes        | Disconnected components, very wide and tall arrangements, large graphs requiring scale below the old floor, one node, a small graph and an empty graph.                                                                                                             |
| Rendered extents    | Mesh HTML labels, Cytoscape node/edge/group labels, selected styling, SVG text/curves/arrows and current group boundaries remain contained.                                                                                                                         |
| Viewport changes    | Resize before activation, desktop inspector, compact inspector, minimap/control intersections and zero available area; use current dimensions and CSS-pixel padding.                                                                                                |
| Browser interaction | Recover from deliberately off-screen pan and arbitrary zoom in one activation; exercise mouse, Enter and Space; retain visible text and reachable controls in both locales and compact layouts; repeated clicks remain stable.                                      |
| State preservation  | Compare Workspace, perspective, filters, selection, inspector visibility, expanded Peer set, graph membership and node positions before and after fit. Verify no fit-triggered request, expansion or layout run.                                                    |
| Camera continuity   | Polling, filters, expansion and resizing preserve manual camera state; an empty filter result followed by restoration retains the camera; pending requests do not override later manual input; manual zoom after a low-scale fit does not snap to the former floor. |
| Supported views     | Mesh's five perspectives with structured/force/circle layouts where available, agent neighborhood exploration and the SVG agent detail graph. Include an already-expanded authorized remote projection and preserve existing authorization tests.                   |

Browser assertions permit at most 1 CSS px error for complete containment and
centering. Screenshots may supplement those assertions. Until #95 is implemented,
use current compound groups as the boundary fixture; when execution regions and
Home-owned shared-data groups are rendered, they must contribute to this same
bounds contract without changing fit membership or camera semantics.

## Implementation and local validation

[Shared geometry](../../web/src/graph-fit.ts) calculates the largest padded fit
at or below unit scale and selects a deterministic unobscured rectangle.
[DOM measurement and request handling](../../web/src/graph-fit-view.ts) clip the
current viewport to the screen and scroll containers, coalesce fit requests and
cancel pending requests on later manual input. Observers refresh availability
without moving the camera.

Both Cytoscape canvases use the [existing-camera adapter](../../web/src/collaboration/graph-camera.ts).
Mesh adds measured HTML label dimensions to Cytoscape's node, edge, label and
compound-group bounds. The SVG canvas measures rendered text, curves, node
shapes, strokes and arrow extents, then maps screen padding into its viewBox.
Each canvas applies zoom and pan together. Its effective manual zoom floor
includes one octave below the required overview and never exceeds the current
zoom; the existing manual maximum remains intact.

The localized Scan button retains visible text on compact screens. Mesh controls
sit above the compact inspector; fit preserves the panel and uses the remaining
area. This can produce a very small overview, as accepted in Q4. Empty filtering
keeps the renderer and camera, disables fit and clears the minimap. Initial
framing and deliberate perspective/layout framing use the same fit contract.

Local verification completed on 2026-10-01:

- `npm run test:graph --prefix web` and
  `npm run test:collaboration --prefix web`: **101 passed**, including nine new
  pure-geometry tests for containment, centering, invalid inputs, small viewports,
  deterministic viewport selection and smooth low-scale manual zoom limits.
- From `web`, `AIDASH_UI_PORT=18196 npx playwright test --config playwright.ui.config.ts mesh-graph.spec.ts agent-graph.spec.ts federated-graph.spec.ts`:
  **25 passed**. Independent DOM/Cytoscape assertions
  check containment and centering within 1 CSS px across all three renderers,
  five mesh perspectives and three layouts. Coverage includes off-screen pan,
  large/single/empty graphs, loaded authorized remote resources, both languages,
  compact inspector, resize, Enter/Space, repeat stability, camera continuity,
  cancellation, unchanged positions/selection/expansion and no fit-triggered fetch.
- TypeScript/Vite build, Trunk ESLint/Prettier and diff whitespace checks passed. Browser
  captures were inspected for the compact Japanese mesh and English SVG canvas.

Mesh geometry fixtures wait for currently used fonts before camera interaction,
so the assertion compares the geometry present at activation. Later font arrival
does not schedule a refit, as agreed in Q10. Browser padding assertions use the
actual usable rectangle's 36/16 CSS px policy, including the tiny-area cap.

Review captures use synthetic authorized fixtures:
[compact Japanese baseline](issue-96-fit/before-mesh-ja-JP.png) rebuilt from the
inspected baseline SHA, [compact Japanese fit](issue-96-fit/after-mesh-ja-JP.png)
and [English agent fit](issue-96-fit/after-agent-en-US.png) from the passing
browser suite. The baseline's compact inspector obscures the old control, so its
existing fit handler was invoked programmatically for that capture.

The normal prebuild API generation was blocked by the host's unaccepted Xcode
license. For the frontend build, existing generated API types and OpenAPI output
were copied from a clean checkout at the exact inspected baseline SHA, then
`npm run build --ignore-scripts --prefix web` passed. No API or backend changes
are part of this feature. Fresh contract generation remains unverified on this
host; no hosted CI or deployment result is claimed.

## Accepted boundary decision

[ADR 0010](../adr/0010-whole-graph-fit-preserves-view-scope.md) records why
whole-graph fit preserves the bounded current graph and prioritizes an overview
over minimum detail readability. Numeric padding and control placement remain
ordinary implementation policy in this design record. Round 2 requires no new
ADR or domain term; the existing glossary and boundary decision remain valid.

## Documentation policy

The glossary records resolved domain terms in [CONTEXT.md](../../CONTEXT.md).
Record interview answers here as they are accepted. Create an ADR only for a
decision with a meaningful reversal cost, a non-obvious rationale and a real
trade-off; ordinary button copy and padding choices belong in this design record.
