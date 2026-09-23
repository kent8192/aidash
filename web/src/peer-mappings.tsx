import { ReferenceName, PeerSelect } from "./record-view";
import { RecordView } from "./record-view";
import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  authorizationPeerMappings,
  authorizationPeerMappingHistory,
  authorizationSetPeerMapping,
} from "./generated/aidash";
import type {
  Credential,
  PeerMapping,
  PeerMappingInput,
} from "./generated/models";
import { ApiError } from "./transport";
import { Badge, Field, Modal, Panel, useI18n } from "./ui";

const PAGE_SIZE = 25;
export function PeerMappings({
  tenant,
  credentials,
}: {
  tenant: string;
  credentials: Credential[];
}) {
  const { t } = useI18n();
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
          <button
            onClick={() => {
              setError("");
              setEditing("new");
            }}
          >
            {t("authAddPeerMapping")}
          </button>
        }
      >
        <p className="auth-padding">{t("authPeerMappingHelp")}</p>
        {error && !editing && (
          <p role="alert" className="error">
            {t(error)}
          </p>
        )}
        {mappings.isError && (
          <p role="alert" className="error">
            {mappings.error.message}
          </p>
        )}
        {mappings.isPending && <p className="auth-padding">{t("loading")}</p>}
        {rows?.length === 0 && (
          <p className="auth-padding">{t("authNoPeerMappings")}</p>
        )}
        {rows?.map((mapping) => (
          <div
            className="auth-row"
            key={JSON.stringify([
              mapping.source_node,
              mapping.source_tenant,
              mapping.source_subject,
            ])}
          >
            <div>
              <strong>
                <ReferenceName id={mapping.source_node} />
              </strong>
              <small>
                {mapping.source_tenant} / {mapping.source_subject}
              </small>
              <small>
                {t("authMappedCredential")}:{" "}
                {credentials.find(
                  (credential) => credential.id === mapping.credential_id,
                )?.subject || t("unavailableEntity")}
              </small>
              <small>
                {t("revision")}: {mapping.revision}
              </small>
            </div>
            <Badge value={mapping.enabled ? "authApproved" : "authDisabled"} />
            <button
              disabled={busy}
              onClick={() => {
                setError("");
                setEditing(mapping);
              }}
            >
              {t("authEditPeerMapping")}
            </button>
            <button
              disabled={busy}
              onClick={() =>
                void save({
                  expected_revision: mapping.revision,
                  enabled: !mapping.enabled,
                  source_node: mapping.source_node,
                  source_tenant: mapping.source_tenant,
                  source_subject: mapping.source_subject,
                  credential_id: mapping.credential_id,
                })
              }
            >
              {t(mapping.enabled ? "authDisableApproval" : "authApprove")}
            </button>
          </div>
        ))}
        <div className="auth-pagination">
          <button
            disabled={offset === 0 || mappings.isFetching}
            onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
          >
            {t("authPrevious")}
          </button>
          <span>
            {t("authPage")} {offset / PAGE_SIZE + 1}
          </span>
          <button
            disabled={!rows || rows.length < PAGE_SIZE || mappings.isFetching}
            onClick={() => setOffset(offset + PAGE_SIZE)}
          >
            {t("authNext")}
          </button>
        </div>
      </Panel>
      <Panel title={t("authPeerMappingHistory")}>
        {history.isError && (
          <p role="alert" className="error">
            {history.error.message}
          </p>
        )}
        {history.isPending && <p className="auth-padding">{t("loading")}</p>}
        {revisions?.length === 0 && (
          <p className="auth-padding">{t("authNoHistory")}</p>
        )}
        {revisions?.map((revision) => (
          <details className="auth-history" key={revision.sequence}>
            <summary>
              <ReferenceName id={revision.source_node} /> ·{" "}
              {revision.source_tenant} / {revision.source_subject}
              <time>{revision.updated_at}</time>
            </summary>
            <RecordView
              value={revision}
              labels={
                new Map(
                  credentials.map((credential) => [
                    credential.id,
                    `${credential.subject} · ${new Date(credential.created_at).toLocaleString()}`,
                  ]),
                )
              }
            />
          </details>
        ))}
        <div className="auth-pagination">
          <button
            disabled={cursors.length === 1 || history.isFetching}
            onClick={() => setCursors(cursors.slice(0, -1))}
          >
            {t("authPrevious")}
          </button>
          <span>
            {t("authPage")} {cursors.length}
          </span>
          <button
            disabled={
              !revisions || revisions.length < PAGE_SIZE || history.isFetching
            }
            onClick={() => {
              const next = revisions?.at(-1)?.sequence;
              if (next !== undefined) setCursors([...cursors, next]);
            }}
          >
            {t("authNext")}
          </button>
        </div>
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
            className="form-grid"
            onSubmit={(event) => {
              event.preventDefault();
              const form = new FormData(event.currentTarget);
              void save({
                source_node: String(form.get("source_node")).trim(),
                source_tenant: String(form.get("source_tenant")).trim(),
                source_subject: String(form.get("source_subject")).trim(),
                credential_id: String(form.get("credential_id")),
                enabled: true,
                expected_revision: editing === "new" ? 0 : editing.revision,
              });
            }}
          >
            <Field label={t("authSourceNode")}>
              <PeerSelect
                name="source_node"
                initial={editing === "new" ? "" : editing.source_node}
                disabled={editing !== "new"}
              />
            </Field>
            {editing !== "new" && (
              <input
                type="hidden"
                name="source_node"
                value={editing.source_node}
              />
            )}
            {(
              [
                ["source_tenant", "authSourceTenant"],
                ["source_subject", "authSourceSubject"],
              ] as const
            ).map(([name, label]) => (
              <Field key={name} label={t(label)}>
                <input
                  name={name}
                  required
                  maxLength={256}
                  readOnly={editing !== "new"}
                  defaultValue={editing === "new" ? "" : editing[name]}
                />
              </Field>
            ))}
            <Field label={t("authMappedCredential")}>
              <select
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
                    {credential.subject} ·{" "}
                    {new Date(credential.created_at).toLocaleString()}
                  </option>
                ))}
              </select>
            </Field>
            <p>{t("authPeerMappingSaveHelp")}</p>
            {error && (
              <p role="alert" className="error">
                {t(error)}
              </p>
            )}
            <button disabled={busy}>{t("authSavePeerMapping")}</button>
          </form>
        </Modal>
      )}
    </>
  );
}
