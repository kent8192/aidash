# Graph View design QA

final result: passed

Reviewed on 2026-09-24 in branch `feat/graph-mesh-layout`, based on
`develop/0.1.0` at `10fb917`.
Local production-build preview: <http://127.0.0.1:18144/graph>.
The preview uses synthetic test records, not a production cluster.

## Findings and comparison history

No actionable P0/P1/P2 findings remain in the reviewed states.

| Earlier finding                                                                                  | Severity | Fix and post-fix evidence                                                                                                                                                                                                                                                                     |
| ------------------------------------------------------------------------------------------------ | -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Agent, tool and artifact groups overlapped; some node labels collided.                           | P1       | Separate compound bounds and stable semantic positions in `mesh-model.ts`; the revised mesh, knowledge and topology captures show distinct groups.                                                                                                                                            |
| Execution cards were cramped and perspective changes could leave lower nodes outside the canvas. | P1       | Larger task/agent cards, ordered execution lanes, and `cy.resize()` before fitting the new perspective. The execution capture shows the full flow and timeline. Browser tests assert every node label is inside the stage immediately after switching each perspective, without a manual fit. |
| Captured HTML node labels lagged behind the graph camera.                                        | P2       | Labels follow the camera through animation-frame projection; screenshot assertions wait for that projection. Revised captures have aligned labels and nodes.                                                                                                                                  |
| The expanded legend covered the tool group.                                                      | P2       | Perspective-specific filters omit contextual types from the mesh legend; less common relationships remain in an expandable section. The revised mesh capture leaves tools visible.                                                                                                            |
| Compact layouts inherited the previous reversed grid order and clipped controls.                 | P2       | Explicit responsive placement, compact rail, collapsible filters and a bottom-sheet inspector. Revised 390 × 844 and 900 × 600 captures have reachable controls and no document overflow.                                                                                                     |
| Relationship rows inherited a light hover background.                                            | P2       | Scoped dark table hover/focus colors. Revised responsive list captures retain the graph palette and readable text.                                                                                                                                                                            |
| Japanese Marketplace navigation wrapped its final character.                                     | P2       | Adjusted sidebar padding/gap and kept navigation labels on one line. Final Japanese browser observation at 1280 × 800 confirms each item fits its 153 px width without overflow.                                                                                                              |

The above issues were found during earlier source/render comparisons, corrected,
and followed by revised captures and another comparison. Test/build execution
alone was not treated as visual evidence.

## Visual evidence

Source directory A:
`/tmp/codex-remote-attachments/01a0d18b-278d-70a3-ba51-9e1a07df16a6/312DB4BE-8068-49BA-AB19-18E34E20B21F/`

Source directory B:
`/tmp/codex-remote-attachments/01a0d18b-278d-70a3-ba51-9e1a07df16a6/4379B470-1DF6-4252-AAFF-281AF951D0D6/`

| Source visual truth | Implementation screenshot                 | Selected inspector  |
| ------------------- | ----------------------------------------- | ------------------- |
| A / `1-写真1.jpg`   | `web/test-results/mesh-mesh.png`          | Planner Agent       |
| B / `1-写真1.jpg`   | `web/test-results/mesh-collaboration.png` | Product Lab         |
| B / `2-写真2.jpg`   | `web/test-results/mesh-knowledge.png`     | PRD                 |
| B / `3-写真3.jpg`   | `web/test-results/mesh-execution.png`     | Research & Plan     |
| B / `4-写真4.jpg`   | `web/test-results/mesh-topology.png`      | Local Agent Cluster |

Each source and implementation was opened together in the same comparison input.
All five full views use a 1280 × 800 CSS viewport and 1280 × 800 image pixels at
device scale factor 1. No density scaling or browser chrome was included.
The implementation state is an authorized operator in Product Lab, English,
dark graph theme, structured layout, default type/relation filters and a 24-hour
activity window. Records differ from the design examples: this is a comparison
of layout and interaction hierarchy, not a pixel-diff of identical database data.

Focused inspector comparison was also opened together at native resolution:

- Source: `/tmp/aidash-source-inspector.jpg`.
- Implementation: `/tmp/aidash-rendered-inspector.png`.
- Both crops are 272 × 730 pixels from the same desktop frames, covering header,
  tabs, metadata, capabilities, activity and connected tasks.
- This confirms readable small text, aligned metadata, consistent tab hierarchy
  and colored status treatment. The source's estimated performance metrics are
  replaced by counts of recorded events because those metrics are not supplied.

Additional browser captures: `web/test-results/mesh-390.png` at 390 × 844 and
`web/test-results/mesh-900.png` at 900 × 600, both at scale factor 1. They exercise
the relationship list and an open task inspector. There is no supplied mobile
reference; these states were checked for usability and overflow rather than
claimed as exact reproductions. Reviewed copies of all seven captures are
committed in [`docs/screenshots/graph-view`](docs/screenshots/graph-view/README.md)
for pull-request review; the test output originals remain ignored.

## Required fidelity surfaces

- **Fonts and typography:** Retains the application's DM Sans / Noto Sans JP /
  sans-serif stack. The reference uses a compact sans-serif hierarchy; exact font
  metadata was not supplied. Titles, metadata, tags and graph labels preserve that
  hierarchy. English and Japanese wrapping were checked, including the final
  sidebar fix. Wordmark lettering uses the supplied outlined SVG paths.
- **Spacing and layout rhythm:** The desktop shell uses a 172 px left rail,
  flexible graph and approximately 272 px inspector, matching the reference's
  three major regions. Group frames, floating filters, minimap, camera controls,
  tabs and section spacing were compared in full views and inspector crops.
  Execution has its own card lanes, counters and bottom timeline.
- **Colors and tokens:** Near-black green canvas (`#041c17`), darker panel
  surfaces (`#09241e`), subtle green borders (`#24443a`), pale text (`#e0eee8`)
  and muted text (`#92aca2`) preserve the reference palette. Cyan tasks, purple
  agents, green goals and amber artifacts distinguish node kinds. Selection,
  focus, status chips and table hover remain visible on the dark surfaces.
- **Image quality and asset fidelity:** The user's `aidash-logo-kit.zip` supplies
  the unchanged light/dark Mesh + Accent wordmarks, compact SVG mark and ICO
  favicon in `web/public/brand`. Original archive bytes and built copies were
  verified. SVGs preserve aspect ratio and sharpness. Existing Lucide symbols
  are used consistently for graph node types and controls. The current API does
  not provide profile photographs, so the UI uses human-type icons; it does not
  invent avatars or convert reference portraits into user data.
- **Copy and content:** All graph controls and descriptive UI have English and
  Japanese text. Names, task states, configuration links and event counts come
  from authorized snapshots. Configured/discovered nodes are labeled by that
  evidence. Unsupported health, CPU, latency, approval or success metrics are
  absent. The app contains no design-reference or implementation instructions.

## Functional verification

- Graph unit tests: 29 passed, including 10 new mesh projection tests.
- Collaboration unit tests: 32 passed. Total unit tests: 61 passed.
- Complete mock UI suite: 67 passed after updating to the current development
  base and integrating the logo kit. This includes all seven mesh tests and
  the upstream exact-focus navigation/reload/history regression test.
- TypeScript and Vite production build passed. Scoped ESLint passed for changed
  source/tests. The last navigation-only CSS adjustment was rebuilt and manually
  verified in the Japanese production preview.
- Tested five perspectives, structured/force/circle layouts, search, node and
  relationship filters, neighborhood focus/reset, table view, detail/task
  navigation, timeline selection, zoom/fit and camera preservation on refresh.
  Also checked revoked data removal, subject isolation and both compact sizes.
  Fullscreen entry/exit was inspected in the local browser.
- The upstream focus-URL test initially found that an agent-focus link opened
  the default mesh instead of the preserved agent-focused perspective. The
  initial mode now respects explicit focus URLs; the unchanged upstream test
  passes for selection, links, reload, browser history and unavailable agents.
- Scoped Trunk Prettier/ESLint checks passed without issues.
- Native buttons, labeled inputs, focus styles and an accessible relationship
  table provide a keyboard alternative to the canvas.
- The final production-preview reload had no captured console errors or warnings.
  Both logo variants loaded successfully; document width equaled the 1280 px
  viewport with no horizontal overflow.

Reproduction commands from `web`:

```sh
npm run test:graph
npm run test:collaboration
npm exec -- tsc -b
npm exec -- vite build
AIDASH_UI_PORT=18146 npm run test:ui
```

The normal build first regenerates the API client through `prebuild`. This
frontend-only work used the existing same-base OpenAPI schema and regenerated
the client; no backend/schema changes were needed. Browser tests intercept API
responses. Real-cluster integration and remote CI were not part of this run.

## Accepted differences and follow-up

The five supplied images are alternative compositions, implemented as switchable
perspectives rather than a single static scene. Exact node placement follows
actual records and authorization; the renderer does not insert example nodes.
The legacy Agent relationships view and existing entity dialogs remain available.
Very dense snapshots can still require filtering, zooming or the relationship
table. The visible 180-node/600-edge limit includes omission counts.

No open design questions block this implementation. Future telemetry or document
history can populate additional inspector sections when authorized API data is
available. These are future data capabilities, not fabricated indicators.

## Implementation checklist

- [x] Create a dedicated branch and worktree; preserve the invoking checkout.
- [x] Implement all five compositions with real projection and interactive controls.
- [x] Reuse the provided logo assets unchanged and preserve existing navigation.
- [x] Compare source/rendered desktop views and focused inspector regions.
- [x] Correct visual findings and recheck responsive and Japanese states.
- [x] Complete relevant unit/UI checks, lint and production build.
- [x] Leave a local sample-data preview and record validation boundaries.
