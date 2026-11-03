import type {
  ReceivingKeyStatus as KeyStatus,
  RegistrationKeySelector,
} from "./generated/models";
import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { Textarea } from "./components/ui/textarea";
import {
  Alert,
  Check,
  Hint,
  Loading,
  Pager,
  RowList,
} from "./components/patterns";
import { tenantNodeRegistrations, tenantNodes } from "./generated/aidash";
import { ApiError, apiFetch } from "./transport";
import { Field, Modal, Panel, useI18n } from "./ui";

type Selector = RegistrationKeySelector;

/** Each opening reads both revisions. A conflict consumes consent, and never
 * retries the previous confirmation against a newly read crypto snapshot. */
export function ReceivingKeyControl({
  endpoint,
  selector,
  initialStatus,
}: {
  endpoint: string;
  selector: Selector;
  initialStatus?: KeyStatus;
}) {
  const { t } = useI18n();
  const client = useQueryClient();
  const [open, setOpen] = useState(initialStatus !== undefined);
  const [status, setStatus] = useState<KeyStatus | null>(initialStatus ?? null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [kid, setKid] = useState("");
  const [pem, setPem] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const read = () =>
    apiFetch<KeyStatus>(
      `${endpoint}?${new URLSearchParams(Object.entries(selector))}`,
    );
  const show = async () => {
    setOpen(true);
    setStatus(null);
    setError("");
    setKid("");
    setPem("");
    setConfirmed(false);
    setBusy(true);
    try {
      setStatus(await read());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const change = async (action: "quarantine" | "confirm") => {
    if (
      busy ||
      !status ||
      (action === "confirm" && (!confirmed || !kid.trim() || !pem.trim()))
    )
      return;
    setBusy(true);
    setError("");
    setConfirmed(false);
    try {
      setStatus(
        await apiFetch<KeyStatus>(endpoint, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            ...selector,
            action,
            expected_keys_revision: status.keys_revision,
            expected_authorization_revision: status.authorization_revision,
            ...(action === "confirm"
              ? { expected_key_id: kid.trim(), public_key_pem: pem.trim() }
              : {}),
          }),
        }),
      );
      await client.invalidateQueries({ queryKey: ["node-registrations"] });
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 409) {
        setStatus(null);
        try {
          setStatus(await read());
        } catch {
          /* Keep the stale form unusable. */
        }
        setError("receivingKeyConflict");
      } else {
        if (
          reason instanceof ApiError &&
          [401, 403, 404, 410].includes(reason.status)
        )
          setStatus(null);
        setError(reason instanceof Error ? reason.message : String(reason));
      }
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <Button variant="outline" size="sm" onClick={() => void show()}>
        {t("receivingKeyManage")}
      </Button>
      {open && (
        <Modal
          title={t("receivingKeyManage")}
          close={() => {
            if (!busy) setOpen(false);
          }}
        >
          <div className="grid min-w-0 gap-3">
            <Hint>{t("receivingKeyHelp")}</Hint>
            {!status && !error && <Loading />}
            {status && (
              <>
                <p className="break-all font-mono text-xs">{status.key_id}</p>
                <p className="text-sm">
                  {t(
                    status.quarantined
                      ? "receivingKeyQuarantined"
                      : "receivingKeyTrusted",
                  )}
                </p>
                {!status.enabled && <Hint>{t("receivingKeyDisabled")}</Hint>}
                <Button
                  variant="outline"
                  disabled={busy || status.quarantined}
                  onClick={() => void change("quarantine")}
                >
                  {t("receivingKeyQuarantine")}
                </Button>
                <Field label={t("receivingKeyExpected")}>
                  <Input
                    value={kid}
                    maxLength={128}
                    onChange={(event) => {
                      setKid(event.target.value);
                      setConfirmed(false);
                    }}
                  />
                </Field>
                <Field label={t("receivingKeyPublic")}>
                  <Textarea
                    value={pem}
                    maxLength={4096}
                    onChange={(event) => {
                      setPem(event.target.value);
                      setConfirmed(false);
                    }}
                  />
                </Field>
                <Check
                  checked={confirmed}
                  disabled={busy}
                  onChange={(event) => setConfirmed(event.target.checked)}
                >
                  {t("receivingKeyIndependent")}
                </Check>
                <Button
                  disabled={busy || !confirmed || !kid.trim() || !pem.trim()}
                  onClick={() => void change("confirm")}
                >
                  {t("receivingKeyConfirm")}
                </Button>
              </>
            )}
            {error && <Alert>{t(error)}</Alert>}
          </div>
        </Modal>
      )}
    </>
  );
}

/** Known immutable names address the existing control directly; catalog
 * enumeration and the ordinary live Node selector do not establish authority. */
export function KnownRegistrationKeyControls({ tenant }: { tenant: string }) {
  const { t } = useI18n();
  const [node, setNode] = useState("");
  const [source, setSource] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [selected, setSelected] = useState<{
    endpoint: string;
    source: string;
    status: KeyStatus;
  } | null>(null);
  const clear = () => {
    setSelected(null);
    setError("");
  };
  const lookup = async () => {
    if (busy || !node.trim() || !source.trim()) return;
    clear();
    setBusy(true);
    const endpoint = `/api/tenants/${encodeURIComponent(tenant)}/nodes/${encodeURIComponent(node.trim())}/registration-keys`;
    const sourceNode = source.trim();
    try {
      const status = await apiFetch<KeyStatus>(
        `${endpoint}?${new URLSearchParams({ source_node: sourceNode })}`,
      );
      setSelected({ endpoint, source: sourceNode, status });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Panel title={t("receivingKeyKnownPair")}>
      <Hint>{t("receivingKeyKnownHelp")}</Hint>
      <form
        className="grid min-w-0 gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          void lookup();
        }}
      >
        <Field label={t("receivingKeyReceiver")}>
          <Input
            value={node}
            required
            maxLength={128}
            disabled={busy}
            onChange={(event) => {
              clear();
              setNode(event.target.value);
            }}
          />
        </Field>
        <Field label={t("receivingKeySource")}>
          <Input
            value={source}
            required
            maxLength={4096}
            disabled={busy}
            onChange={(event) => {
              clear();
              setSource(event.target.value);
            }}
          />
        </Field>
        <Button type="submit" disabled={busy || !node.trim() || !source.trim()}>
          {t("receivingKeyLookup")}
        </Button>
      </form>
      {error && <Alert>{error}</Alert>}
      {selected && (
        <ReceivingKeyControl
          key={`${selected.endpoint}/${selected.source}`}
          endpoint={selected.endpoint}
          selector={{ source_node: selected.source }}
          initialStatus={selected.status}
        />
      )}
    </Panel>
  );
}

/** A separate management page preserves the ordinary live-only Node selector. */
export function DeletedNodeKeyControls({ tenant }: { tenant: string }) {
  const { t } = useI18n();
  const [offset, setOffset] = useState(0);
  const [selected, setSelected] = useState("");
  const nodes = useQuery({
    queryKey: ["deleted-node-keys", tenant, offset],
    queryFn: () =>
      tenantNodes(tenant, { include_deleted: true, limit: 100, offset }),
  });
  const deleted =
    nodes.data?.filter(
      (node) => node.state === "deleted" || node.state === "deleting",
    ) ?? [];
  const selectedNode = deleted.find((node) => node.name === selected);
  // Restoration or a changed page must discard the old record's controls.
  if (selected && nodes.isSuccess && !selectedNode) setSelected("");
  return (
    <Panel title={t("receivingKeyDeletedNodes")}>
      <Hint>{t("receivingKeyDeletedHelp")}</Hint>
      {nodes.isError && <Alert>{nodes.error.message}</Alert>}
      <RowList>
        {deleted.map((node) => (
          <div
            key={node.name}
            className="flex items-center justify-between gap-3 py-2"
          >
            <span>{node.label}</span>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setSelected(node.name)}
            >
              {t("receivingKeyReview")}
            </Button>
          </div>
        ))}
      </RowList>
      <Pager
        page={offset / 100 + 1}
        previous={{
          disabled: offset === 0 || nodes.isFetching,
          go: () => {
            setOffset(Math.max(0, offset - 100));
            setSelected("");
          },
        }}
        next={{
          disabled: !nodes.data || nodes.data.length < 100 || nodes.isFetching,
          go: () => {
            setOffset(offset + 100);
            setSelected("");
          },
        }}
      />
      {selectedNode && (
        <DeletedRegistrationKeys
          key={selectedNode.name}
          tenant={tenant}
          node={selectedNode.name}
        />
      )}
    </Panel>
  );
}
function DeletedRegistrationKeys({
  tenant,
  node,
}: {
  tenant: string;
  node: string;
}) {
  const [offset, setOffset] = useState(0);
  const records = useQuery({
    queryKey: ["node-registrations", tenant, node, "deleted-keys", offset],
    queryFn: () => tenantNodeRegistrations(tenant, node, { limit: 25, offset }),
  });
  return (
    <>
      {records.isError && <Alert>{records.error.message}</Alert>}
      <RowList>
        {records.data
          ?.filter((row) => row.key_id)
          .map((row) => (
            <div
              key={row.source_node}
              className="flex min-w-0 items-center justify-between gap-3 py-2"
            >
              <span className="break-all font-mono text-xs">
                {row.source_node}
              </span>
              <ReceivingKeyControl
                endpoint={`/api/tenants/${encodeURIComponent(tenant)}/nodes/${encodeURIComponent(node)}/registration-keys`}
                selector={{ source_node: row.source_node }}
              />
            </div>
          ))}
      </RowList>
      <Pager
        page={offset / 25 + 1}
        previous={{
          disabled: offset === 0 || records.isFetching,
          go: () => setOffset(Math.max(0, offset - 25)),
        }}
        next={{
          disabled:
            !records.data || records.data.length < 25 || records.isFetching,
          go: () => setOffset(offset + 25),
        }}
      />
    </>
  );
}
