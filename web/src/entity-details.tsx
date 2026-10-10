import { Badge as Tag } from "./components/ui/badge";
import { Hint } from "./components/patterns";
import { ArrowUpRight } from "lucide-react";
import type { Entry, Run, State, Task } from "./types";
import { Badge, useI18n } from "./ui";
import { RecordView } from "./record-view";
import { AgentRelationshipGraph } from "./agent-graph";
import { graphCopy } from "./agent-graph/copy";
import type { GraphNode } from "./agent-graph/model";

type Selection =
  | { kind: "entityDetail"; entity: Entry }
  | { kind: "run"; run: Run; node: string }
  | { kind: "taskDetail"; task: Task };
export function EntityDetails({
  entity,
  data,
  open,
}: {
  entity: Entry;
  data: State;
  open: (selection: Selection) => void;
}) {
  const { local, t, locale, entityName } = useI18n();
  // Re-resolve on every authorized snapshot instead of retaining the modal's old metadata.
  const current = data.registry.find(
    (value) =>
      value.id === entity.id &&
      value.version === entity.version &&
      value.kind === entity.kind,
  );
  if (!current) return <p role="status">{graphCopy[locale].unavailable}</p>;
  const openNode = (node: GraphNode) => {
    if (!node.available) return;
    if (node.entity) {
      const entry = data.registry.find(
        (value) =>
          value.kind === node.kind &&
          value.id === node.entity?.id &&
          value.version === node.entity?.version,
      );
      if (entry) open({ kind: "entityDetail", entity: entry });
    } else if (node.kind === "run") {
      const run = data.runs.find((value) => value.id === node.resourceId);
      if (run) open({ kind: "run", run, node: data.node.id });
    } else if (node.kind === "task") {
      const task = data.tasks.find((value) => value.id === node.resourceId);
      if (task) open({ kind: "taskDetail", task });
    }
  };
  const runs =
    current.kind === "agent"
      ? data.runs.filter(
          (run) =>
            run.agent_id === current.id &&
            run.agent_version === current.version,
        )
      : [];
  return (
    <div className="grid min-w-0 gap-5">
      <header className="grid min-w-0 gap-2">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <h3 className="min-w-0 text-[15px] font-semibold text-foreground [overflow-wrap:anywhere]">
            {entityName(current)}
          </h3>
          <Badge value={current.kind} />
          <span className="font-mono text-xs text-muted-foreground tabular">
            v{current.version}
          </span>
        </div>
        {local(current.description) && (
          <Hint>{local(current.description)}</Hint>
        )}
        {current.capabilities.length > 0 && (
          <div className="flex min-w-0 flex-wrap gap-1">
            {current.capabilities.map((capability) => (
              <Tag key={capability} tone="neutral" className="font-mono">
                {capability}
              </Tag>
            ))}
          </div>
        )}
      </header>
      {current.kind === "agent" && (
        <AgentRelationshipGraph
          key={JSON.stringify([data.node.id, current.id, current.version])}
          entry={current}
          data={data}
          open={openNode}
        />
      )}
      <section className="grid min-w-0 gap-2 border-t border-border pt-4">
        <h4 className="text-[13px] font-semibold text-foreground">
          {t("execution")}
        </h4>
        {runs.length === 0 ? (
          <Hint>{t("empty")}</Hint>
        ) : (
          <ul className="divide-y divide-border rounded-md border border-border">
            {runs.map((run) => (
              <li key={run.id}>
                <button
                  type="button"
                  className="run-choice flex h-9 w-full min-w-0 cursor-pointer items-center gap-2 px-3 text-left text-[13px] transition-colors duration-150 hover:bg-accent"
                  onClick={() => open({ kind: "run", run, node: data.node.id })}
                >
                  <Badge value={run.phase} />
                  <span className="min-w-0 flex-1 truncate">
                    {data.tasks.find((task) => task.id === run.task_id)
                      ?.title || t("task")}
                  </span>
                  <ArrowUpRight aria-hidden className="size-3.5 text-faint" />
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section className="grid min-w-0 gap-2 border-t border-border pt-4">
        <h4 className="text-[13px] font-semibold text-foreground">
          {t("metadata")}
        </h4>
        <RecordView value={current} />
      </section>
    </div>
  );
}
