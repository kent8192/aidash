import { lazy, Suspense, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  CheckCircle2,
  Circle,
  Network,
  Pause,
  Play,
  ShieldCheck,
  Target,
  Users,
  X,
} from "lucide-react";
import { runControl, remoteAction } from "../generated/aidash";
import type { Task, Workspace, State, Discovery } from "../types";
import type { Selection } from "./details";
import { Badge, useI18n, useAgentLabel } from "../ui";
import { taskProgress } from "./model";
import { workspaceCopy } from "./workspace-copy";
import { Avatar } from "./avatar";

import { channelAgents, terminalRun, type LocatedRun } from "./workspace-model";
const MiniTopology = lazy(() => import("./mini-topology"));

export function ChannelStatus({
  workspace,
  data,
  tasks,
  runs,
  waiting,
  discovery,
  open,
  graph,
  showTasks,
  expanded,
  close,
}: {
  workspace: Workspace;
  data: State;
  tasks: Task[];
  runs: LocatedRun[];
  waiting: number;
  discovery?: Discovery;
  open: (value: Selection) => void;
  graph: () => void;
  showTasks: () => void;
  expanded: boolean;
  close: () => void;
}) {
  const { locale } = useI18n();
  const words = workspaceCopy[locale];
  const agentLabel = useAgentLabel(data, discovery);
  const label = (item: LocatedRun) =>
    agentLabel(item.node, {
      id: item.run.agent_id,
      version: item.run.agent_version,
    });
  const agents = channelAgents(runs);
  const progress = taskProgress(tasks, workspace.id);
  const active = runs.filter((item) => !terminalRun(item.run));
  const paused =
    active.length > 0 && active.every((item) => item.run.control === "PAUSED");
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState("");
  const client = useQueryClient();
  async function control() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError("");
    let failed = false;
    for (const { run, node } of active) {
      if (!paused && run.control === "PAUSED") continue;
      try {
        const action = paused ? "resume" : "pause";
        if (node === data.node.id) await runControl(run.id, { action });
        else
          await remoteAction({
            node_id: node,
            control: { run_id: run.id, action },
          });
      } catch {
        failed = true;
      }
    }
    await Promise.all([
      client.invalidateQueries({ queryKey: ["state"] }),
      client.invalidateQueries({ queryKey: ["mesh"] }),
      client.invalidateQueries({ queryKey: ["run"] }),
    ]);
    if (failed) setError(words.partialFailure);
    inFlight.current = false;
    setBusy(false);
  }
  return (
    <aside
      className={`workspace-status ${expanded ? "is-open" : ""}`}
      aria-label={words.status}
    >
      <header>
        <span>
          <ShieldCheck size={13} />
          {words.status}
        </span>
        <button
          type="button"
          className="workspace-status-close"
          aria-label={words.close}
          onClick={close}
        >
          <X size={16} />
        </button>
      </header>
      <div className="workspace-status-content">
        <section className="workspace-goal-summary">
          <h3>
            <Target size={13} />
            {locale === "ja-JP" ? "ゴール" : "Goal"}
          </h3>
          <h2>{workspace.title}</h2>
          <p>{workspace.goal}</p>
          <div className="workspace-progress-caption">
            <span>
              <strong>{progress.completed}</strong> / {progress.total}{" "}
              {words.taskCounts}
            </span>
            <small>
              {waiting} {words.waiting}
            </small>
          </div>
          <progress
            value={progress.completed}
            max={Math.max(progress.total, 1)}
            aria-label={words.taskCounts}
          />
        </section>
        <section>
          <div className="workspace-section-heading">
            <h3>
              <CheckCircle2 size={13} />
              {words.currentTasks}
            </h3>
            <button type="button" onClick={showTasks}>
              {words.showAll}
            </button>
          </div>
          {tasks.length === 0 && (
            <p className="muted">
              {locale === "ja-JP"
                ? "まだタスクはありません。"
                : "No tasks yet."}
            </p>
          )}
          {[...tasks]
            .sort(
              (a, b) =>
                Number(a.status === "COMPLETED") -
                Number(b.status === "COMPLETED"),
            )
            .slice(0, 3)
            .map((task) => (
              <button
                className="workspace-task-row"
                key={task.id}
                type="button"
                onClick={() => open({ kind: "taskDetail", task })}
              >
                {task.status === "COMPLETED" ? (
                  <CheckCircle2 size={13} />
                ) : (
                  <Circle size={13} />
                )}
                <span>{task.title}</span>
                <Badge value={task.status} />
              </button>
            ))}
          {tasks.length > 3 && (
            <button
              className="workspace-more-tasks"
              type="button"
              onClick={showTasks}
            >
              + {tasks.length - 3} {words.task}
            </button>
          )}
        </section>
        <section>
          <div className="workspace-section-heading">
            <h3>
              <Users size={13} />
              {words.participants}
              <span className="workspace-count">{agents.length}</span>
            </h3>
            <button
              type="button"
              onClick={() => open({ kind: "task", workspace })}
            >
              {words.manage}
            </button>
          </div>
          {agents.length === 0 && (
            <p className="muted">{words.noParticipants}</p>
          )}
          {agents.map((item) => (
            <button
              className="workspace-participant"
              type="button"
              key={`${item.node}:${item.run.agent_id}:${item.run.agent_version}`}
              onClick={() => open({ kind: "run", ...item })}
            >
              <Avatar name={label(item)} />
              <span>
                <strong>{label(item)}</strong>
                <small>
                  {item.node === data.node.id ? words.local : words.peer}
                </small>
              </span>
              <Badge
                value={
                  item.run.control === "PAUSED" ? "PAUSED" : item.run.phase
                }
              />
            </button>
          ))}
        </section>
        <section>
          <div className="workspace-section-heading">
            <h3>
              <Network size={13} />
              {words.connections}
            </h3>
            <button type="button" onClick={graph}>
              Graph View
            </button>
          </div>
          <div className="workspace-mini-map">
            <Suspense fallback={<div className="workspace-mini-graph" />}>
              {" "}
              <MiniTopology
                runs={runs}
                tasks={tasks}
                label={label}
                title={words.snapshot}
              />
            </Suspense>
            <div>
              <span>
                {new Set(runs.map((item) => item.node)).size} {words.nodes}
              </span>
              <small>{words.snapshot}</small>
            </div>
          </div>
        </section>
      </div>
      <div className="workspace-status-bottom">
        {error && (
          <p role="alert" className="error">
            {error}
          </p>
        )}
        <button
          type="button"
          disabled={busy || active.length === 0}
          onClick={() => void control()}
        >
          {paused ? <Play size={13} /> : <Pause size={13} />}
          {paused ? words.resume : words.pause}
        </button>
      </div>
    </aside>
  );
}
