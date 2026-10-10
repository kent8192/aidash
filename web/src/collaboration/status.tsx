import { Button } from "../components/ui/button";
import { useRef, useState } from "react";
import { Alert } from "../components/patterns";
import { useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, Circle, Pause, Play } from "lucide-react";
import { runControl, remoteAction } from "../generated/aidash";
import type { Task, Workspace, State, Discovery } from "../types";
import type { Selection } from "./details";
import { Badge, useI18n, useAgentLabel } from "../ui";
import { taskProgress } from "./model";
import { workspaceCopy } from "./workspace-copy";
import { Avatar } from "./avatar";
import { SegmentedProgress } from "./request/progress";
import { runDisplayStatus } from "./request/canvas-model";
import {
  channelAgents,
  terminalRun,
  resumableRun,
  type LocatedRun,
} from "./workspace-model";

/** Pause or resume every live run of this channel, local or on a peer. */
export function useRunControl(runs: LocatedRun[], localNode: string) {
  const { locale } = useI18n();
  const words = workspaceCopy[locale];
  const active = runs.filter((item) => !terminalRun(item.run));
  const paused =
    active.length > 0 && active.every((item) => item.run.control === "PAUSED");
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState("");
  const client = useQueryClient();
  async function toggle() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError("");
    let failed = false;
    for (const { run, node } of active) {
      if (
        (!paused && run.control === "PAUSED") ||
        (paused && !resumableRun(run))
      )
        continue;
      try {
        const action = paused ? "resume" : "pause";
        if (node === localNode) await runControl(run.id, { action });
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
  return {
    paused,
    busy,
    error,
    label: paused ? words.resume : words.pause,
    disabled:
      busy ||
      active.length === 0 ||
      (paused && active.every(({ run }) => !resumableRun(run))),
    toggle,
  };
}

const sectionTitle = "text-[11px] font-medium text-faint";

/** Overview tab of the progress sheet: goal, current tasks, participants and run control. */
export function ChannelStatus({
  workspace,
  data,
  tasks,
  runs,
  waiting,
  discovery,
  open,
  showTasks,
}: {
  workspace: Workspace;
  data: State;
  tasks: Task[];
  runs: LocatedRun[];
  waiting: number;
  discovery?: Discovery;
  open: (value: Selection) => void;
  showTasks: () => void;
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
  const control = useRunControl(runs, data.node.id);
  const nodes = new Set(runs.map((item) => item.node)).size;
  return (
    <section className="flex min-h-0 flex-1 flex-col" aria-label={words.status}>
      <div className="min-h-0 flex-1 divide-y divide-border overflow-y-auto">
        <section className="grid gap-2 pb-4">
          <h3 className={sectionTitle}>
            {locale === "ja-JP" ? "ゴール" : "Goal"}
          </h3>
          <p className="text-[13px] leading-relaxed text-foreground">
            {workspace.goal}
          </p>
          <div className="mt-1 flex items-center gap-3 text-xs text-muted-foreground">
            <span>
              <span className="font-mono tabular text-foreground">
                {progress.completed}/{progress.total}
              </span>{" "}
              {words.taskCounts}
            </span>
            <span aria-hidden className="text-faint">
              ·
            </span>
            <span>
              <span className="font-mono tabular text-foreground">
                {waiting}
              </span>{" "}
              {words.waiting}
            </span>
          </div>
          <SegmentedProgress tasks={tasks} label={words.taskCounts} />
        </section>
        <section className="grid gap-1 py-4">
          <div className="flex items-center justify-between">
            <h3 className={sectionTitle}>{words.currentTasks}</h3>
            <Button variant="ghost" size="sm" type="button" onClick={showTasks}>
              {words.showAll}
            </Button>
          </div>
          {tasks.length === 0 && (
            <p className="text-xs text-muted-foreground">
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
                className="flex h-9 items-center gap-2.5 rounded-md px-2 text-left transition-colors hover:bg-accent"
                key={task.id}
                type="button"
                onClick={() => open({ kind: "taskDetail", task })}
              >
                {task.status === "COMPLETED" ? (
                  <CheckCircle2 className="size-3.5 shrink-0 text-success" />
                ) : (
                  <Circle className="size-3.5 shrink-0 text-faint" />
                )}
                <span className="min-w-0 flex-1 truncate">{task.title}</span>
                <Badge value={task.status} />
              </button>
            ))}
          {tasks.length > 3 && (
            <Button
              variant="link"
              size="sm"
              className="justify-self-start px-2"
              type="button"
              onClick={showTasks}
            >
              + {tasks.length - 3} {words.task}
            </Button>
          )}
        </section>
        <section className="grid gap-1 py-4">
          <div className="flex items-center justify-between">
            <h3 className={sectionTitle}>
              {words.participants}{" "}
              <span className="font-mono tabular">{agents.length}</span>
              {nodes > 1 && (
                <span className="font-normal">
                  {" "}
                  · {nodes} {words.nodes}
                </span>
              )}
            </h3>
            <Button
              variant="ghost"
              size="sm"
              type="button"
              onClick={() => open({ kind: "task", workspace })}
            >
              {words.manage}
            </Button>
          </div>
          {agents.length === 0 && (
            <p className="text-xs text-muted-foreground">
              {words.noParticipants}
            </p>
          )}
          {agents.map((item) => (
            <button
              className="flex h-11 items-center gap-2.5 rounded-md px-2 text-left transition-colors hover:bg-accent"
              type="button"
              key={`${item.node}:${item.run.agent_id}:${item.run.agent_version}`}
              onClick={() => open({ kind: "run", ...item })}
            >
              <Avatar name={label(item)} small />
              <span className="grid min-w-0 flex-1">
                <span className="truncate font-medium">{label(item)}</span>
                <span className="text-[11px] text-faint">
                  {item.node === data.node.id ? words.local : words.peer}
                </span>
              </span>
              <Badge value={runDisplayStatus(item.run)} />
            </button>
          ))}
        </section>
      </div>
      <div className="grid gap-2 border-t border-border pt-4">
        {control.error && (
          <Alert>{control.error}</Alert>
        )}
        <Button
          variant="outline"
          type="button"
          disabled={control.disabled}
          onClick={() => void control.toggle()}
        >
          {control.paused ? <Play /> : <Pause />}
          {control.label}
        </Button>
      </div>
    </section>
  );
}
