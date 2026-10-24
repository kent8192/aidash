import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Alert, Hint, Loading, Pager, RowList } from "./components/patterns";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import {
  tenantNodeDefault,
  tenantNodeDeletion,
  tenantNodePurge,
  tenantNodeRestore,
  tenantNodes,
} from "./generated/aidash";
import type { NodeRecord } from "./generated/models";
import { useTenantNodes } from "./nodes";
import { ApiError } from "./transport";
import { Badge, Field, Modal, Panel, useI18n } from "./ui";

const PAGE_SIZE = 25;

type Confirmation = { node: NodeRecord; action: "delete" | "purge" };

/** Bounded lifecycle management has its own cache; selectors keep the complete
 * live list. Deployment Administrators pass the Tenant they explicitly opened. */
export function NodeLifecycleSettings({
  tenant,
  canUpdate,
  canDelete,
  deploymentAdministrator = false,
}: {
  tenant: string;
  canUpdate: boolean;
  canDelete: boolean;
  deploymentAdministrator?: boolean;
}) {
  const { t, locale } = useI18n();
  const client = useQueryClient();
  const live = useTenantNodes(tenant);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 5000);
    return () => window.clearInterval(timer);
  }, []);
  const [offset, setOffset] = useState(0);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const path = encodeURIComponent(tenant);
  const management = useQuery({
    queryKey: ["node-lifecycle", tenant, offset],
    queryFn: () =>
      tenantNodes(path, { include_deleted: true, offset, limit: PAGE_SIZE }),
    enabled: canDelete,
    refetchInterval: 5000,
    retry: (count, reason) =>
      count < 1 && !(reason instanceof ApiError && reason.status < 500),
  });
  const rows = canDelete
    ? management.isError
      ? undefined
      : management.data
    : live.isError
      ? undefined
      : live.data;
  const liveCount = live.isError
    ? 0
    : (live.data?.filter((node) => node.state === "live").length ?? 0);
  const act = async (action: () => Promise<NodeRecord>, restoring = false) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
      setConfirmation(null);
      setTyped("");
    } catch (reason) {
      setError(
        reason instanceof ApiError && reason.status === 409
          ? restoring
            ? "nodeRestoreConflict"
            : "nodeLifecycleConflict"
          : reason instanceof ApiError && reason.status === 403
            ? "nodeDenied"
            : reason instanceof Error
              ? reason.message
              : String(reason),
      );
    } finally {
      await Promise.all([
        client.invalidateQueries({ queryKey: ["nodes", tenant] }),
        client.invalidateQueries({ queryKey: ["node-lifecycle", tenant] }),
        client.invalidateQueries({ queryKey: ["deleted-node-keys", tenant] }),
        client.invalidateQueries({ queryKey: ["state"] }),
      ]);
      setBusy(false);
    }
  };
  const confirm = (node: NodeRecord, action: Confirmation["action"]) => {
    setTyped("");
    setError("");
    setConfirmation({ node, action });
  };
  return (
    <Panel title={t("nodeLifecycle")}>
      <Hint>{t("nodeLifecycleHelp")}</Hint>
      {error && !confirmation && <Alert>{t(error)}</Alert>}
      {canDelete && management.isError && (
        <Alert>{management.error.message}</Alert>
      )}
      {!canDelete && live.isError && <Alert>{live.error.message}</Alert>}
      {(canDelete ? management.isPending : live.isPending) && <Loading />}
      {rows?.length === 0 && <Hint>{t("nodeLifecycleEmpty")}</Hint>}
      {!!rows?.length && (
        <RowList>
          {rows.map((node) => {
            const deletion = node.deletion;
            const operatorDeletion =
              deletion?.deleted_by_kind === "deployment_administrator";
            const restorable =
              node.state === "deleted" &&
              deletion &&
              new Date(deletion.purge_after).getTime() > now &&
              (!operatorDeletion || deploymentAdministrator);
            return (
              <div
                key={node.name}
                data-lifecycle-node={node.name}
                className="grid min-w-0 gap-2 py-3"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span className="flex items-center gap-2 font-medium text-foreground">
                    {node.label}
                    <Badge value={`nodeState_${node.state}`} />
                    {node.default && node.state === "live" && (
                      <Badge value="nodeDefault" tone="brand" />
                    )}
                  </span>
                  <div className="flex flex-wrap gap-1.5">
                    {node.state === "live" && !node.default && canUpdate && (
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={busy}
                        onClick={() =>
                          void act(() =>
                            tenantNodeDefault(
                              path,
                              encodeURIComponent(node.name),
                            ),
                          )
                        }
                      >
                        {t("nodeMakeDefault")}
                      </Button>
                    )}
                    {node.state === "live" &&
                      !node.default &&
                      liveCount > 1 &&
                      canDelete && (
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={busy}
                          onClick={() => confirm(node, "delete")}
                        >
                          {t("nodeDelete")}
                        </Button>
                      )}
                    {canDelete && restorable && (
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={busy}
                        onClick={() =>
                          void act(
                            () =>
                              tenantNodeRestore(
                                path,
                                encodeURIComponent(node.name),
                              ),
                            true,
                          )
                        }
                      >
                        {t("nodeRestore")}
                      </Button>
                    )}
                    {deploymentAdministrator && node.state === "deleted" && (
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={busy}
                        onClick={() => confirm(node, "purge")}
                      >
                        {t("nodePurgeNow")}
                      </Button>
                    )}
                  </div>
                </div>
                <span className="truncate font-mono text-[11px] text-muted-foreground">
                  {node.address}
                </span>
                {deletion && (
                  <div className="grid gap-1 text-xs text-muted-foreground">
                    <span>
                      {t("nodeDeletedAt")}{" "}
                      {new Date(deletion.deleted_at).toLocaleString(locale)}
                    </span>
                    <span>
                      {t("nodePurgeDeadline")}{" "}
                      {new Date(deletion.purge_after).toLocaleString(locale)}
                    </span>
                    {node.state === "purged" && deletion.purged_at && (
                      <span>
                        {t("nodePurgedAt")}{" "}
                        {new Date(deletion.purged_at).toLocaleString(locale)}
                      </span>
                    )}
                    {operatorDeletion &&
                      !deploymentAdministrator &&
                      node.state === "deleted" && (
                        <Hint>{t("nodeOperatorRestore")}</Hint>
                      )}
                    {node.state === "deleted" &&
                      !restorable &&
                      !operatorDeletion && (
                        <Hint>{t("nodeRestoreExpired")}</Hint>
                      )}
                    {deletion.purge_step && (
                      <span>
                        {t("nodePurgeProgress")}{" "}
                        {t(`nodePurgeStep_${deletion.purge_step}`)}
                      </span>
                    )}
                    {deletion.purge_failures > 0 && (
                      <Alert>
                        {t("nodePurgeFailing")} {deletion.purge_failures}
                        {deletion.purge_failed_at && (
                          <>
                            {" "}
                            ·{" "}
                            {new Date(deletion.purge_failed_at).toLocaleString(
                              locale,
                            )}
                          </>
                        )}
                        {deletion.purge_error && <> · {deletion.purge_error}</>}
                      </Alert>
                    )}
                  </div>
                )}
                {node.storage_retained_until_purge && (
                  <Hint>{t("nodeStorageNotice")}</Hint>
                )}
              </div>
            );
          })}
        </RowList>
      )}
      {canDelete && (
        <Pager
          page={offset / PAGE_SIZE + 1}
          previous={{
            disabled: offset === 0 || management.isFetching || busy,
            go: () => setOffset(Math.max(0, offset - PAGE_SIZE)),
          }}
          next={{
            disabled:
              !rows || rows.length < PAGE_SIZE || management.isFetching || busy,
            go: () => setOffset(offset + PAGE_SIZE),
          }}
        />
      )}
      {confirmation && (
        <Modal
          title={t(
            confirmation.action === "delete"
              ? "nodeDelete"
              : "nodePurgeConfirm",
          )}
          close={() => {
            if (!busy) setConfirmation(null);
          }}
        >
          <form
            className="grid gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              if (typed !== confirmation.node.name) return;
              const name = encodeURIComponent(confirmation.node.name);
              void act(() =>
                confirmation.action === "delete"
                  ? tenantNodeDeletion(path, name, { confirmation: typed })
                  : tenantNodePurge(path, name),
              );
            }}
          >
            <span className="font-mono text-xs">
              {confirmation.node.address}
            </span>
            <Hint>
              {t(
                confirmation.action === "delete"
                  ? "nodeDeleteNotice"
                  : "nodePurgeNotice",
              )}
            </Hint>
            <Hint>{t("nodeStorageNotice")}</Hint>
            <Field label={t("nodeConfirmName")}>
              <Input
                value={typed}
                onChange={(event) => setTyped(event.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
            </Field>
            {error && <Alert>{t(error)}</Alert>}
            <div className="flex justify-end border-t border-border pt-4">
              <Button
                variant="destructive"
                disabled={busy || typed !== confirmation.node.name}
              >
                {t(
                  confirmation.action === "delete"
                    ? "nodeDelete"
                    : "nodePurgeConfirm",
                )}
              </Button>
            </div>
          </form>
        </Modal>
      )}
    </Panel>
  );
}
