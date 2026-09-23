import { useState } from "react";
import { PeerMappings } from "./peer-mappings";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  authorizationSnapshot,
  authorizationReplace,
  authorizationSimulate,
  authorizationEvaluate,
  authorizationRevisions,
  authorizationDecisions,
  authorizationCredentials,
  authorizationIssueCredential,
  authorizationRevokeCredential,
  authorizationCatalog,
  authorizationSetCatalog,
} from "./generated/aidash";
import type {
  Binding,
  Credential,
  Decision,
  Entry,
  Evaluation,
  IssuedCredential,
  PolicyBundle,
  Snapshot,
} from "./generated/models";
import { ApiError } from "./transport";
import {
  Badge,
  Empty,
  Field,
  JsonView,
  Modal,
  Panel,
  useI18n,
  useEntryLabel,
} from "./ui";

const PAGE_SIZE = 25;
const object = (value: unknown): Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
const jsonObject = (text: string) => {
  const value: unknown = JSON.parse(text);
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new Error("authJsonObject");
  return value as Record<string, unknown>;
};
const pretty = (value: unknown) => JSON.stringify(value, null, 2);

export function AuthorizationPage({ entries }: { entries: Entry[] }) {
  const { t } = useI18n();
  const [tenant, setTenant] = useState("");
  return (
    <div className="authorization-page">
      <p className="notice">{t("authorizationHelp")}</p>
      <form
        className="generation-tenant"
        onSubmit={(event) => {
          event.preventDefault();
          setTenant(
            String(new FormData(event.currentTarget).get("tenant")).trim(),
          );
        }}
      >
        <Field label={t("tenant")}>
          <input name="tenant" required maxLength={256} />
        </Field>
        <button>{t("open")}</button>
      </form>
      {tenant && (
        <TenantAuthorization key={tenant} tenant={tenant} entries={entries} />
      )}
    </div>
  );
}

function TenantAuthorization({
  tenant,
  entries,
}: {
  tenant: string;
  entries: Entry[];
}) {
  const { t } = useI18n();
  const entryLabel = useEntryLabel(entries);
  const client = useQueryClient();
  const path = encodeURIComponent(tenant);
  const [editor, setEditor] = useState<Snapshot | null>(null);
  const [issuing, setIssuing] = useState(false);
  const [issued, setIssued] = useState<IssuedCredential | null>(null);
  const [revoking, setRevoking] = useState<Credential | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const snapshot = useQuery({
    queryKey: ["authorization", tenant, "snapshot"],
    queryFn: () => authorizationSnapshot(path),
    retry: false,
    refetchInterval: 5000,
  });
  const missing =
    snapshot.error instanceof ApiError && snapshot.error.status === 404;
  const current = !snapshot.isError ? snapshot.data : undefined;
  const credentials = useQuery({
    queryKey: ["authorization", tenant, "credentials"],
    queryFn: async () => {
      const all: Credential[] = [];
      for (let offset = 0; ; offset += 200) {
        const page = await authorizationCredentials(path, { offset });
        all.push(...page);
        if (page.length < 200) return all;
      }
    },
    enabled: !!current,
    refetchInterval: 5000,
  });
  const catalog = useQuery({
    queryKey: ["authorization", tenant, "catalog"],
    queryFn: () => authorizationCatalog(path),
    enabled: !!current,
    refetchInterval: 5000,
  });
  const message = (reason: unknown) =>
    reason instanceof ApiError && reason.status === 409
      ? t("authConflict")
      : reason instanceof Error
        ? t(reason.message)
        : String(reason);
  const mutate = async (action: () => Promise<unknown>) => {
    if (busy) return false;
    setBusy(true);
    setError("");
    try {
      await action();
      await client.invalidateQueries();
      return true;
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 409) {
        await client.invalidateQueries({ queryKey: ["authorization", tenant] });
      }
      setError(message(reason));
      return false;
    } finally {
      setBusy(false);
    }
  };
  const close = () => {
    if (!busy) {
      setEditor(null);
      setIssuing(false);
      setRevoking(null);
      setError("");
    }
  };
  const modal = !!editor || issuing || !!revoking;
  return (
    <>
      <p className="muted">
        {t("tenant")}: <strong>{tenant}</strong>
      </p>
      {error && !modal && (
        <p role="alert" className="error">
          {error}
        </p>
      )}
      {snapshot.isPending && <p>{t("loading")}</p>}
      {snapshot.isError && !missing && (
        <p role="alert" className="error">
          {snapshot.error.message}
          <button onClick={() => void snapshot.refetch()}>{t("retry")}</button>
        </p>
      )}
      {(current || missing) && (
        <Panel
          title={t("authPolicyBundle")}
          action={
            <button
              onClick={() => {
                setError("");
                setEditor(
                  current ?? {
                    revision: 0,
                    bundle: {
                      tenant,
                      subjects: {},
                      groups: {},
                      roles: {},
                      policies: [],
                    },
                  },
                );
              }}
            >
              {t(current ? "authEditPolicy" : "authCreatePolicy")}
            </button>
          }
        >
          {current ? (
            <div className="auth-padding">
              <BundleSummary snapshot={current} />
              <details>
                <summary>{t("authCurrentDocument")}</summary>
                <JsonView value={current.bundle} />
              </details>
            </div>
          ) : (
            <p className="auth-padding">{t("authMissingPolicy")}</p>
          )}
        </Panel>
      )}
      {current && (
        <>
          <EvaluationPanel
            key={current.revision}
            tenant={tenant}
            snapshot={current}
          />
          <Panel
            title={t("authCredentials")}
            action={
              <button
                onClick={() => {
                  setError("");
                  setIssued(null);
                  setIssuing(true);
                }}
              >
                {t("authIssueCredential")}
              </button>
            }
          >
            <p className="auth-padding muted">{t("authCredentialsHelp")}</p>
            {credentials.isError && (
              <p role="alert" className="error">
                {credentials.error.message}
              </p>
            )}
            {credentials.isPending && (
              <p className="auth-padding">{t("loading")}</p>
            )}
            {!credentials.isError && credentials.data?.length === 0 && (
              <Empty />
            )}
            {!credentials.isError &&
              credentials.data?.map((credential) => (
                <div className="auth-row" key={credential.id}>
                  <div>
                    <strong>{credential.subject}</strong>
                    <small>
                      {new Date(credential.created_at).toLocaleString()}
                    </small>
                    <small>
                      {t("authExpires")}:{" "}
                      {new Date(credential.expires_at).toLocaleString()}
                    </small>
                  </div>
                  <Badge
                    value={
                      credential.revoked_at
                        ? "authRevoked"
                        : Date.parse(credential.expires_at) <=
                            credentials.dataUpdatedAt
                          ? "authExpired"
                          : "authActive"
                    }
                  />
                  {!credential.revoked_at && (
                    <button
                      disabled={busy}
                      onClick={() => {
                        setError("");
                        setRevoking(credential);
                      }}
                    >
                      {t("authRevoke")}
                    </button>
                  )}
                </div>
              ))}
          </Panel>
          <Panel title={t("authCatalog")}>
            <p className="auth-padding">{t("authCatalogHelp")}</p>
            {catalog.isError ? (
              <p role="alert" className="error">
                {catalog.error.message}
              </p>
            ) : catalog.data ? (
              <>
                <CatalogForm
                  entries={entries}
                  bindings={catalog.data}
                  busy={busy}
                  save={(binding) =>
                    mutate(() => authorizationSetCatalog(path, binding))
                  }
                />
                {catalog.data.map((binding) => (
                  <div
                    className="auth-row"
                    key={`${binding.entry_id}@${binding.entry_version}`}
                  >
                    <div>
                      <strong>
                        {entryLabel({
                          id: binding.entry_id,
                          version: binding.entry_version,
                        })}
                      </strong>
                      <small>
                        {t("revision")}: {binding.revision}
                      </small>
                    </div>
                    <Badge
                      value={binding.enabled ? "authApproved" : "authDisabled"}
                    />
                    <button
                      disabled={busy}
                      onClick={() =>
                        void mutate(() =>
                          authorizationSetCatalog(path, {
                            entry: {
                              id: binding.entry_id,
                              version: binding.entry_version,
                            },
                            expected_revision: binding.revision,
                            enabled: !binding.enabled,
                          }),
                        )
                      }
                    >
                      {t(
                        binding.enabled ? "authDisableApproval" : "authApprove",
                      )}
                    </button>
                  </div>
                ))}
              </>
            ) : (
              <p className="auth-padding">{t("loading")}</p>
            )}
          </Panel>
          <PeerMappings
            tenant={tenant}
            credentials={!credentials.isError ? (credentials.data ?? []) : []}
          />
          <HistoryPanel
            key={`${tenant}-revisions`}
            tenant={tenant}
            kind="revisions"
          />
          <HistoryPanel
            key={`${tenant}-decisions`}
            tenant={tenant}
            kind="decisions"
          />
        </>
      )}
      {editor && (
        <Modal
          title={t(editor.revision ? "authEditPolicy" : "authCreatePolicy")}
          close={close}
        >
          <PolicyEditor
            snapshot={editor}
            busy={busy}
            error={error}
            save={async (bundle) => {
              if (
                await mutate(() =>
                  authorizationReplace(path, {
                    expected_revision: editor.revision,
                    bundle,
                  }),
                )
              )
                setEditor(null);
            }}
          />
        </Modal>
      )}
      {issuing && (
        <Modal title={t("authIssueCredential")} close={close}>
          <form
            className="auth-editor"
            onSubmit={(event) => {
              event.preventDefault();
              const data = new FormData(event.currentTarget);
              void mutate(async () => {
                const value = await authorizationIssueCredential(path, {
                  subject: String(data.get("subject")),
                  expires_in_seconds: Number(data.get("seconds")),
                });
                setIssuing(false);
                setIssued(value);
              });
            }}
          >
            {error && (
              <p role="alert" className="error">
                {error}
              </p>
            )}
            <Field label={t("authSubject")}>
              <select name="subject" required>
                {Object.entries(current?.bundle.subjects ?? {})
                  .filter(([, subject]) => subject.enabled !== false)
                  .map(([id]) => (
                    <option key={id}>{id}</option>
                  ))}
              </select>
            </Field>
            <Field label={t("authLifetime")}>
              <input
                name="seconds"
                type="number"
                required
                min={1}
                max={2592000}
                defaultValue={3600}
              />
            </Field>
            <button className="primary" disabled={busy}>
              {t("authIssueCredential")}
            </button>
          </form>
        </Modal>
      )}
      {issued && (
        <Modal title={t("authIssuedCredential")} close={() => setIssued(null)}>
          <div className="auth-editor">
            <p className="notice">{t("authTokenOnce")}</p>
            <p>
              {t("tenant")}: {tenant} · {t("authSubject")}:{" "}
              {issued.credential.subject}
            </p>
            <Field label={t("authIssuedToken")}>
              <textarea
                readOnly
                value={issued.token}
                autoComplete="off"
                spellCheck={false}
                onFocus={(event) => event.target.select()}
              />
            </Field>
            <button onClick={() => setIssued(null)}>
              {t("authDismissToken")}
            </button>
          </div>
        </Modal>
      )}
      {revoking && (
        <Modal title={t("authRevoke")} close={close}>
          <div className="auth-editor">
            <p>{t("authRevokeHelp")}</p>
            <p>{revoking.subject}</p>
            <span>
              {revoking.subject} ·{" "}
              {new Date(revoking.created_at).toLocaleString()}
            </span>
            {error && (
              <p role="alert" className="error">
                {error}
              </p>
            )}
            <button
              className="primary"
              disabled={busy}
              onClick={() =>
                void mutate(async () => {
                  await authorizationRevokeCredential(path, revoking.id);
                  if (issued?.credential.id === revoking.id) setIssued(null);
                  setRevoking(null);
                })
              }
            >
              {t("authConfirmRevoke")}
            </button>
          </div>
        </Modal>
      )}
    </>
  );
}

function BundleSummary({ snapshot }: { snapshot: Snapshot }) {
  const { t } = useI18n();
  return (
    <dl className="auth-summary">
      {[
        ["revision", snapshot.revision],
        ["authSubjects", Object.keys(snapshot.bundle.subjects ?? {}).length],
        ["authGroups", Object.keys(snapshot.bundle.groups ?? {}).length],
        ["authRoles", Object.keys(snapshot.bundle.roles ?? {}).length],
        ["authRules", snapshot.bundle.policies?.length ?? 0],
      ].map(([label, value]) => (
        <div key={label}>
          <dt>{t(String(label))}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function PolicyEditor({
  snapshot,
  busy,
  error,
  save,
}: {
  snapshot: Snapshot;
  busy: boolean;
  error: string;
  save: (bundle: PolicyBundle) => Promise<void>;
}) {
  const { t } = useI18n();
  const [draft, setDraft] = useState(pretty(snapshot.bundle));
  const [review, setReview] = useState<PolicyBundle | null>(null);
  const [invalid, setInvalid] = useState("");
  return (
    <form
      className="auth-editor"
      onSubmit={(event) => {
        event.preventDefault();
        setInvalid("");
        if (review) {
          void save(review);
          return;
        }
        try {
          const bundle = jsonObject(draft);
          if (
            bundle.tenant !== snapshot.bundle.tenant ||
            (bundle.policies !== undefined &&
              !Array.isArray(bundle.policies)) ||
            ["subjects", "groups", "roles"].some(
              (key) =>
                bundle[key] !== undefined &&
                (bundle[key] === null ||
                  typeof bundle[key] !== "object" ||
                  Array.isArray(bundle[key])),
            )
          )
            throw new Error("authInvalidBundle");
          setReview(bundle as unknown as PolicyBundle);
        } catch {
          setInvalid(t("authInvalidBundle"));
        }
      }}
    >
      <p>{t("authPolicyHelp")}</p>
      <p>
        {t("tenant")}: {snapshot.bundle.tenant} · {t("authExpectedRevision")}:{" "}
        {snapshot.revision}
      </p>
      {(error || invalid) && (
        <p role="alert" className="error">
          {error || invalid}
        </p>
      )}
      {review ? (
        <>
          <p className="notice">{t("authReviewHelp")}</p>
          <BundleSummary
            snapshot={{ bundle: review, revision: snapshot.revision + 1 }}
          />
          <JsonView value={review} />
          <button type="button" disabled={busy} onClick={() => setReview(null)}>
            {t("authBackToEdit")}
          </button>
        </>
      ) : (
        <Field label={t("authPolicyJson")}>
          <textarea
            required
            rows={20}
            spellCheck={false}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
        </Field>
      )}
      <button className="primary" disabled={busy}>
        {t(review ? "authApplyPolicy" : "authReviewPolicy")}
      </button>
    </form>
  );
}

function DecisionView({ decision }: { decision: Decision }) {
  const { t } = useI18n();
  return (
    <div className="auth-decision" role="status">
      <Badge value={decision.allowed ? "authAllowed" : "authDenied"} />
      <dl>
        <dt>{t("authDecisionReason")}</dt>
        <dd>{t(`authReason_${decision.reason}`)}</dd>
        <dt>{t("revision")}</dt>
        <dd>{decision.revision}</dd>
        <dt>{t("authMatchedPolicies")}</dt>
        <dd>{decision.matched_policies.join(", ") || "—"}</dd>
        <dt>{t("authEffectiveRoles")}</dt>
        <dd>{decision.effective_roles.join(", ") || "—"}</dd>
      </dl>
    </div>
  );
}

function EvaluationPanel({
  tenant,
  snapshot,
}: {
  tenant: string;
  snapshot: Snapshot;
}) {
  const { t } = useI18n();
  const client = useQueryClient();
  const [decision, setDecision] = useState<Decision | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [audit, setAudit] = useState(false);
  return (
    <Panel title={t("authEvaluate")}>
      <form
        className="auth-editor"
        onChange={() => {
          setDecision(null);
          setError("");
        }}
        onSubmit={(event) => {
          event.preventDefault();
          if (busy) return;
          const data = new FormData(event.currentTarget);
          setError("");
          setDecision(null);
          let input: Evaluation;
          try {
            input = {
              subject: String(data.get("subject")),
              action: String(data.get("action")),
              resource: {
                tenant,
                kind: String(data.get("kind")),
                id: String(data.get("resource")),
                attributes: jsonObject(String(data.get("attributes"))),
              },
              environment: jsonObject(String(data.get("environment"))),
            };
          } catch {
            setError(t("authJsonObject"));
            return;
          }
          setBusy(true);
          const evaluate = audit
            ? authorizationEvaluate
            : authorizationSimulate;
          void evaluate(encodeURIComponent(tenant), input)
            .then((value) => {
              setDecision(value);
              if (audit)
                void client.invalidateQueries({
                  queryKey: ["authorization", tenant, "decisions"],
                });
            })
            .catch((reason) =>
              setError(
                reason instanceof Error ? reason.message : String(reason),
              ),
            )
            .finally(() => setBusy(false));
        }}
      >
        <p>{t("authEvaluateHelp")}</p>
        <fieldset disabled={busy} className="auth-fields">
          <Field label={t("authSubject")}>
            <select name="subject" required>
              {Object.keys(snapshot.bundle.subjects ?? {}).map((id) => (
                <option key={id}>{id}</option>
              ))}
            </select>
          </Field>
          <Field label={t("authAction")}>
            <input name="action" required defaultValue="workspace.read" />
          </Field>
          <Field label={t("authResourceKind")}>
            <input name="kind" required defaultValue="workspace" />
          </Field>
          <Field label={t("authResourceId")}>
            <input name="resource" required defaultValue="example" />
          </Field>
          <Field label={t("authResourceAttributes")}>
            <textarea
              name="attributes"
              defaultValue="{}"
              rows={3}
              spellCheck={false}
              required
            />
          </Field>
          <Field label={t("authEnvironment")}>
            <textarea
              name="environment"
              defaultValue="{}"
              rows={3}
              spellCheck={false}
              required
            />
          </Field>
          <label className="generation-check">
            <input
              type="checkbox"
              checked={audit}
              onChange={(event) => setAudit(event.target.checked)}
            />
            {t("authRecordDecision")}
          </label>
        </fieldset>
        <button disabled={busy} className="primary">
          {t(audit ? "authEvaluateRecord" : "authDryRun")}
        </button>
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        {decision && <DecisionView decision={decision} />}
      </form>
    </Panel>
  );
}

function CatalogForm({
  entries,
  bindings,
  busy,
  save,
}: {
  entries: Entry[];
  bindings: Binding[];
  busy: boolean;
  save: (input: {
    entry: { id: string; version: string };
    expected_revision: number;
    enabled: boolean;
  }) => Promise<boolean>;
}) {
  const { t, local } = useI18n();
  const available = entries.filter(
    (entry) =>
      !bindings.some(
        (binding) =>
          binding.entry_id === entry.id &&
          binding.entry_version === entry.version &&
          binding.enabled,
      ),
  );
  return (
    <form
      className="auth-catalog-form"
      onSubmit={(event) => {
        event.preventDefault();
        const selected = String(new FormData(event.currentTarget).get("entry"));
        const entry = available.find(
          (entry) => JSON.stringify([entry.id, entry.version]) === selected,
        );
        if (!entry) return;
        const binding = bindings.find(
          (binding) =>
            binding.entry_id === entry.id &&
            binding.entry_version === entry.version,
        );
        void save({
          entry: { id: entry.id, version: entry.version },
          expected_revision: binding?.revision ?? 0,
          enabled: true,
        });
      }}
    >
      <Field label={t("authCatalogEntry")}>
        <select name="entry" required defaultValue="">
          <option value="" disabled>
            {t("authSelectEntry")}
          </option>
          {available.map((entry) => (
            <option
              value={JSON.stringify([entry.id, entry.version])}
              key={`${entry.id}@${entry.version}`}
            >
              {local(entry.name) || t("unnamedEntity")} · {entry.version}
            </option>
          ))}
        </select>
      </Field>
      <button disabled={busy || !available.length}>{t("authApprove")}</button>
    </form>
  );
}

function HistoryPanel({
  tenant,
  kind,
}: {
  tenant: string;
  kind: "revisions" | "decisions";
}) {
  const { t } = useI18n();
  const [cursors, setCursors] = useState([0]);
  const after = cursors[cursors.length - 1];
  const query = useQuery({
    queryKey: ["authorization", tenant, kind, after],
    queryFn: () =>
      (kind === "revisions" ? authorizationRevisions : authorizationDecisions)(
        encodeURIComponent(tenant),
        { after, limit: PAGE_SIZE },
      ),
    refetchInterval: 5000,
  });
  const records = !query.isError ? query.data : undefined;
  const next = Number(
    object(records?.at(-1))[kind === "revisions" ? "revision" : "sequence"],
  );
  return (
    <Panel
      title={t(
        kind === "revisions" ? "authRevisionHistory" : "authDecisionHistory",
      )}
    >
      {query.isError && (
        <p role="alert" className="error">
          {query.error.message}
        </p>
      )}
      {query.isPending && <p className="auth-padding">{t("loading")}</p>}
      {records?.length === 0 && (
        <p className="auth-padding">{t("authNoHistory")}</p>
      )}
      {records?.map((value, index) => {
        const record = object(value);
        return (
          <details
            className="auth-history"
            key={String(record.revision) + index}
          >
            <summary>
              {kind === "revisions"
                ? `${t("revision")} ${record.revision} · ${record.actor}`
                : `#${record.sequence} · ${record.subject} · ${record.action} · ${t(object(record.decision).allowed ? "authAllowed" : "authDenied")}`}
              <time>{String(record.created_at ?? "")}</time>
            </summary>
            <JsonView value={value} />
          </details>
        );
      })}
      <div className="auth-pagination">
        <button
          disabled={cursors.length === 1 || query.isFetching}
          onClick={() => setCursors(cursors.slice(0, -1))}
        >
          {t("authPrevious")}
        </button>
        <span>
          {t("authPage")} {cursors.length}
        </span>
        <button
          disabled={
            records?.length !== PAGE_SIZE ||
            !Number.isSafeInteger(next) ||
            next <= after ||
            query.isFetching
          }
          onClick={() => setCursors([...cursors, next])}
        >
          {t("authNext")}
        </button>
      </div>
    </Panel>
  );
}
