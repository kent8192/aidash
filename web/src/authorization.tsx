import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { RecordView } from "./record-view";
import { useState } from "react";
import { ShieldCheck, ShieldX } from "lucide-react";
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
  transactionRevocations,
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
  type StatusTone,
} from "./ui";
import {
  Alert,
  Check,
  Disclosure,
  Facts,
  HistoryRow,
  Hint,
  Loading,
  Metric,
  MetricRow,
  Notice,
  Pager,
  RowList,
} from "./components/patterns";
import { cn } from "./lib/utils";

const PAGE_SIZE = 25;

const accessTones = {
  authAllowed: "success",
  authApproved: "success",
  authActive: "success",
  authDenied: "danger",
  authRevoked: "danger",
  authExpired: "warning",
  authDisabled: "neutral",
} satisfies Record<string, StatusTone>;

/** Access-control state badge; keeps the `badge <state>` hook class. */
function AccessBadge({ value }: { value: keyof typeof accessTones }) {
  return <Badge value={value} tone={accessTones[value]} />;
}
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
    <div className="authorization-page grid min-w-0 gap-6">
      <div className="grid min-w-0 gap-3">
        <Hint>{t("authorizationHelp")}</Hint>
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            setTenant(
              String(new FormData(event.currentTarget).get("tenant")).trim(),
            );
          }}
        >
          <div className="w-full min-w-0 sm:w-72">
            <Field label={t("tenant")}>
              <Input
                name="tenant"
                required
                maxLength={256}
                autoComplete="off"
                spellCheck={false}
                className="font-mono"
              />
            </Field>
          </div>
          <Button variant="outline">{t("open")}</Button>
        </form>
      </div>
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
  const { t, locale } = useI18n();
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
  const admissions = useQuery({
    queryKey: ["authorization", tenant, "transaction-revocations"],
    queryFn: () => transactionRevocations(path),
    refetchInterval: 1000,
  });
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
      <div className="grid min-w-0 gap-3">
        <p className="text-xs text-muted-foreground">
          {t("tenant")}{" "}
          <span className="font-mono text-foreground">{tenant}</span>
          {current && (
            <>
              {" · "}
              {t("revision")}{" "}
              <span className="font-mono tabular text-foreground">
                {current.revision}
              </span>
            </>
          )}
        </p>
        {!admissions.isError && (admissions.data?.length ?? 0) > 0 && (
          <Notice tone="warning" role="status">{t("transactionRevocationPending")}</Notice>
        )}
        {error && !modal && <Alert>{error}</Alert>}
        {snapshot.isPending && <Loading />}
        {snapshot.isError && !missing && (
          <Alert retry={() => void snapshot.refetch()}>
            {snapshot.error.message}
          </Alert>
        )}
      </div>
      {(current || missing) && (
        <Panel
          title={t("authPolicyBundle")}
          action={
            <Button
              variant={current ? "outline" : "default"}
              size="sm"
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
            </Button>
          }
        >
          {current ? (
            <>
              <BundleSummary snapshot={current} />
              <Disclosure summary={t("authCurrentDocument")}>
                <JsonView value={current.bundle} />
              </Disclosure>
            </>
          ) : (
            <Hint>{t("authMissingPolicy")}</Hint>
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
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  setError("");
                  setIssued(null);
                  setIssuing(true);
                }}
              >
                {t("authIssueCredential")}
              </Button>
            }
          >
            <Hint>{t("authCredentialsHelp")}</Hint>
            {credentials.isError && (
              <Alert>{credentials.error.message}</Alert>
            )}
            {credentials.isPending && <Loading />}
            {!credentials.isError && credentials.data?.length === 0 && (
              <Empty />
            )}
            {!credentials.isError && !!credentials.data?.length && (
              <RowList>
                {credentials.data.map((credential) => (
                  <div
                    className="auth-row grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-2 py-2 sm:grid-cols-[minmax(0,1fr)_auto_auto]"
                    key={credential.id}
                  >
                    <div className="grid min-w-0 gap-0.5">
                      <span className="truncate font-medium text-foreground">
                        {credential.subject}
                      </span>
                      <span className="truncate text-[11px] text-faint">
                        <time
                          className="font-mono"
                          dateTime={credential.created_at}
                        >
                          {new Date(credential.created_at).toLocaleString(
                            locale,
                          )}
                        </time>
                        {" · "}
                        {t("authExpires")}{" "}
                        <time
                          className="font-mono"
                          dateTime={credential.expires_at}
                        >
                          {new Date(credential.expires_at).toLocaleString(
                            locale,
                          )}
                        </time>
                      </span>
                    </div>
                    <AccessBadge
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
                      <Button
                        variant="destructive"
                        size="sm"
                        className="col-span-2 justify-self-start sm:col-span-1"
                        disabled={busy}
                        onClick={() => {
                          setError("");
                          setRevoking(credential);
                        }}
                      >
                        {t("authRevoke")}
                      </Button>
                    )}
                  </div>
                ))}
              </RowList>
            )}
          </Panel>
          <Panel title={t("authCatalog")}>
            <Hint>{t("authCatalogHelp")}</Hint>
            {catalog.isError ? (
              <Alert>{catalog.error.message}</Alert>
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
                {catalog.data.length > 0 && (
                  <RowList>
                    {catalog.data.map((binding) => (
                      <div
                        className="auth-row grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-2 py-2 sm:grid-cols-[minmax(0,1fr)_auto_auto]"
                        key={`${binding.entry_id}@${binding.entry_version}`}
                      >
                        <div className="grid min-w-0 gap-0.5">
                          <span className="truncate font-medium text-foreground">
                            {entryLabel({
                              id: binding.entry_id,
                              version: binding.entry_version,
                            })}
                          </span>
                          <span className="text-[11px] text-faint">
                            {t("revision")}{" "}
                            <span className="font-mono tabular">
                              {binding.revision}
                            </span>
                          </span>
                        </div>
                        <AccessBadge
                          value={
                            binding.enabled ? "authApproved" : "authDisabled"
                          }
                        />
                        <Button
                          variant="ghost"
                          size="sm"
                          className="col-span-2 justify-self-start sm:col-span-1"
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
                            binding.enabled
                              ? "authDisableApproval"
                              : "authApprove",
                          )}
                        </Button>
                      </div>
                    ))}
                  </RowList>
                )}
              </>
            ) : (
              <Loading />
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
            className="grid min-w-0 gap-3"
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
            {error && <Alert>{error}</Alert>}
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label={t("authSubject")}>
                <NativeSelect name="subject" required className="font-mono">
                  {Object.entries(current?.bundle.subjects ?? {})
                    .filter(([, subject]) => subject.enabled !== false)
                    .map(([id]) => (
                      <option key={id}>{id}</option>
                    ))}
                </NativeSelect>
              </Field>
              <Field label={t("authLifetime")}>
                <Input
                  name="seconds"
                  type="number"
                  required
                  min={1}
                  max={2592000}
                  defaultValue={3600}
                  className="font-mono tabular"
                />
              </Field>
            </div>
            <div className="flex justify-end gap-2 border-t border-border pt-4">
              <Button disabled={busy}>{t("authIssueCredential")}</Button>
            </div>
          </form>
        </Modal>
      )}
      {issued && (
        <Modal title={t("authIssuedCredential")} close={() => setIssued(null)}>
          <div className="grid min-w-0 gap-3">
            <Notice tone="warning">{t("authTokenOnce")}</Notice>
            <Facts
              items={[
                [t("tenant"), <span className="font-mono">{tenant}</span>],
                [
                  t("authSubject"),
                  <span className="font-mono">
                    {issued.credential.subject}
                  </span>,
                ],
              ]}
            />
            <Field label={t("authIssuedToken")}>
              <Textarea
                readOnly
                value={issued.token}
                autoComplete="off"
                spellCheck={false}
                rows={4}
                className="font-mono text-xs break-all"
                onFocus={(event) => event.target.select()}
              />
            </Field>
            <div className="flex justify-end border-t border-border pt-4">
              <Button variant="outline" onClick={() => setIssued(null)}>
                {t("authDismissToken")}
              </Button>
            </div>
          </div>
        </Modal>
      )}
      {revoking && (
        <Modal title={t("authRevoke")} close={close}>
          <div className="grid min-w-0 gap-3">
            <p className="text-[13px] leading-relaxed text-muted-foreground">
              {t("authRevokeHelp")}
            </p>
            <Facts
              items={[
                [
                  t("authSubject"),
                  <span className="font-mono">{revoking.subject}</span>,
                ],
                [
                  t("createdAt"),
                  <time className="font-mono" dateTime={revoking.created_at}>
                    {new Date(revoking.created_at).toLocaleString(locale)}
                  </time>,
                ],
              ]}
            />
            {error && <Alert>{error}</Alert>}
            <div className="flex flex-wrap justify-end gap-2 border-t border-border pt-4">
              <Button variant="outline" disabled={busy} onClick={close}>
                {t("cancel")}
              </Button>
              <Button
                variant="destructive"
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
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </>
  );
}

function BundleSummary({ snapshot }: { snapshot: Snapshot }) {
  const { t } = useI18n();
  return (
    <MetricRow className="border-y">
      {(
        [
          ["revision", snapshot.revision],
          ["authSubjects", Object.keys(snapshot.bundle.subjects ?? {}).length],
          ["authGroups", Object.keys(snapshot.bundle.groups ?? {}).length],
          ["authRoles", Object.keys(snapshot.bundle.roles ?? {}).length],
          ["authRules", snapshot.bundle.policies?.length ?? 0],
        ] as const
      ).map(([label, value]) => (
        <Metric key={label} label={t(label)} value={value} />
      ))}
    </MetricRow>
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
      className="grid min-w-0 gap-3"
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
      <p className="text-[13px] leading-relaxed text-muted-foreground">
        {t("authPolicyHelp")}
      </p>
      <Facts
        items={[
          [
            t("tenant"),
            <span className="font-mono">{snapshot.bundle.tenant}</span>,
          ],
          [
            t("authExpectedRevision"),
            <span className="font-mono tabular">{snapshot.revision}</span>,
          ],
        ]}
      />
      {(error || invalid) && <Alert>{error || invalid}</Alert>}
      {review ? (
        <>
          <Notice tone="warning">{t("authReviewHelp")}</Notice>
          <BundleSummary
            snapshot={{ bundle: review, revision: snapshot.revision + 1 }}
          />
          <JsonView value={review} />
        </>
      ) : (
        <Field label={t("authPolicyJson")}>
          <Textarea
            required
            rows={20}
            spellCheck={false}
            className="font-mono text-xs"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
        </Field>
      )}
      <div className="flex flex-wrap justify-end gap-2 border-t border-border pt-4">
        {review && (
          <Button
            variant="outline"
            type="button"
            disabled={busy}
            onClick={() => setReview(null)}
          >
            {t("authBackToEdit")}
          </Button>
        )}
        <Button disabled={busy}>
          {t(review ? "authApplyPolicy" : "authReviewPolicy")}
        </Button>
      </div>
    </form>
  );
}

function DecisionView({ decision }: { decision: Decision }) {
  const { t } = useI18n();
  const Icon = decision.allowed ? ShieldCheck : ShieldX;
  return (
    <div
      className={cn(
        "auth-decision grid min-w-0 gap-3 rounded-lg border p-4",
        decision.allowed
          ? "border-success/30 bg-success-soft"
          : "border-destructive/30 bg-destructive-soft",
      )}
      role="status"
    >
      <div
        className={cn(
          "flex items-center gap-2",
          decision.allowed ? "text-success" : "text-destructive",
        )}
      >
        <Icon aria-hidden className="size-5 shrink-0" />
        <span className="text-[15px] font-semibold">
          {t(decision.allowed ? "authAllowed" : "authDenied")}
        </span>
      </div>
      <Facts
        className="sm:grid-cols-[minmax(0,8rem)_minmax(0,1fr)]"
        items={[
          [t("authDecisionReason"), t(`authReason_${decision.reason}`)],
          [
            t("revision"),
            <span className="font-mono tabular">{decision.revision}</span>,
          ],
          [
            t("authMatchedPolicies"),
            <span className="font-mono">
              {decision.matched_policies.join(", ") || "-"}
            </span>,
          ],
          [
            t("authEffectiveRoles"),
            <span className="font-mono">
              {decision.effective_roles.join(", ") || "-"}
            </span>,
          ],
        ]}
      />
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
        className="grid min-w-0 gap-5 lg:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]"
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
        <div className="grid min-w-0 content-start gap-3">
          <fieldset
            disabled={busy}
            className="grid min-w-0 gap-3 sm:grid-cols-2"
          >
            <Field label={t("authSubject")}>
              <NativeSelect name="subject" required className="font-mono">
                {Object.keys(snapshot.bundle.subjects ?? {}).map((id) => (
                  <option key={id}>{id}</option>
                ))}
              </NativeSelect>
            </Field>
            <Field label={t("authAction")}>
              <Input
                name="action"
                required
                defaultValue="workspace.read"
                className="font-mono"
              />
            </Field>
            <Field label={t("authResourceKind")}>
              <Input
                name="kind"
                required
                defaultValue="workspace"
                className="font-mono"
              />
            </Field>
            <Field label={t("authResourceId")}>
              <Input
                name="resource"
                required
                defaultValue="example"
                className="font-mono"
              />
            </Field>
            <Field label={t("authResourceAttributes")}>
              <Textarea
                name="attributes"
                defaultValue="{}"
                rows={3}
                spellCheck={false}
                className="font-mono text-xs"
                required
              />
            </Field>
            <Field label={t("authEnvironment")}>
              <Textarea
                name="environment"
                defaultValue="{}"
                rows={3}
                spellCheck={false}
                className="font-mono text-xs"
                required
              />
            </Field>
          </fieldset>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <Button disabled={busy}>
              {t(audit ? "authEvaluateRecord" : "authDryRun")}
            </Button>
            <Check
              checked={audit}
              disabled={busy}
              onChange={(event) => setAudit(event.target.checked)}
            >
              {t("authRecordDecision")}
            </Check>
          </div>
        </div>
        <div className="grid min-w-0 content-start gap-3">
          {error && <Alert>{error}</Alert>}
          {decision ? (
            <DecisionView decision={decision} />
          ) : (
            <div className="rounded-lg border border-dashed border-border-strong p-4">
              <Hint>{t("authEvaluateHelp")}</Hint>
            </div>
          )}
        </div>
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
      className="flex flex-wrap items-end gap-2"
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
      <div className="w-full min-w-0 sm:w-96">
        <Field label={t("authCatalogEntry")}>
          <NativeSelect name="entry" required defaultValue="">
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
          </NativeSelect>
        </Field>
      </div>
      <Button variant="outline" disabled={busy || !available.length}>
        {t("authApprove")}
      </Button>
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
  const { t, locale } = useI18n();
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
      {query.isError && <Alert>{query.error.message}</Alert>}
      {query.isPending && <Loading />}
      {records?.length === 0 && (
        <p className="text-xs text-muted-foreground">{t("authNoHistory")}</p>
      )}
      {!!records?.length && (
        <RowList>
          {records.map((value, index) => {
            const record = object(value);
            const allowed = !!object(record.decision).allowed;
            return (
              <HistoryRow
                className="auth-history"
                key={String(record.revision) + index}
                time={
                  record.created_at
                    ? new Date(String(record.created_at)).toLocaleString(locale)
                    : ""
                }
                summary={
                  kind === "revisions" ? (
                    <>
                      {t("revision")}{" "}
                      <span className="font-mono tabular">
                        {String(record.revision)}
                      </span>
                      <span className="text-muted-foreground">
                        {" · "}
                        {String(record.actor)}
                      </span>
                    </>
                  ) : (
                    <>
                      <AccessBadge
                        value={allowed ? "authAllowed" : "authDenied"}
                      />{" "}
                      <span className="font-mono">
                        {String(record.subject)} · {String(record.action)}
                      </span>
                    </>
                  )
                }
              >
                <RecordView value={value} />
              </HistoryRow>
            );
          })}
        </RowList>
      )}
      <Pager
        page={cursors.length}
        previous={{
          disabled: cursors.length === 1 || query.isFetching,
          go: () => setCursors(cursors.slice(0, -1)),
        }}
        next={{
          disabled:
            records?.length !== PAGE_SIZE ||
            !Number.isSafeInteger(next) ||
            next <= after ||
            query.isFetching,
          go: () => setCursors([...cursors, next]),
        }}
      />
    </Panel>
  );
}
