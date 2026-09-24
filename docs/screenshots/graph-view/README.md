# Graph View screenshots

Captured on 2026-09-24 from the production frontend build using the synthetic
records in `web/tests/mesh-scene.mjs`. These images contain no production data.
The supplied Aidash logo kit is used without modifying its original paths.

| Image                                   | State                                | CSS viewport |
| --------------------------------------- | ------------------------------------ | ------------ |
| [Agent mesh](mesh-mesh.png)             | Planner Agent selected               | 1280 × 800   |
| [Collaboration](mesh-collaboration.png) | Product Lab selected                 | 1280 × 800   |
| [Knowledge](mesh-knowledge.png)         | PRD selected                         | 1280 × 800   |
| [Execution](mesh-execution.png)         | Task selected from the timeline      | 1280 × 800   |
| [Topology](mesh-topology.png)           | Local Agent Cluster selected         | 1280 × 800   |
| [Mobile](mesh-390.png)                  | Relationship list and task inspector | 390 × 844    |
| [Compact](mesh-900.png)                 | Relationship list and task inspector | 900 × 600    |

All captures use device scale factor 1 and English. The desktop views use the
structured layout. The default graph opens with the mesh perspective; existing
agent-focus URLs still open the preserved Agent relationships perspective.

To regenerate the ignored originals in `web/test-results`, build the frontend
and run `npm run test:ui -- mesh-graph.spec.ts` from `web`. Copy reviewed captures
here only when the expected interface changes. See [design QA](../../../design-qa.md)
for reference-image comparisons and validation boundaries.
