import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Check as Tick } from "lucide-react";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import {
  Alert,
  Check,
  Hint,
  HistoryRow,
  Loading,
  Pager,
  RowList,
} from "./components/patterns";
import {
  tenantNodeRegistrationHistory,
  tenantNodeRegistrationSet,
  tenantNodeRegistrations,
  tenantNodeUpdate,
  tenantNodesCreate,
} from "./generated/aidash";
import { useNodes } from "./nodes";
import {
  changedRegistration,
  inOtherDeployment,
  newRegistration,
  type RegistrationDraft,
} from "./node-registrations";
import { registryDeployment } from "./node-selection";
import { RecordView } from "./record-view";
import { ApiError } from "./transport";
import { useTenantActions } from "./tenant-administration";
import { Badge, Field, Modal, Panel, useI18n } from "./ui";
import type { State } from "./types";

const PAGE_SIZE = 25;

const rowClass =
  "grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-2 py-2 md:grid-cols-[minmax(0,1fr)_auto_auto]";

/** What a refused change tells the person: a stale revision, a missing
 *  permission, or the server's own reason (a limit, an unreachable Deployment,
 *  a key ID that does not match). */
function failure(reason: unknown): string {
  if (reason instanceof ApiError && reason.status === 409)
    return "nodeConflict";
  if (reason instanceof ApiError && reason.status === 403) return "nodeDenied";
  return reason instanceof Error ? reason.message : String(reason);
}

/** The Tenant's Nodes and the Node Registrations of the selected one (#241). */
export function NodesPage({ data }: { data: State }) {
  const { t } = useI18n();
  const { tenant, can } = useTenantActions(data.access);
  const { nodes, selected, select } = useNodes();
  const client = useQueryClient();
  const [labelling, setLabelling] = useState<
    { name: string; label: string } | "new" | null
  >(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  if (tenant === null) return <Hint>{t("nodesTenantOnly")}</Hint>;
  const path = encodeURIComponent(tenant);
  const labelNode = async (label: string) => {
    if (busy || labelling === null) return;
    setBusy(true);
    setError("");
    try {
      if (labelling === "new") {
        const created = await tenantNodesCreate(path, { label });
        select(created.name);
      } else await tenantNodeUpdate(path, labelling.name, { label });
      setLabelling(null);
      await client.invalidateQueries({ queryKey: ["nodes", tenant] });
    } catch (reason) {
      setError(failure(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="grid max-w-4xl min-w-0 gap-6">
      <Panel
        title={t("nodes")}
        action={
          can("node.create") && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setError("");
                setLabelling("new");
              }}
            >
              {t("nodeAdd")}
            </Button>
          )
        }
      >
        <Hint>{t("nodesHelp")}</Hint>
        {nodes.length === 0 && <Loading />}
        {nodes.length > 0 && (
          <RowList>
            {nodes.map((node) => (
              <div className={rowClass} key={node.name} data-node={node.name}>
                <div className="grid min-w-0 gap-0.5">
                  <span className="flex min-w-0 items-center gap-2">
                    <span className="truncate font-medium text-foreground">
                      {node.label}
                    </span>
                    {node.default && <Badge value="nodeDefault" tone="brand" />}
                    {node.name === selected?.name && (
                      <Tick
                        aria-label={t("nodeSelected")}
                        className="size-3.5 text-brand"
                      />
                    )}
                  </span>
                  <span className="truncate font-mono text-[11px] text-muted-foreground">
                    {node.address}
                  </span>
                </div>
                <span className="hidden md:block" />
                <div className="col-span-2 flex flex-wrap gap-1.5 md:col-span-1">
                  {node.name !== selected?.name && (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => select(node.name)}
                    >
                      {t("nodeSelect")}
                    </Button>
                  )}
                  {can("node.update") && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => {
                        setError("");
                        setLabelling({ name: node.name, label: node.label });
                      }}
                    >
                      {t("nodeChangeLabel")}
                    </Button>
                  )}
                </div>
              </div>
            ))}
          </RowList>
        )}
      </Panel>
      {selected && can("node_registration.read") && (
        <Registrations
          key={selected.name}
          tenant={tenant}
          node={selected.name}
          label={selected.label}
          own={registryDeployment(data)}
          editable={can("node_registration.update")}
        />
      )}
      {labelling && (
        <Modal
          title={t(labelling === "new" ? "nodeAdd" : "nodeChangeLabel")}
          close={() => {
            if (!busy) setLabelling(null);
          }}
        >
          <form
            className="grid min-w-0 gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              void labelNode(
                String(new FormData(event.currentTarget).get("label")).trim(),
              );
            }}
          >
            <Field label={t("nodeLabel")}>
              <Input
                name="label"
                required
                maxLength={64}
                defaultValue={labelling === "new" ? "" : labelling.label}
              />
            </Field>
            <Hint>{t("nodeLabelHelp")}</Hint>
            {error && <Alert>{t(error)}</Alert>}
            <div className="flex justify-end border-t border-border pt-4">
              <Button disabled={busy}>{t("save")}</Button>
            </div>
          </form>
        </Modal>
      )}
    </div>
  );
}

function Registrations({
  tenant,
  node,
  label,
  own,
  editable,
}: {
  tenant: string;
  node: string;
  label: string;
  /** `aidash://<name>` of this Deployment. */
  own: string;
  editable: boolean;
}) {
  const { t, locale } = useI18n();
  const client = useQueryClient();
  const [offset, setOffset] = useState(0);
  const [cursors, setCursors] = useState([0]);
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState<RegistrationDraft>({
    source_node: "",
    endpoint: "",
    dashboard_origin: "",
    key_id: "",
    atomic_transactions: false,
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const after = cursors[cursors.length - 1];
  const registrations = useQuery({
    queryKey: ["node-registrations", tenant, node, offset],
    queryFn: () =>
      tenantNodeRegistrations(tenant, node, { offset, limit: PAGE_SIZE }),
    refetchInterval: 5000,
  });
  const history = useQuery({
    queryKey: ["node-registration-history", tenant, node, after],
    queryFn: () =>
      tenantNodeRegistrationHistory(tenant, node, { after, limit: PAGE_SIZE }),
    refetchInterval: 5000,
  });
  const rows = registrations.isError ? undefined : registrations.data;
  const revisions = history.isError ? undefined : history.data;
  const save = async (
    input: Parameters<typeof tenantNodeRegistrationSet>[2],
  ) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await tenantNodeRegistrationSet(
        encodeURIComponent(tenant),
        encodeURIComponent(node),
        input,
      );
      setAdding(false);
      await client.invalidateQueries({ queryKey: ["node-registrations"] });
      await client.invalidateQueries({
        queryKey: ["node-registration-history"],
      });
    } catch (reason) {
      setError(failure(reason));
      await client.invalidateQueries({ queryKey: ["node-registrations"] });
    } finally {
      setBusy(false);
    }
  };
  const remote = inOtherDeployment(draft.source_node.trim(), own);
  return (
    <>
      <Panel
        title={`${t("nodeRegistrations")} · ${label}`}
        action={
          editable && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setError("");
                setDraft({
                  source_node: "",
                  endpoint: "",
                  dashboard_origin: "",
                  key_id: "",
                  atomic_transactions: false,
                });
                setAdding(true);
              }}
            >
              {t("nodeRegistrationAdd")}
            </Button>
          )
        }
      >
        <Hint>{t("nodeRegistrationsHelp")}</Hint>
        {error && !adding && <Alert>{t(error)}</Alert>}
        {registrations.isError && <Alert>{registrations.error.message}</Alert>}
        {registrations.isPending && <Loading />}
        {rows?.length === 0 && (
          <p className="text-xs text-muted-foreground">
            {t("nodeRegistrationNone")}
          </p>
        )}
        {!!rows?.length && (
          <RowList>
            {rows.map((row) => (
              <div
                className={rowClass}
                key={row.source_node}
                data-registration={row.source_node}
              >
                <div className="grid min-w-0 gap-0.5">
                  <span className="truncate font-mono font-medium text-foreground">
                    {row.source_node}
                  </span>
                  {row.endpoint && (
                    <span className="truncate font-mono text-[11px] text-muted-foreground">
                      {row.endpoint}
                    </span>
                  )}
                  {row.key_id && (
                    <span className="truncate text-[11px] text-faint">
                      {t("nodeRegistrationDashboardOrigin")}:{" "}
                      {row.dashboard_origin ?? "—"}
                    </span>
                  )}
                  {row.key_id && (
                    <span className="truncate text-[11px] text-faint">
                      {t("nodeKeyId")}{" "}
                      <span className="font-mono text-muted-foreground">
                        {row.key_id}
                      </span>
                    </span>
                  )}
                  <span className="truncate text-[11px] text-faint">
                    {t("revision")}{" "}
                    <span className="font-mono tabular">{row.revision}</span>
                  </span>
                </div>
                <div className="flex flex-wrap gap-1.5">
                  <Badge
                    value={
                      row.enabled
                        ? "nodeRegistrationEnabled"
                        : "nodeRegistrationDisabled"
                    }
                    tone={row.enabled ? "success" : "neutral"}
                  />
                  {row.atomic_transactions && (
                    <Badge value="nodeRegistrationAtomic" tone="brand" />
                  )}
                </div>
                {editable && (
                  <div className="col-span-2 flex flex-wrap gap-1.5 md:col-span-1">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy}
                      onClick={() =>
                        void save(
                          changedRegistration(row, { enabled: !row.enabled }),
                        )
                      }
                    >
                      {t(
                        row.enabled
                          ? "nodeRegistrationDisable"
                          : "nodeRegistrationEnable",
                      )}
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={busy}
                      onClick={() =>
                        void save(
                          changedRegistration(row, {
                            atomic_transactions: !row.atomic_transactions,
                          }),
                        )
                      }
                    >
                      {t(
                        row.atomic_transactions
                          ? "nodeRegistrationAtomicOff"
                          : "nodeRegistrationAtomicOn",
                      )}
                    </Button>
                  </div>
                )}
              </div>
            ))}
          </RowList>
        )}
        <Pager
          page={offset / PAGE_SIZE + 1}
          previous={{
            disabled: offset === 0 || registrations.isFetching,
            go: () => setOffset(Math.max(0, offset - PAGE_SIZE)),
          }}
          next={{
            disabled:
              !rows || rows.length < PAGE_SIZE || registrations.isFetching,
            go: () => setOffset(offset + PAGE_SIZE),
          }}
        />
      </Panel>
      <Panel title={t("nodeRegistrationHistory")}>
        {history.isError && <Alert>{history.error.message}</Alert>}
        {history.isPending && <Loading />}
        {revisions?.length === 0 && (
          <p className="text-xs text-muted-foreground">{t("authNoHistory")}</p>
        )}
        {!!revisions?.length && (
          <RowList>
            {revisions.map((revision) => (
              <HistoryRow
                key={revision.sequence}
                time={new Date(revision.updated_at).toLocaleString(locale)}
                summary={
                  <>
                    <span className="font-mono">{revision.source_node}</span>
                    <span className="text-muted-foreground">
                      {" · "}
                      {t(
                        revision.enabled
                          ? "nodeRegistrationEnabled"
                          : "nodeRegistrationDisabled",
                      )}
                    </span>
                  </>
                }
              >
                <RecordView value={revision} />
              </HistoryRow>
            ))}
          </RowList>
        )}
        <Pager
          page={cursors.length}
          previous={{
            disabled: cursors.length === 1 || history.isFetching,
            go: () => setCursors(cursors.slice(0, -1)),
          }}
          next={{
            disabled:
              !revisions || revisions.length < PAGE_SIZE || history.isFetching,
            go: () => {
              const next = revisions?.at(-1)?.sequence;
              if (next !== undefined) setCursors([...cursors, next]);
            },
          }}
        />
      </Panel>
      {adding && (
        <Modal
          title={t("nodeRegistrationAdd")}
          close={() => {
            if (!busy) setAdding(false);
          }}
        >
          <form
            className="grid min-w-0 gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              void save(newRegistration(draft, own));
            }}
          >
            <Field label={t("nodeRegistrationSource")}>
              <Input
                required
                className="font-mono"
                placeholder="aidash://<deployment>/<node>"
                value={draft.source_node}
                onChange={(event) =>
                  setDraft({ ...draft, source_node: event.target.value })
                }
              />
            </Field>
            {remote && (
              <>
                <Field label={t("endpoint")}>
                  <Input
                    required
                    type="url"
                    className="font-mono"
                    placeholder="https://aidash.example.com"
                    value={draft.endpoint}
                    onChange={(event) =>
                      setDraft({ ...draft, endpoint: event.target.value })
                    }
                  />
                </Field>
                <Field label={t("nodeKeyId")}>
                  <Input
                    required
                    className="font-mono"
                    maxLength={64}
                    value={draft.key_id}
                    onChange={(event) =>
                      setDraft({ ...draft, key_id: event.target.value })
                    }
                  />
                </Field>
                <Field label={t("nodeRegistrationDashboardOrigin")}>
                  <Input
                    type="url"
                    className="font-mono"
                    placeholder="https://dashboard.example.com"
                    value={draft.dashboard_origin}
                    onChange={(event) =>
                      setDraft({
                        ...draft,
                        dashboard_origin: event.target.value,
                      })
                    }
                  />
                </Field>
                <Hint>{t("nodeRegistrationDashboardOriginHelp")}</Hint>
                <Hint>{t("nodeKeyIdHelp")}</Hint>
              </>
            )}
            <Check
              checked={draft.atomic_transactions}
              onChange={(event) =>
                setDraft({
                  ...draft,
                  atomic_transactions: event.target.checked,
                })
              }
            >
              {t("nodeRegistrationAtomicHelp")}
            </Check>
            {error && <Alert>{t(error)}</Alert>}
            <div className="flex justify-end border-t border-border pt-4">
              <Button disabled={busy}>{t("nodeRegistrationSave")}</Button>
            </div>
          </form>
        </Modal>
      )}
    </>
  );
}
