import { ArrowUpRight } from "lucide-react";
import type { Entry, Run, State, Task } from "./types";
import { Badge, JsonView, useI18n } from "./ui";
import { AgentRelationshipGraph } from "./agent-graph";
import { graphCopy } from "./agent-graph/copy";
import type { GraphNode } from "./agent-graph/model";

type Selection = { kind: "entityDetail"; entity: Entry } | { kind: "run"; run: Run; node: string } | { kind: "taskDetail"; task: Task };
export function EntityDetails({ entity, data, open }: { entity: Entry; data: State; open: (selection: Selection) => void }) {
  const { local, t, locale } = useI18n();
  // Re-resolve on every authorized snapshot instead of retaining the modal's old metadata.
  const current = data.registry.find((value) => value.id === entity.id && value.version === entity.version && value.kind === entity.kind);
  if (!current) return <p role="status">{graphCopy[locale].unavailable}</p>;
  const openNode = (node: GraphNode) => {
    if (!node.available) return;
    if (node.entity) {
      const entry = data.registry.find((value) => value.kind === node.kind && value.id === node.entity?.id && value.version === node.entity?.version);
      if (entry) open({ kind: "entityDetail", entity: entry });
    } else if (node.kind === "run") {
      const run = data.runs.find((value) => value.id === node.resourceId);
      if (run) open({ kind: "run", run, node: data.node.id });
    } else if (node.kind === "task") {
      const task = data.tasks.find((value) => value.id === node.resourceId);
      if (task) open({ kind: "taskDetail", task });
    }
  };
  return <>
    <h3>{local(current.name)}</h3>
    <p>{local(current.description)}</p>
    <div className="tags">{current.capabilities.map((capability) => <span key={capability}>{capability}</span>)}</div>
    {current.kind === "agent" && <AgentRelationshipGraph key={JSON.stringify([data.node.id, current.id, current.version])} entry={current} data={data} open={openNode} />}
    <h4>{t("execution")}</h4>
    {current.kind === "agent" && data.runs.filter((run) => run.agent_id === current.id && run.agent_version === current.version).map((run) => <button className="run-choice" key={run.id} onClick={() => open({ kind: "run", run, node: data.node.id })}><Badge value={run.phase} />{run.task_id.slice(0, 8)}<ArrowUpRight size={15} /></button>)}
    <h4>{t("metadata")}</h4>
    <JsonView value={current} />
  </>;
}
