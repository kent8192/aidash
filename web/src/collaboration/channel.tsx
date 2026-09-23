import { ReferenceName } from "../record-view";
import { RecordView } from "../record-view";
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { workspaceGet } from "../generated/aidash";
import type {
  Artifact,
  Discovery,
  HumanRequest,
  Run,
  State,
  Task,
  Workspace,
} from "../types";
import { Badge, JsonView, useI18n, useAgentLabel } from "../ui";
import { collaborationCopy } from "./copy";
import { taskProgress } from "./model";
import { entityKey } from "../agent-graph/model";
import type { Selection } from "./details";
import { ChannelConversation } from "./conversation";

export function ArtifactList({ artifacts }: { artifacts: Artifact[] }) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  return (
    <section className="collab-artifacts" aria-label={copy.results}>
      <h3>{copy.results}</h3>
      {artifacts.length === 0 && <p className="muted">{copy.empty}</p>}
      {artifacts.map((artifact) => (
        <details key={artifact.id}>
          <summary>
            <strong>{artifact.name}</strong> <Badge value={artifact.kind} />
          </summary>
          <JsonView value={artifact.content} />
          <small>{artifact.created_by}</small>
        </details>
      ))}
    </section>
  );
}

export function Channel({
  workspace,
  data,
  discovery,
  runs,
  requests,
  open,
  graph,
}: {
  discovery?: Discovery;
  workspace: Workspace;
  data: State;
  runs: { run: Run; node: string }[];
  requests: { request: HumanRequest; node: string }[];
  open: (selection: Selection) => void;
  graph: (focus?: string) => void;
}) {
  const { locale } = useI18n();
  const agentLabel = useAgentLabel(data, discovery);
  const copy = collaborationCopy[locale];
  const [tab, setTab] = useState<"conversation" | "work" | "activity">(
    "conversation",
  );
  const query = useQuery({
    queryKey: ["workspace", workspace.id],
    queryFn: () => workspaceGet(workspace.id),
    refetchInterval: 2000,
    retry: false,
  });
  const tasks: Task[] = query.isError
    ? []
    : (query.data?.tasks ??
      data.tasks.filter((task) => task.workspace_id === workspace.id));
  const scopedRuns = runs.filter(
    ({ run }) =>
      run.workspace_id === workspace.id && run.home_node === data.node.id,
  );
  const scopedRequests = requests.filter(
    ({ request }) =>
      request.workspace_id === workspace.id && request.response === null,
  );
  const progress = taskProgress(tasks, workspace.id);
  const agents = Array.from(
    new Map(
      scopedRuns.map(({ run, node }) => [
        JSON.stringify([node, run.agent_id, run.agent_version]),
        { node, id: run.agent_id, version: run.agent_version },
      ]),
    ).values(),
  );
  return (
    <section className="collab-channel" aria-label={workspace.title}>
      <header className="collab-channel-heading">
        <div>
          <span className="eyebrow">{copy.channels}</span>
          <h1 aria-label={`# ${workspace.title}`}>
            <span aria-hidden="true"># </span>
            {workspace.title}
          </h1>
        </div>
        <button type="button" onClick={() => graph()}>
          {copy.graphLink}
        </button>
      </header>
      <details className="collab-goal" open>
        <summary>{copy.goal}</summary>
        <p>{workspace.goal}</p>
      </details>
      <div className="collab-tabs" role="group" aria-label={workspace.title}>
        {(["conversation", "work", "activity"] as const).map((value) => (
          <button
            type="button"
            key={value}
            aria-pressed={tab === value}
            onClick={() => setTab(value)}
          >
            {copy[value]}
          </button>
        ))}
      </div>
      {query.isError ? (
        <div className="error" role="alert">
          <p>{copy.unavailable}</p>
          <p>{query.error.message}</p>
          <button type="button" onClick={() => void query.refetch()}>
            {copy.retry}
          </button>
        </div>
      ) : (
        <>
          {scopedRequests.length > 0 && (
            <section className="collab-attention" aria-label={copy.needsInput}>
              <h3>{copy.needsInput}</h3>
              {scopedRequests.map((item) => (
                <button
                  key={`${item.node}:${item.request.id}`}
                  type="button"
                  className="request-card"
                  onClick={() => open({ kind: "human", ...item })}
                >
                  <Badge value={item.request.kind} />
                  <p>{item.request.prompt}</p>
                  <span>{copy.answer}</span>
                </button>
              ))}
            </section>
          )}
          <ChannelConversation
            key={workspace.id}
            workspace={workspace.id}
            data={data}
            discovery={discovery}
            visible={tab === "conversation"}
          />
          {tab === "work" && (
            <>
              <div className="collab-progress" aria-label={copy.taskProgress}>
                <span>
                  {copy.completed}: {progress.completed} / {progress.total}
                </span>
                <span>
                  {copy.active}: {progress.active}
                </span>
                <span>
                  {copy.attention}: {progress.attention}
                </span>
              </div>
              <button
                type="button"
                onClick={() => open({ kind: "task", workspace })}
              >
                {copy.newTask}
              </button>
              {tasks.length === 0 && (
                <p className="collab-empty">{copy.noTasks}</p>
              )}
              {tasks.map((task) => (
                <button
                  type="button"
                  className="collab-task"
                  key={task.id}
                  onClick={() => open({ kind: "taskDetail", task })}
                >
                  <Badge value={task.status} />
                  <span>
                    <strong>{task.title}</strong>
                    <small>{task.description}</small>
                  </span>
                </button>
              ))}
              <ArtifactList artifacts={query.data?.artifacts ?? []} />
              <h3>{copy.participants}</h3>
              {agents.map((agent) => {
                const label = (
                  <>
                    {agentLabel(agent.node, agent)}
                    <small>
                      <ReferenceName id={agent.node} />
                    </small>
                  </>
                );
                return agent.node === data.node.id ? (
                  <button
                    className="collab-agent"
                    type="button"
                    key={JSON.stringify(agent)}
                    onClick={() => graph(entityKey(agent.node, "agent", agent))}
                  >
                    {label}
                  </button>
                ) : (
                  <div className="collab-agent" key={JSON.stringify(agent)}>
                    {label}
                  </div>
                );
              })}
            </>
          )}
          {tab === "activity" && (
            <>
              <p className="muted">{copy.limitedHistory}</p>
              {scopedRuns.map((item) => (
                <button
                  type="button"
                  className="collab-task"
                  key={`${item.node}:${item.run.id}`}
                  onClick={() => open({ kind: "run", ...item })}
                >
                  <Badge
                    value={
                      item.run.control === "PAUSED" ? "PAUSED" : item.run.phase
                    }
                  />
                  <span>
                    {agentLabel(item.node, {
                      id: item.run.agent_id,
                      version: item.run.agent_version,
                    })}
                    <small>
                      <ReferenceName id={item.node} />
                    </small>
                  </span>
                </button>
              ))}
              {[...(query.data?.events ?? [])].reverse().map((event) => (
                <details className="call-detail" key={event.id}>
                  <summary>
                    {event.kind} ·{" "}
                    {new Date(event.created_at).toLocaleString(locale)}
                  </summary>
                  <RecordView value={event.data} />
                </details>
              ))}
            </>
          )}
        </>
      )}
    </section>
  );
}
