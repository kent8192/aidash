import { ReferenceName } from "../record-view";
import type { Discovery, Run, State } from "../types";
import { useI18n } from "../ui";
import { cn } from "../lib/utils";

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
    <div
      className={cn(
        "mesh-canvas flex flex-col gap-2 overflow-x-auto rounded-lg border border-border bg-background",
        compact && "compact",
      )}
    >
      <svg
        className={cn(
          "block h-auto w-full min-w-[520px]",
          compact ? "max-h-[260px]" : "max-h-[600px]",
        )}
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
            <circle cx="1" cy="1" r="1" className="fill-[var(--grid-dot)]" />
          </pattern>
          <marker
            id={arrowId}
            markerWidth="6"
            markerHeight="6"
            refX="5"
            refY="3"
            orient="auto"
          >
            <path d="M0 0 L6 3 L0 6" className="fill-edge-strong" />
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
            className="stroke-edge"
            strokeWidth="1.5"
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
              className="stroke-edge-strong"
              strokeWidth="1.5"
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
                rx={10}
                className={
                  i === 0
                    ? "fill-raised stroke-brand-line"
                    : "fill-surface stroke-border-strong"
                }
              />
              <circle
                cx={i * 300 + 48}
                cy={56}
                r={4}
                className={i === 0 ? "fill-brand-mark" : "fill-faint"}
              />
              <text
                x={i * 300 + 64}
                y={54}
                className="fill-foreground font-mono text-[12px] font-medium"
              >
                <ReferenceName id={node} />
              </text>
              <text
                x={i * 300 + 64}
                y={74}
                className="fill-faint text-[11px]"
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
                      className="stroke-border-strong"
                      strokeWidth="1"
                    />
                    <rect
                      x={i * 300 + 78}
                      y={104 + j * 64}
                      width={190}
                      height={46}
                      rx={6}
                      className="fill-surface stroke-border-strong"
                    />
                    <circle
                      cx={i * 300 + 93}
                      cy={127 + j * 64}
                      r={4}
                      className={run ? "fill-success" : "fill-faint"}
                    />
                    <text
                      x={i * 300 + 106}
                      y={123 + j * 64}
                      className="fill-foreground text-[12px] font-medium"
                    >
                      {local(a.entity.name).slice(0, 22)}
                    </text>
                    <text
                      x={i * 300 + 106}
                      y={138 + j * 64}
                      className="fill-faint font-mono text-[10px]"
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
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border px-3 py-2 text-[11px] text-faint">
        <span className="inline-flex items-center gap-1.5">
          <i aria-hidden className="w-4 border-t border-edge-strong" />
          {t("taskCommunication")} ·{" "}
          <span className="font-mono tabular">{communications.length}</span>
        </span>
        <span className="inline-flex items-center gap-1.5">
          <i
            aria-hidden
            className="size-2.5 rounded-sm border border-brand-line bg-raised"
          />
          {t("node")}
        </span>
        <span className="inline-flex items-center gap-1.5">
          <i
            aria-hidden
            className="size-2.5 rounded-sm border border-border-strong bg-surface"
          />
          {t("agent")}
        </span>
        <span>
          <span className="font-mono tabular">
            {
              data.tasks.filter(
                (t) => t.owner && !t.owner.startsWith(data.node.id + "/"),
              ).length
            }
          </span>{" "}
          · {t("remoteTasks")}
        </span>
      </div>
    </div>
  );
}
