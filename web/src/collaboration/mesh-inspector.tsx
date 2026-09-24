import { useState, type CSSProperties } from "react";
import { ArrowUpRight, X } from "lucide-react";
import type { State } from "../types";
import { JsonView, useI18n } from "../ui";
import { relatedChannels } from "./model";
import { MeshIcon, meshColors } from "./mesh-icons";
import type { MeshCopy } from "./mesh-copy";
import {
  nodeEvents,
  reference,
  type MeshGraph,
  type MeshNode,
} from "./mesh-model";
import type { Selection } from "./details";

export function MeshInspector({
  node,
  graph,
  data,
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
}: {
  node: MeshNode;
  graph: MeshGraph;
  data: State;
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
}) {
  const { local, locale, t } = useI18n();
  const [tab, setTab] = useState<"overview" | "tasks" | "events" | "config">(
    "overview",
  );
  const entity =
    node.nodeId === data.node.id
      ? data.registry.find(
          (e) =>
            e.kind === node.kind &&
            e.id === node.entity?.id &&
            e.version === node.entity.version,
        )
      : undefined;
  const task =
    node.kind === "task"
      ? data.tasks.find((v) => v.id === node.resourceId)
      : undefined;
  const artifact =
    node.kind === "artifact"
      ? data.artifacts.find((v) => v.id === node.resourceId)
      : undefined;
  const workspace = node.workspaceId
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
  const tasks =
    node.kind === "workspace" || node.kind === "goal"
      ? graph.nodes.filter(
          (n) => n.kind === "task" && n.workspaceId === node.workspaceId,
        )
      : neighbors.filter((n) => n.kind === "task");
  const events = nodeEvents(node, graph, data, hours, now, channel);
  const related = workspace
    ? [workspace]
    : node.nodeId === data.node.id
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
  const nodeLink = (n: MeshNode) => (
    <button
      type="button"
      className="mesh-connected-node"
      key={n.id}
      onClick={() => select(n.id)}
    >
      <span style={{ color: meshColors[n.kind] }}>
        <MeshIcon kind={n.kind} size={15} />
      </span>
      <span>{label(n)}</span>
      {n.status ? (
        <span className={`mesh-status status-${n.status.toLowerCase()}`}>
          {status(n)}
        </span>
      ) : (
        <small>{copy.kinds[n.kind]}</small>
      )}
    </button>
  );
  const eventList = (limit: number) =>
    events.length ? (
      <ol className="mesh-event-list">
        {events.slice(0, limit).map((e) => (
          <li key={e.id}>
            <span
              className={`mesh-event-dot ${/fail|block/i.test(e.kind) ? "warning" : ""}`}
            />
            <span>
              {t(e.kind).replaceAll("_", " ")}
              <time dateTime={e.created_at}>{date(e.created_at)}</time>
            </span>
          </li>
        ))}
      </ol>
    ) : (
      <p className="mesh-muted">{copy.noEvents}</p>
    );
  const buckets = Array.from({ length: 16 }, () => 0);
  const oldest = hours
    ? now - hours * 3_600_000
    : Math.min(
        now - 3_600_000,
        ...events.map((e) => Date.parse(e.created_at)).filter(Number.isFinite),
      );
  for (const event of events) {
    const i = Math.min(
      15,
      Math.max(
        0,
        Math.floor(
          ((Date.parse(event.created_at) - oldest) / (now - oldest)) * 16,
        ),
      ),
    );
    if (Number.isFinite(i)) buckets[i]++;
  }
  const maxBucket = Math.max(1, ...buckets);
  return (
    <aside
      className="mesh-inspector collab-selection"
      aria-label={copy.select}
      style={{ "--node-color": meshColors[node.kind] } as CSSProperties}
    >
      <div className="mesh-inspector-top">
        <span>
          <MeshIcon kind={node.kind} size={12} />
          {copy.kinds[node.kind]}
        </span>
        <button type="button" aria-label={copy.close} onClick={close}>
          <X size={16} />
        </button>
      </div>
      <header className="mesh-inspector-title">
        <div className="mesh-inspector-symbol">
          <MeshIcon kind={node.kind} size={26} />
        </div>
        <div>
          <h2>{label(node)}</h2>
          <span
            className={`mesh-status status-${node.status?.toLowerCase() ?? "unknown"}`}
          >
            {status(node)}
          </span>
        </div>
      </header>
      {entity && local(entity.description) && (
        <p className="mesh-description">{local(entity.description)}</p>
      )}
      <div
        className="mesh-inspector-tabs"
        role="group"
        aria-label={copy.select}
      >
        {(["overview", "tasks", "events", "config"] as const).map((name) => (
          <button
            type="button"
            key={name}
            aria-pressed={tab === name}
            onClick={() => setTab(name)}
          >
            {copy[name]}
            {name === "tasks" && tasks.length > 0 && (
              <small>{tasks.length}</small>
            )}
            {name === "events" && events.length > 0 && (
              <small>{events.length}</small>
            )}
          </button>
        ))}
      </div>
      <div className="mesh-inspector-content">
        {!node.available && <p role="status">{copy.missing}</p>}
        {tab === "overview" && (
          <>
            <section>
              <h3>{copy.about}</h3>
              <dl className="mesh-metadata">
                <dt>{copy.type}</dt>
                <dd>{copy.kinds[node.kind]}</dd>
                <dt>{copy.status}</dt>
                <dd>{status(node)}</dd>
                {node.entity && (
                  <>
                    <dt>{copy.version}</dt>
                    <dd>{node.entity.version}</dd>
                  </>
                )}
                <dt>{copy.node}</dt>
                <dd>{node.nodeId}</dd>
                {modelRef && (
                  <>
                    <dt>{copy.model}</dt>
                    <dd>
                      {model ? local(model.name) : copy.missing} ·{" "}
                      {modelRef.version}
                    </dd>
                  </>
                )}
                {workspace && (
                  <>
                    <dt>{copy.kinds.workspace}</dt>
                    <dd>{workspace.title}</dd>
                  </>
                )}
              </dl>
              {task && <p className="mesh-description">{task.description}</p>}
              {(node.kind === "workspace" || node.kind === "goal") &&
                workspace && (
                  <p className="mesh-description">{workspace.goal}</p>
                )}
              {artifact && (
                <>
                  <p>
                    {t(artifact.kind)} · {date(artifact.created_at)}
                  </p>
                  <details className="mesh-artifact">
                    <summary>{copy.open}</summary>
                    <JsonView value={artifact.content} />
                  </details>
                </>
              )}
            </section>
            {!!entity?.capabilities.length && (
              <section>
                <h3>{copy.capabilities}</h3>
                <div className="mesh-tags">
                  {entity.capabilities.map((c) => (
                    <span key={c}>{c}</span>
                  ))}
                </div>
              </section>
            )}
            <section>
              <h3>{copy.activity}</h3>
              <div className="mesh-activity">
                <div
                  className="mesh-histogram"
                  role="img"
                  aria-label={`${events.length} ${copy.events}`}
                >
                  {buckets.map((count, i) => (
                    <span
                      key={i}
                      style={{
                        height: `${Math.max(2, (count / maxBucket) * 100)}%`,
                        opacity: count ? 1 : 0.15,
                      }}
                    />
                  ))}
                </div>
                <div>
                  <strong>{events.length}</strong>
                  <small>{copy.events}</small>
                </div>
              </div>
            </section>
            <section>
              <h3>
                {copy.recent}
                <button
                  type="button"
                  onClick={() => setTab("events")}
                  aria-label={copy.events}
                >
                  <ArrowUpRight size={13} />
                </button>
              </h3>
              {eventList(5)}
            </section>
            <section>
              <h3>{copy.tasks}</h3>
              {tasks.length ? (
                tasks.slice(0, 8).map(nodeLink)
              ) : (
                <p className="mesh-muted">{copy.noTasks}</p>
              )}
            </section>
          </>
        )}
        {tab === "tasks" && (
          <section>
            <h3>
              {copy.tasks} · {tasks.length}
            </h3>
            {tasks.length ? (
              tasks.map(nodeLink)
            ) : (
              <p className="mesh-muted">{copy.noTasks}</p>
            )}
            {task && (
              <>
                <h3>{copy.relations.depends}</h3>
                {connectedEdges
                  .filter((e) => e.relation === "depends")
                  .map((e) => (
                    <p key={e.id}>
                      {label(graph.nodes.find((n) => n.id === e.source)!)} →{" "}
                      {label(graph.nodes.find((n) => n.id === e.target)!)}
                    </p>
                  ))}
              </>
            )}
          </section>
        )}
        {tab === "events" && (
          <section>
            <h3>
              {copy.events} · {events.length}
            </h3>
            {eventList(100)}
            {events.length > 100 && (
              <p>
                {copy.omitted}: {events.length - 100}
              </p>
            )}
          </section>
        )}
        {(tab === "overview" || tab === "config") && (
          <>
            <section>
              <h3>{copy.connected}</h3>
              {neighbors.length ? (
                neighbors.slice(0, tab === "config" ? 180 : 6).map(nodeLink)
              ) : (
                <p className="mesh-muted">{copy.noConnections}</p>
              )}
              {tab === "config" && (
                <ul className="mesh-relations-list">
                  {connectedEdges.map((e) => (
                    <li key={e.id}>
                      {label(graph.nodes.find((n) => n.id === e.source)!)} →{" "}
                      {copy.relations[e.relation]} →{" "}
                      {label(graph.nodes.find((n) => n.id === e.target)!)}
                    </li>
                  ))}
                </ul>
              )}
            </section>
            <section>
              <h3>{copy.related}</h3>
              {related.map((w) => (
                <button
                  type="button"
                  className="collab-channel-link"
                  key={w.id}
                  onClick={() => visitChannel(w.id)}
                >
                  # {w.title}
                </button>
              ))}
            </section>
          </>
        )}
      </div>
      {(entity || task || workspace) && (
        <footer className="mesh-inspector-footer">
          <button
            type="button"
            onClick={() => {
              if (entity) open({ kind: "entityDetail", entity });
              else if (task) open({ kind: "taskDetail", task });
              else if (workspace) visitChannel(workspace.id);
            }}
          >
            {entity || task ? copy.open : copy.channel}
            <ArrowUpRight size={14} />
          </button>
        </footer>
      )}
    </aside>
  );
}
