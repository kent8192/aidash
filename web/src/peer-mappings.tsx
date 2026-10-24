import { Button } from "./components/ui/button";
import { disambiguateLabels } from "./display-labels";
import { RecordView } from "./record-view";
import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  authorizationPeerMappings,
  authorizationPeerMappingHistory,
  authorizationSetPeerMapping,
  tenantNodeRegistrations,
} from "./generated/aidash";
import type {
  Credential,
  PeerMapping,
  PeerMappingInput,
} from "./generated/models";
import { ApiError } from "./transport";
import { nodeLabel } from "./node-selection";
import { useNodes, useTenantNodes } from "./nodes";
import { Badge, Field, Modal, Panel, useI18n } from "./ui";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import {
  Alert,
  Hint,
  HistoryRow,
  Loading,
  Pager,
  RowList,
} from "./components/patterns";

const PAGE_SIZE = 25;

/**
 * The receiving Node and the Node whose subject is mapped. A new mapping picks
 * the receiving Node among the Tenant's and the source among the Nodes
 * registered at it, because a mapping cannot precede its Node Registration.
 * Only when the registrations cannot be read is the source typed.
 */
function NodeFields({
  tenant,
  editing,
}: {
  tenant: string;
  editing: PeerMapping | "new";
}) {
  const { t } = useI18n();
  // A Deployment Administrator opens any Tenant and has none of its own, so
  // the Nodes are those of the Tenant whose mappings these are.
  const list = useTenantNodes(tenant);
  const nodes = list.isError ? [] : (list.data ?? []);
  const own = useNodes();
  const selected =
    own.tenant === tenant ? own.selected : nodes.find((value) => value.default);
  const creating = editing === "new";
  const [chosen, setChosen] = useState<string>();
  const node = chosen ?? (creating ? selected?.name : editing.node) ?? "";
  const registrations = useQuery({
    queryKey: ["node-registrations", tenant, node, "choices"],
    queryFn: () =>
      tenantNodeRegistrations(tenant, node, { offset: 0, limit: 200 }),
    enabled: creating && node !== "",
    retry: false,
  });
  const sources = registrations.isError
    ? undefined
    : registrations.data?.filter((registration) => registration.enabled);
  return (
    <>
      <Field label={t("authReceivingNode")}>
        {creating ? (
          <NativeSelect
            name="node"
            required
            value={node}
            onChange={(event) => setChosen(event.target.value)}
          >
            {nodes.map((value) => (
              <option key={value.name} value={value.name}>
                {value.label}
              </option>
            ))}
          </NativeSelect>
        ) : (
          <Input
            readOnly
            value={nodeLabel(nodes, node)}
            className="read-only:bg-raised read-only:text-muted-foreground"
          />
        )}
      </Field>
      {!creating && <input type="hidden" name="node" value={node} />}
      <Field label={t("authSourceNode")}>
        {creating && sources ? (
          <NativeSelect name="source_node" required defaultValue="">
            <option value="">{t("choose")}</option>
            {sources.map((registration) => (
              <option
                key={registration.source_node}
                value={registration.source_node}
              >
                {registration.source_node}
              </option>
            ))}
          </NativeSelect>
        ) : (
          <Input
            name="source_node"
            required
            maxLength={256}
            readOnly={!creating}
            className="font-mono read-only:bg-raised read-only:text-muted-foreground"
            defaultValue={creating ? "" : editing.source_node}
          />
        )}
      </Field>
    </>
  );
}

export function PeerMappings({
  tenant,
  credentials,
  editable = true,
}: {
  tenant: string;
  credentials: Credential[];
  /** Whether the caller may create, change and disable mappings. */
  editable?: boolean;
}) {
  const { t, locale } = useI18n();
  const list = useTenantNodes(tenant);
  const nodes = list.isError ? [] : (list.data ?? []);
  const credentialLabels = disambiguateLabels(
    credentials,
    (credential) => credential.id,
    (credential) =>
      `${credential.subject} · ${new Date(credential.created_at).toLocaleString()}`,
  );
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  const client = useQueryClient();
  const path = encodeURIComponent(tenant);
  const [offset, setOffset] = useState(0);
  const [cursors, setCursors] = useState([0]);
  const [editing, setEditing] = useState<PeerMapping | "new" | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const after = cursors[cursors.length - 1];
  const mappings = useQuery({
    queryKey: ["authorization", tenant, "peer-mappings", offset],
    queryFn: () =>
      authorizationPeerMappings(path, { offset, limit: PAGE_SIZE }),
    refetchInterval: 5000,
  });
  const history = useQuery({
    queryKey: ["authorization", tenant, "peer-mapping-history", after],
    queryFn: () =>
      authorizationPeerMappingHistory(path, { after, limit: PAGE_SIZE }),
    refetchInterval: 5000,
  });
  const rows = !mappings.isError ? mappings.data : undefined;
  const revisions = !history.isError ? history.data : undefined;
  const save = async (input: PeerMappingInput) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await authorizationSetPeerMapping(path, input);
      setEditing(null);
      await client.invalidateQueries({ queryKey: ["authorization", tenant] });
    } catch (reason) {
      setError(
        reason instanceof ApiError && reason.status === 409
          ? "authConflict"
          : reason instanceof ApiError && reason.status === 403
            ? "authPeerMappingDenied"
            : reason instanceof Error
              ? reason.message
              : String(reason),
      );
      await client.invalidateQueries({
        queryKey: ["authorization", tenant, "peer-mappings"],
      });
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <Panel
        title={t("authPeerMappings")}
        action={
          editable && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setError("");
                setEditing("new");
              }}
            >
              {t("authAddPeerMapping")}
            </Button>
          )
        }
      >
        <Hint>{t("authPeerMappingHelp")}</Hint>
        {error && !editing && <Alert>{t(error)}</Alert>}
        {mappings.isError && <Alert>{mappings.error.message}</Alert>}
        {mappings.isPending && <Loading />}
        {rows?.length === 0 && (
          <p className="text-xs text-muted-foreground">
            {t("authNoPeerMappings")}
          </p>
        )}
        {!!rows?.length && (
          <RowList>
            {rows.map((mapping) => (
              <div
                className="auth-row grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-2 py-2 md:grid-cols-[minmax(0,1fr)_auto_auto]"
                key={JSON.stringify([
                  mapping.node,
                  mapping.source_node,
                  mapping.source_subject,
                ])}
              >
                <div className="grid min-w-0 gap-0.5">
                  <span className="truncate font-mono font-medium text-foreground">
                    {mapping.source_node}
                  </span>
                  <span className="truncate font-mono text-[11px] text-muted-foreground">
                    {nodeLabel(nodes, mapping.node)} / {mapping.source_subject}
                  </span>
                  <span className="truncate text-[11px] text-faint">
                    {t("authMappedCredential")}{" "}
                    <span className="font-mono text-muted-foreground">
                      {credentials.find(
                        (credential) => credential.id === mapping.credential_id,
                      )?.subject || t("unavailableEntity")}
                    </span>
                    {" · "}
                    {t("revision")}{" "}
                    <span className="font-mono tabular">
                      {mapping.revision}
                    </span>
                  </span>
                </div>
                <Badge
                  value={mapping.enabled ? "authApproved" : "authDisabled"}
                  tone={mapping.enabled ? "success" : "neutral"}
                />
                {editable && (
                  <div className="col-span-2 flex flex-wrap gap-1.5 md:col-span-1">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy}
                      onClick={() => {
                        setError("");
                        setEditing(mapping);
                      }}
                    >
                      {t("authEditPeerMapping")}
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={busy}
                      onClick={() =>
                        void save({
                          expected_revision: mapping.revision,
                          enabled: !mapping.enabled,
                          node: mapping.node,
                          source_node: mapping.source_node,
                          source_subject: mapping.source_subject,
                          credential_id: mapping.credential_id,
                        })
                      }
                    >
                      {t(
                        mapping.enabled ? "authDisableApproval" : "authApprove",
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
            disabled: offset === 0 || mappings.isFetching,
            go: () => setOffset(Math.max(0, offset - PAGE_SIZE)),
          }}
          next={{
            disabled: !rows || rows.length < PAGE_SIZE || mappings.isFetching,
            go: () => setOffset(offset + PAGE_SIZE),
          }}
        />
      </Panel>
      <Panel title={t("authPeerMappingHistory")}>
        {history.isError && <Alert>{history.error.message}</Alert>}
        {history.isPending && <Loading />}
        {revisions?.length === 0 && (
          <p className="text-xs text-muted-foreground">{t("authNoHistory")}</p>
        )}
        {!!revisions?.length && (
          <RowList>
            {revisions.map((revision) => (
              <HistoryRow
                className="auth-history"
                key={revision.sequence}
                time={new Date(revision.updated_at).toLocaleString(locale)}
                summary={
                  <>
                    <span className="font-mono">{revision.source_node}</span>
                    <span className="font-mono text-muted-foreground">
                      {" · "}
                      {nodeLabel(nodes, revision.node)} /{" "}
                      {revision.source_subject}
                    </span>
                  </>
                }
              >
                <RecordView
                  value={revision}
                  labels={
                    new Map(
                      credentials.map((credential) => [
                        credential.id,
                        credentialLabels.get(credential.id) ??
                          credential.subject,
                      ]),
                    )
                  }
                />
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
      {editing && (
        <Modal
          title={t(
            editing === "new" ? "authAddPeerMapping" : "authEditPeerMapping",
          )}
          close={() => {
            if (!busy) {
              setEditing(null);
              setError("");
            }
          }}
        >
          <form
            className="grid min-w-0 gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              const form = new FormData(event.currentTarget);
              void save({
                node: String(form.get("node")).trim(),
                source_node: String(form.get("source_node")).trim(),
                source_subject: String(form.get("source_subject")).trim(),
                credential_id: String(form.get("credential_id")),
                enabled: true,
                expected_revision: editing === "new" ? 0 : editing.revision,
              });
            }}
          >
            <div className="grid gap-3">
              <NodeFields tenant={tenant} editing={editing} />
              <Field label={t("authSourceSubject")}>
                <Input
                  name="source_subject"
                  required
                  maxLength={256}
                  readOnly={editing !== "new"}
                  className="font-mono read-only:bg-raised read-only:text-muted-foreground"
                  defaultValue={editing === "new" ? "" : editing.source_subject}
                />
              </Field>
            </div>
            <Field label={t("authMappedCredential")}>
              <NativeSelect
                name="credential_id"
                required
                defaultValue={editing === "new" ? "" : editing.credential_id}
              >
                <option value="">{t("authSelectCredential")}</option>
                {credentials.map((credential) => (
                  <option
                    key={credential.id}
                    value={credential.id}
                    disabled={
                      !!credential.revoked_at ||
                      Date.parse(credential.expires_at) <= now
                    }
                  >
                    {credentialLabels.get(credential.id)}
                  </option>
                ))}
              </NativeSelect>
            </Field>
            <Hint>{t("authPeerMappingSaveHelp")}</Hint>
            {error && <Alert>{t(error)}</Alert>}
            <div className="flex justify-end border-t border-border pt-4">
              <Button disabled={busy}>{t("authSavePeerMapping")}</Button>
            </div>
          </form>
        </Modal>
      )}
    </>
  );
}
