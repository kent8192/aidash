import { Button } from "../components/ui/button";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "../components/ui/tabs";
import { useState, type ReactNode } from "react";
import { ArrowUpRight, X } from "lucide-react";
import type { Run, State } from "../types";
import { cn } from "../lib/utils";
import { JsonView, statusTone, useI18n } from "../ui";
import { relatedChannels } from "./model";
import { MeshIcon } from "./mesh-icons";
import {
  Disclosure,
  Dot,
  Facts,
  InspectorSection,
  Metric,
  MetricRow,
  Notice,
} from "../components/patterns";
import type { MeshCopy } from "./mesh-copy";
import {
  eventReferences,
  nodeEvents,
  reference,
  type MeshGraph,
  type MeshNode,
} from "./mesh-model";
import type { Selection } from "./details";
import type { GraphActivity } from "./federated-graph";

const tabs = ["overview", "tasks", "events", "config"] as const;

export function MeshInspector({
  node,
  graph,
  data,
  runs,
  hours,
  now,
  channel,
  copy,
  label,
  status,
  select,
  close,
  open,
  visitChannel,
  remoteActivity = [],
}: {
  node: MeshNode;
  graph: MeshGraph;
  data: State;
  runs: { run: Run; node: string }[];
  hours: number;
  now: number;
  channel: string;
  copy: MeshCopy;
  label: (node: MeshNode) => string;
  status: (node: MeshNode) => string;
  select: (id: string) => void;
  close: () => void;
  open: (selection: Selection) => void;
  visitChannel: (id: string) => void;
  remoteActivity?: GraphActivity[];
}) {
  const { local, locale, t } = useI18n();
  const localNode = (node.homeNodeId ?? node.nodeId) === data.node.id;
  const [tab, setTab] = useState<(typeof tabs)[number]>("overview");
  const entity = localNode
    ? data.registry.find(
        (e) =>
          e.kind === node.kind &&
          e.id === node.entity?.id &&
          e.version === node.entity.version,
      )
    : undefined;
  const task =
    localNode && node.kind === "task"
      ? data.tasks.find((v) => v.id === node.resourceId)
      : undefined;
  const artifact =
    localNode && node.kind === "artifact"
      ? data.artifacts.find((v) => v.id === node.resourceId)
      : undefined;
  const workspace =
    localNode && node.workspaceId
      ? data.workspaces.find((v) => v.id === node.workspaceId)
      : undefined;
  const connectedEdges = graph.edges.filter(
    (e) => e.source === node.id || e.target === node.id,
  );
  const neighborIds = new Set(
    connectedEdges.flatMap((e) => [e.source, e.target]),
  );
  const neighbors = graph.nodes.filter(
    (n) => n.id !== node.id && neighborIds.has(n.id),
  );
  const connectedRunIds = new Set(
    neighbors
      .filter((neighbor) => neighbor.kind === "run")
      .map((neighbor) => neighbor.id),
  );
  const tasks =
    node.kind === "workspace" || node.kind === "goal"
      ? graph.nodes.filter(
          (n) =>
            n.kind === "task" &&
            n.nodeId === node.nodeId &&
            n.workspaceId === node.workspaceId,
        )
      : graph.nodes.filter(
          (candidate) =>
            candidate.kind === "task" &&
            (neighborIds.has(candidate.id) ||
              graph.edges.some(
                (edge) =>
                  (connectedRunIds.has(edge.source) &&
                    edge.target === candidate.id) ||
                  (connectedRunIds.has(edge.target) &&
                    edge.source === candidate.id),
              )),
        );
  const events = localNode
    ? nodeEvents(node, graph, data, hours, now, channel)
    : remoteActivity.map((marker, index) => ({
        id: `${marker.reference}:${marker.at}:${index}`,
        kind: marker.kind,
        created_at: marker.at,
      }));
  const related = workspace
    ? [workspace]
    : localNode
      ? relatedChannels(
          node,
          {
            nodeId: data.node.id,
            workspaces: data.workspaces,
            tasks: data.tasks,
            runs: data.runs,
            registry: data.registry,
          },
          channel,
        )
      : [];
  const modelRef = reference(entity?.config.model);
  const model = modelRef
    ? data.registry.find(
        (e) =>
          e.kind === "model" &&
          e.id === modelRef.id &&
          e.version === modelRef.version,
      )
    : undefined;
  // Agents show their most recently updated run on this node; Run nodes show themselves.
  const run =
    node.kind === "run"
      ? runs.find(
          (item) =>
            item.run.id === node.resourceId && item.node === node.nodeId,
        )?.run
      : node.kind === "agent" && node.entity
        ? runs
            .filter(
              (item) =>
                item.node === node.nodeId &&
                item.run.agent_id === node.entity!.id &&
                item.run.agent_version === node.entity!.version,
            )
            .map((item) => item.run)
            .sort((a, b) => b.updated_at.localeCompare(a.updated_at))[0]
        : undefined;
  const runStarted = run
    ? Math.min(
        ...data.events
          .filter((event) => eventReferences(event).run_id === run.id)
          .map((event) => Date.parse(event.created_at))
          .filter(Number.isFinite),
      )
    : Infinity;
  const runEnded = run
    ? ["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase)
      ? Date.parse(run.updated_at)
      : now
    : NaN;
  const elapsed = Number.isFinite(runStarted)
    ? Math.max(0, Math.round((runEnded - runStarted) / 1000))
    : undefined;
  const number = new Intl.NumberFormat(locale);
  const runStats = run
    ? ([
        [copy.step, number.format(run.step)],
        elapsed !== undefined && [
          copy.duration,
          elapsed >= 3600
            ? `${Math.floor(elapsed / 3600)}h ${Math.floor((elapsed % 3600) / 60)}m`
            : `${Math.floor(elapsed / 60)}m ${elapsed % 60}s`,
        ],
        run.context?.usage && [
          copy.inputTokens,
          number.format(run.context.usage.input_tokens),
        ],
        run.context?.usage && [
          copy.outputTokens,
          number.format(run.context.usage.output_tokens),
        ],
        run.context?.history && [
          copy.toolCalls,
          number.format(
            run.context.history.filter((entry) => entry.kind === "tool").length,
          ),
        ],
      ].filter(Boolean) as [string, string][])
    : [];
  const date = (value: string) => {
    const parsed = new Date(value);
    return Number.isNaN(parsed.getTime())
      ? "—"
      : parsed.toLocaleString(locale, {
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        });
  };
  const statusLine = (n: MeshNode) => (
    <span className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
      <Dot tone={n.status ? statusTone(n.status) : "neutral"} />
      <span className="truncate">{status(n)}</span>
    </span>
  );
  const nodeLink = (n: MeshNode) => (
    <button
      type="button"
      className="flex h-9 w-full min-w-0 items-center gap-2 rounded-md px-2 text-left text-[13px] transition-colors hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring"
      key={n.id}
      onClick={() => select(n.id)}
    >
      <MeshIcon
        kind={n.kind}
        size={14}
        className="shrink-0 text-muted-foreground"
      />
      <span className="min-w-0 flex-1 truncate">{label(n)}</span>
      {n.status ? (
        <span className="shrink-0">{statusLine(n)}</span>
      ) : (
        <small className="shrink-0 text-[11px] text-faint">
          {copy.kinds[n.kind]}
        </small>
      )}
    </button>
  );
  const eventList = (limit: number) =>
    events.length ? (
      <ol className="grid">
        {events.slice(0, limit).map((e) => (
          <li
            key={e.id}
            className="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-2.5 py-1"
          >
            <span
              aria-hidden="true"
              className={cn(
                "size-1.5 translate-y-[-1px] rounded-full",
                /fail|block/i.test(e.kind) ? "bg-destructive" : "bg-edge",
              )}
            />
            <span className="flex min-w-0 items-baseline justify-between gap-3">
              <span className="truncate">{t(e.kind).replaceAll("_", " ")}</span>
              <time
                className="shrink-0 font-mono text-[11px] text-faint tabular"
                dateTime={e.created_at}
              >
                {date(e.created_at)}
              </time>
            </span>
          </li>
        ))}
      </ol>
    ) : (
      <p className="text-xs text-faint">{copy.noEvents}</p>
    );
  const buckets = Array.from({ length: 24 }, () => 0);
  const oldest = hours
    ? now - hours * 3_600_000
    : Math.min(
        now - 3_600_000,
        ...events.map((e) => Date.parse(e.created_at)).filter(Number.isFinite),
      );
  for (const event of events) {
    const i = Math.min(
      23,
      Math.max(
        0,
        Math.floor(
          ((Date.parse(event.created_at) - oldest) / (now - oldest)) * 24,
        ),
      ),
    );
    if (Number.isFinite(i)) buckets[i]++;
  }
  const maxBucket = Math.max(1, ...buckets);
  const facts: [string, ReactNode, boolean?][] = [
    [copy.type, copy.kinds[node.kind]],
    [copy.status, node.status ? statusLine(node) : status(node)],
    ...(node.entity
      ? [[copy.version, node.entity.version, true] as [string, string, boolean]]
      : []),
    [copy.node, node.nodeId, true],
    ...(node.at
      ? [[copy.time, date(node.at), true] as [string, string, boolean]]
      : []),
    ...(modelRef
      ? [
          [
            copy.model,
            `${model ? local(model.name) : copy.missing} · ${modelRef.version}`,
            true,
          ] as [string, string, boolean],
        ]
      : []),
    ...(workspace
      ? [[copy.kinds.workspace, workspace.title] as [string, string]]
      : []),
  ];
  const connections = (all: boolean) => (
    <>
      <InspectorSection
        level={3}
        title={copy.connected}
        count={neighbors.length}
      >
        {neighbors.length ? (
          <div className="-mx-2 grid">
            {neighbors.slice(0, all ? 180 : 6).map(nodeLink)}
          </div>
        ) : (
          <p className="text-xs text-faint">{copy.noConnections}</p>
        )}
        {all && (
          <ul className="mesh-relations-list grid gap-1 border-t border-border pt-2 text-xs text-muted-foreground">
            {connectedEdges.map((e) => (
              <li key={e.id}>
                {label(graph.nodes.find((n) => n.id === e.source)!)} →{" "}
                <span className="text-faint">{copy.relations[e.relation]}</span>{" "}
                → {label(graph.nodes.find((n) => n.id === e.target)!)}
              </li>
            ))}
          </ul>
        )}
      </InspectorSection>
      <InspectorSection level={3} title={copy.related} count={related.length}>
        <div className="flex flex-wrap gap-1.5">
          {related.map((w) => (
            <Button
              variant="outline"
              size="sm"
              type="button"
              className="collab-channel-link"
              key={w.id}
              onClick={() => visitChannel(w.id)}
            >
              # {w.title}
            </Button>
          ))}
        </div>
      </InspectorSection>
    </>
  );
  return (
    <aside
      className="mesh-inspector collab-selection z-30 flex min-h-0 w-full flex-col bg-surface max-md:absolute max-md:inset-x-0 max-md:bottom-0 max-md:h-[min(57dvh,460px)] max-md:rounded-t-lg max-md:border-t max-md:border-border-strong max-md:shadow-overlay md:w-[300px] md:shrink-0 md:border-l md:border-border lg:w-[340px]"
      aria-label={copy.select}
    >
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-border pl-4 pr-2">
        <span className="flex min-w-0 items-center gap-1.5 text-xs text-faint">
          <MeshIcon kind={node.kind} size={12} className="shrink-0" />
          <span className="shrink-0">{copy.kinds[node.kind]}</span>
          {node.resourceId && (
            <span className="truncate font-mono">· {node.resourceId}</span>
          )}
        </span>
        <Button
          variant="ghost"
          size="icon"
          className="ml-auto size-7"
          aria-label={copy.close}
          title={copy.close}
          onClick={close}
        >
          <X />
        </Button>
      </div>
      <Tabs
        value={tab}
        onValueChange={(value) => setTab(value as (typeof tabs)[number])}
        className="flex min-h-0 flex-1 flex-col"
      >
        <header className="flex shrink-0 items-start gap-3 px-4 pb-3 pt-4">
          <span className="grid size-10 shrink-0 place-items-center rounded-md border border-border-strong bg-raised text-foreground">
            {node.kind === "agent" ? (
              <span className="text-[15px] font-semibold">
                {label(node).trim().slice(0, 1).toUpperCase()}
              </span>
            ) : (
              <MeshIcon kind={node.kind} size={18} />
            )}
          </span>
          <div className="grid min-w-0 gap-0.5">
            <h2
              className={cn(
                "break-words text-[15px] font-semibold leading-snug",
                (node.kind === "remote" ||
                  node.kind === "tool" ||
                  node.kind === "model") &&
                  "font-mono text-[13px]",
              )}
            >
              {label(node)}
            </h2>
            {entity && local(entity.description) ? (
              <p className="text-xs text-muted-foreground">
                {local(entity.description)}
              </p>
            ) : (
              statusLine(node)
            )}
          </div>
        </header>
        <TabsList aria-label={copy.select}>
          {tabs.map((name) => (
            <TabsTrigger key={name} value={name}>
              {copy[name]}{" "}
              {name === "tasks" && tasks.length > 0 && (
                <span className="font-mono text-[11px] text-faint tabular">
                  {tasks.length}
                </span>
              )}
              {name === "events" && events.length > 0 && (
                <span className="font-mono text-[11px] text-faint tabular">
                  {events.length}
                </span>
              )}
            </TabsTrigger>
          ))}
        </TabsList>
        <div className="min-h-0 flex-1 overflow-y-auto">
          {!node.available && (
            <Notice tone="warning" role="status" className="mx-4 mt-3">
              {copy.missing}
            </Notice>
          )}
          <TabsContent value="overview">
            <InspectorSection level={3} title={copy.about}>
              <Facts items={facts} />
              {task && (
                <p className="text-xs text-muted-foreground">
                  {task.description}
                </p>
              )}
              {node.kind === "goal" && node.goalBody && (
                <p className="whitespace-pre-wrap text-xs text-muted-foreground">
                  {node.goalBody}
                </p>
              )}
              {(node.kind === "workspace" || node.kind === "goal") &&
                workspace && (
                  <p className="text-xs text-muted-foreground">
                    {workspace.goal}
                  </p>
                )}
              {artifact && (
                <>
                  <p className="text-xs text-muted-foreground">
                    {t(artifact.kind)} · {date(artifact.created_at)}
                  </p>
                  <Disclosure summary={copy.open}>
                    <JsonView value={artifact.content} />
                  </Disclosure>
                </>
              )}
            </InspectorSection>
            {run && (
              <InspectorSection
                level={3}
                title={copy.run}
                action={
                  <span className="font-mono text-[11px] text-faint">
                    {run.id}
                  </span>
                }
              >
                <MetricRow columns={2} compact className="border-y">
                  {runStats.map(([term, value]) => (
                    <Metric key={term} label={term} value={value} />
                  ))}
                  <Metric
                    label={copy.phase}
                    value={t(run.phase)}
                    tone={statusTone(run.phase)}
                  />
                </MetricRow>
              </InspectorSection>
            )}
            {!!entity?.capabilities.length && (
              <InspectorSection level={3} title={copy.capabilities}>
                <div className="flex flex-wrap gap-1.5">
                  {entity.capabilities.map((c) => (
                    <span
                      key={c}
                      className="rounded-sm border border-border px-1.5 py-0.5 text-xs text-muted-foreground"
                    >
                      {c}
                    </span>
                  ))}
                </div>
              </InspectorSection>
            )}
            <InspectorSection level={3} title={copy.activity}>
              <div className="flex items-end gap-3">
                <div
                  className="flex h-9 flex-1 items-end gap-px"
                  role="img"
                  aria-label={`${events.length} ${copy.events}`}
                >
                  {buckets.map((count, i) => (
                    <span
                      key={i}
                      className={cn(
                        "flex-1 rounded-t-[1px]",
                        count ? "bg-brand-mark/70" : "bg-border",
                      )}
                      style={{
                        height: `${Math.max(6, (count / maxBucket) * 100)}%`,
                      }}
                    />
                  ))}
                </div>
                <span className="grid text-right">
                  <strong className="font-mono text-[15px] font-medium tabular">
                    {events.length}
                  </strong>
                  <small className="text-[11px] text-faint">
                    {copy.events}
                  </small>
                </span>
              </div>
            </InspectorSection>
            <InspectorSection
              level={3}
              title={copy.recent}
              action={
                <Button
                  variant="ghost"
                  size="icon"
                  className="size-6"
                  onClick={() => setTab("events")}
                  aria-label={copy.events}
                >
                  <ArrowUpRight />
                </Button>
              }
            >
              {eventList(5)}
            </InspectorSection>
            <InspectorSection level={3} title={copy.tasks} count={tasks.length}>
              {tasks.length ? (
                <div className="-mx-2 grid">
                  {tasks.slice(0, 8).map(nodeLink)}
                </div>
              ) : (
                <p className="text-xs text-faint">{copy.noTasks}</p>
              )}
            </InspectorSection>
            {connections(false)}
          </TabsContent>
          <TabsContent value="tasks">
            <InspectorSection level={3} title={copy.tasks} count={tasks.length}>
              {tasks.length ? (
                <div className="-mx-2 grid">{tasks.map(nodeLink)}</div>
              ) : (
                <p className="text-xs text-faint">{copy.noTasks}</p>
              )}
            </InspectorSection>
            {task && (
              <InspectorSection level={3} title={copy.relations.depends}>
                {connectedEdges
                  .filter((e) => e.relation === "depends")
                  .map((e) => (
                    <p key={e.id} className="text-xs">
                      {label(graph.nodes.find((n) => n.id === e.source)!)} →{" "}
                      {label(graph.nodes.find((n) => n.id === e.target)!)}
                    </p>
                  ))}
              </InspectorSection>
            )}
          </TabsContent>
          <TabsContent value="events">
            <InspectorSection
              level={3}
              title={copy.events}
              count={events.length}
            >
              {eventList(100)}
              {events.length > 100 && (
                <p className="text-xs text-faint">
                  {copy.omitted}: {events.length - 100}
                </p>
              )}
            </InspectorSection>
          </TabsContent>
          <TabsContent value="config">{connections(true)}</TabsContent>
        </div>
      </Tabs>
      {(entity || task || workspace) && (
        <footer className="flex shrink-0 gap-2 border-t border-border px-4 py-3">
          <Button
            variant="outline"
            type="button"
            onClick={() => {
              if (entity) open({ kind: "entityDetail", entity });
              else if (task) open({ kind: "taskDetail", task });
              else if (workspace) visitChannel(workspace.id);
            }}
          >
            {entity || task ? copy.open : copy.channel}
            <ArrowUpRight />
          </Button>
        </footer>
      )}
    </aside>
  );
}
