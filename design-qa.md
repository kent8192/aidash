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


---

# Workbench layout verification

final result: passed

Scope: adapt the supplied Creator and Trust compositions to Aidash's existing dashboard palette and functionality. This is an existing-product implementation, not a static recreation of the example agents or their certification claims.

## Visual evidence

- Creator source: `/tmp/codex-remote-attachments/01a0e7bd-72a5-7473-a388-3df8019abe96/25E74D58-39D2-409D-9CF7-68B3553AB193/1-写真1.jpg`
- Trust source: `/tmp/codex-remote-attachments/01a0e7bd-72a5-7473-a388-3df8019abe96/25E74D58-39D2-409D-9CF7-68B3553AB193/2-写真2.jpg`
- Creator implementation: `docs/screenshots/workbenches/creator-desktop.png`
- Trust implementation: `docs/screenshots/workbenches/trust-desktop.png`
- Mobile captures: `/tmp/workbench-preview/evidence/creator-mobile.png` and `/tmp/workbench-preview/evidence/trust-mobile.png`
- Source images: 1280 × 960 pixels. Desktop CSS viewport and screenshots: 1280 × 960, device scale factor 1. Compared without density rescaling. Mobile DOM viewport: 390 × 844. The in-app screenshot renderer scales mobile captures within the desktop surface, so mobile layout checks also use measured DOM widths.
- State: selected sample agent, Creator overview with a completed simulated conversation; Trust overview with a checked permission context. Only the temporary preview server supplies these sample records. Production UI reads the existing endpoints.
- Preview: `http://127.0.0.1:18473/creator?focus=managed-agent%401.0.0` and `/trust?focus=managed-agent%401.0.0` on the same origin.

## Comparison and iteration

Full-view source and implementation screenshots were reviewed together. Creator preserves the editor / conversation / verification hierarchy; the editor contains profile and instructions above paired capability/model and tools/memory cards. Trust uses five overview metrics, a three-column detail grid, a wider permission matrix, and a summary/evidence/workspace sidebar.

Focused checks covered profile field alignment, conversation bubbles, compact status rows, permission matrix, and sidebar text wrapping. The screenshots are readable at the captured desktop density; no separate cropped image was needed.

1. P1: Creator initially accessed an unhydrated draft while resolving its model label. Added a nullable selected-model lookup; initial load and reload now render normally.
2. P2: global `dl` styles forced sidebar facts into narrow columns. Scoped workbench facts to block layout and removed inherited padding. Verified readable horizontal label/value rows in the final screenshots.
3. P2: Trust sidebar cards stretched to the main grid height. Aligned the sidebar to the start and compacted overview metrics. Final Trust capture shows natural-height cards.
4. P2: Creator profile was unnecessarily tall. Moved description beside tags/icon fields and kept the card responsive. Final Creator screenshot includes the corrected field composition.
5. Preview-only setup issue: the temporary inspection endpoint initially returned the wrong fixture shape. Corrected the preview route. No application errors were recorded after the fix during the final verification period.

## Required fidelity surfaces

- Typography: existing DM Sans / Noto Sans JP stack; compact headings and form text, with wrapping for long IDs and names.
- Spacing/layout: 12px card gaps, compact controls, three primary Creator columns and three Trust detail columns at desktop width. DOM checks found no horizontal page overflow at 1280px or 390px. Mobile uses a single column and expandable Creator details.
- Colors/tokens: white cards, sage page/control surfaces, forest-green actions and existing dashboard green/border tokens. The supplied dark/violet palette was intentionally replaced as requested.
- Assets: existing Aidash brand and installed Lucide icons. The existing navigation is preserved. Example avatars, leaf logos, and promotional artwork are not substituted for actual agent identities or added to the shared navigation.
- Content: preserve Register terminology, technical-validation status, requested-versus-effective permissions, and external-assessment deferral. Never populate example scores, approvals, or certifications without supporting data. The existing draft selector and sandbox configuration remain available; these add vertical space relative to the reference, and lower cards remain reachable by scrolling.

## Interaction verification

Verified in the in-app browser against the isolated sample backend:

- Editing an agent name, saving, and seeing the saved revision.
- Running technical validation and seeing its result.
- Sending a sandbox message and rendering the sample reply. The overview now stays open during this interaction.
- Opening Trust policies, checking a context, returning to overview, and seeing the permission matrix reflect that result.
- Opening Creator details at 390px and verifying the content is visible without horizontal overflow.
- Final console audit: no new error entries after the preview fixes.

Static verification: TypeScript build, ESLint for the changed component, Vite production build, and `git diff --check`. Existing large-chunk build warnings remain. Live authentication, real inference, registration, and backend permission behavior were not changed or revalidated in this layout task.

## Follow-up polish

P3: at the same viewport, the real product's selector, sandbox controls, and longer explanatory copy require more scrolling than the fictional reference. A separate simplification of those controls could further increase density.

## Implementation checklist

- [x] Preserve existing API operations and form controls.
- [x] Match the main information regions to the supplied references.
- [x] Use the existing product palette and typography.
- [x] Check desktop/mobile layouts and key interactions.
- [x] Keep this task isolated from the concurrently edited worktree.
