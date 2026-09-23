import { ReferenceName } from "../record-view";
import type { Discovery, Run, State } from "../types";
import { useI18n } from "../ui";

export function MeshView({
  data,
  discovery,
  compact = false,
  runs,
}: {
  data: State;
  runs: { run: Run; node: string }[];
  discovery?: Discovery;
  compact?: boolean;
}) {
  const { t, local } = useI18n();
  const agents =
    discovery?.agents ??
    data.registry
      .filter((e) => e.kind === "agent")
      .map((entity) => ({ entity, node_id: data.node.id }));
  const nodeIds = [
    data.node.id,
    ...data.peers.filter((p) => p.enabled).map((p) => p.node_id),
  ];
  const width = Math.max(600, nodeIds.length * 300);
  const height = compact
    ? 260
    : Math.max(
        310,
        ...nodeIds.map(
          (n) => 150 + agents.filter((a) => a.node_id === n).length * 64,
        ),
      );
  const shown = compact
    ? agents.filter(
        (a, i, all) =>
          all.filter((b) => b.node_id === a.node_id).indexOf(a) < 2,
      )
    : agents;
  const positions = new Map(
    shown.map((a) => {
      const i = nodeIds.indexOf(a.node_id);
      const j = shown.filter((b) => b.node_id === a.node_id).indexOf(a);
      return [
        `${a.node_id}/agents/${a.entity.id}@${a.entity.version}`,
        { x: i * 300 + 78, y: 127 + j * 64 },
      ];
    }),
  );
  const communications = data.tasks.filter(
    (task) =>
      task.owner &&
      task.created_by !== task.owner &&
      positions.has(task.created_by) &&
      positions.has(task.owner),
  );
  const arrowId = compact ? "communication-small" : "communication";
  return (
    <div className={`mesh-canvas ${compact ? "compact" : ""}`}>
      <svg
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={t("topology")}
      >
        <defs>
          <pattern
            id={compact ? "dots-small" : "dots"}
            width="18"
            height="18"
            patternUnits="userSpaceOnUse"
          >
            <circle cx="1" cy="1" r="1" fill="#dae2dd" />
          </pattern>
          <marker
            id={arrowId}
            markerWidth="6"
            markerHeight="6"
            refX="5"
            refY="3"
            orient="auto"
          >
            <path d="M0 0 L6 3 L0 6" fill="#7ba898" />
          </marker>
        </defs>
        <rect
          width={width}
          height={height}
          fill={`url(#${compact ? "dots-small" : "dots"})`}
        />
        {nodeIds.length > 1 && (
          <path
            d={`M150 58 H${(nodeIds.length - 1) * 300 + 150}`}
            stroke="#85a89b"
            strokeWidth="2"
            strokeDasharray="5 5"
          />
        )}
        {communications.map((task) => {
          const from = positions.get(task.created_by)!;
          const to = positions.get(task.owner!)!;
          const sameNode = from.x === to.x;
          const path = sameNode
            ? `M${from.x} ${from.y} C${from.x - 48} ${from.y},${to.x - 48} ${to.y},${to.x - 4} ${to.y}`
            : `M${from.x + 190} ${from.y} C${from.x + 235} ${from.y},${to.x - 42} ${to.y},${to.x - 4} ${to.y}`;
          return (
            <path
              key={task.id}
              d={path}
              fill="none"
              stroke="#468a70"
              strokeWidth="1.8"
              markerEnd={`url(#${arrowId})`}
            >
              <title>{`${task.title}: ${task.created_by} → ${task.owner} (${t(task.status)})`}</title>
            </path>
          );
        })}
        {nodeIds.map((node, i) => {
          const nodeAgents = shown.filter((a) => a.node_id === node);
          return (
            <g key={node}>
              <rect
                x={i * 300 + 25}
                y={25}
                width={250}
                height={65}
                rx={12}
                fill={i === 0 ? "#184e41" : "#ecf1ed"}
                stroke="#bfd0c5"
              />
              <circle
                cx={i * 300 + 48}
                cy={56}
                r={5}
                fill={i === 0 ? "#bce6bd" : "#619278"}
              />
              <text
                x={i * 300 + 64}
                y={54}
                fill={i === 0 ? "#fff" : "#285240"}
                fontSize="13"
                fontWeight="600"
              >
                <ReferenceName id={node} />
              </text>
              <text
                x={i * 300 + 64}
                y={74}
                fill={i === 0 ? "#b6d1c2" : "#76867d"}
                fontSize="10"
              >
                {nodeAgents.length} {t("agents")}
              </text>
              {nodeAgents.map((a, j) => {
                const cluster = (
                  a.entity.config.cluster as { id?: string } | null
                )?.id;
                const run = runs.find(
                  ({ run: r, node }) =>
                    node === a.node_id &&
                    r.agent_id === a.entity.id &&
                    r.agent_version === a.entity.version &&
                    !["COMPLETED", "FAILED", "CANCELLED"].includes(r.phase),
                );
                return (
                  <g key={`${a.entity.id}@${a.entity.version}`}>
                    <path
                      d={`M${i * 300 + 55} ${j === 0 ? 90 : 125 + (j - 1) * 64} V${125 + j * 64} H${i * 300 + 78}`}
                      fill="none"
                      stroke="#a7bdb1"
                      strokeWidth="1.5"
                    />
                    <rect
                      x={i * 300 + 78}
                      y={104 + j * 64}
                      width={190}
                      height={46}
                      rx={8}
                      fill="white"
                      stroke="#d8e2db"
                    />
                    <circle
                      cx={i * 300 + 93}
                      cy={127 + j * 64}
                      r={4}
                      fill={run ? "#46a374" : "#b9c5bd"}
                    />
                    <text
                      x={i * 300 + 106}
                      y={123 + j * 64}
                      fill="#244438"
                      fontSize="11"
                      fontWeight="600"
                    >
                      {local(a.entity.name).slice(0, 22)}
                    </text>
                    <text
                      x={i * 300 + 106}
                      y={138 + j * 64}
                      fill="#74867a"
                      fontSize="9"
                    >
                      {cluster ? (
                        <ReferenceName id={cluster} />
                      ) : (
                        a.entity.languages.join(" / ")
                      )}
                    </text>
                  </g>
                );
              })}
            </g>
          );
        })}
      </svg>
      <div className="mesh-legend">
        <span>
          {t("taskCommunication")} · {communications.length}
        </span>
        <span>
          <i className="legend-node" />
          {t("node")}
        </span>
        <span>
          <i className="legend-agent" />
          {t("agent")}
        </span>
        <span className="muted">
          {
            data.tasks.filter(
              (t) => t.owner && !t.owner.startsWith(data.node.id + "/"),
            ).length
          }{" "}
          · {t("remoteTasks")}
        </span>
      </div>
    </div>
  );
}
