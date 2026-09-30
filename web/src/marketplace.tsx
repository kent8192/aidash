import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  marketplaceBrowse,
  marketplaceDetail,
  marketplaceSources,
  marketplacePublicationAccess,
  marketplacePublishRegistered,
  marketplaceInstallScoped,
  marketplaceInstallations,
  marketplaceInstallation,
  marketplaceConfigure,
  marketplaceShare,
  marketplaceConsent,
  marketplaceCompatibility,
  marketplaceSetCompatibility,
  marketplaceAdministration,
  marketplaceActivate,
  marketplaceAdopt,
  authorizationCatalog,
} from "./generated/aidash";
import type {
  MarketplaceInstallationRevision,
  MarketplaceDependencyBinding,
} from "./generated/models";
import { ApiError, dashboardContext } from "./transport";
import { useI18n, Panel, Field } from "./ui";
import { RecordView } from "./record-view";
import { marketplaceCopy } from "./marketplace-copy";

const json = (text: string): Record<string, unknown> => {
  const value: unknown = JSON.parse(text);
  if (!value || Array.isArray(value) || typeof value !== "object")
    throw new Error("JSON must be an object");
  return value as Record<string, unknown>;
};
const tenants = (text: string) => [
  ...new Set(text.split(/\s+/).filter(Boolean)),
];

export function ScopedMarketplace({ identity }: { identity: string }) {
  const { locale, local } = useI18n();
  const copy = marketplaceCopy[locale];
  const cache = useQueryClient();
  const context = ["marketplace", dashboardContext(), identity];
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<string>();
  const [editing, setEditing] = useState<MarketplaceInstallationRevision>();
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [denied, setDenied] = useState(false);
  const [config, setConfig] = useState("{}");
  const [bindings, setBindings] = useState("[]");
  const [source, setSource] = useState("");
  const [packageId, setPackageId] = useState("");
  const [author, setAuthor] = useState("");
  const [audience, setAudience] = useState<string>();
  const [redistributor, setRedistributor] = useState("");
  const [consentRevision, setConsentRevision] = useState(0);
  const [consentAudience, setConsentAudience] = useState("");
  const packages = useQuery({
    queryKey: [...context, "packages", search],
    queryFn: () => marketplaceBrowse({ q: search, limit: 50 }),
    enabled: !denied,
    retry: false,
  });
  const installs = useQuery({
    queryKey: [...context, "installations"],
    queryFn: () => marketplaceInstallations(),
    enabled: !denied,
    retry: false,
  });
  const sources = useQuery({
    queryKey: [...context, "sources"],
    queryFn: () => marketplaceSources(),
    enabled: !denied,
    retry: false,
  });
  const detail = useQuery({
    queryKey: [...context, "detail", selected],
    queryFn: () => marketplaceDetail(selected!),
    enabled: !!selected && !denied,
    retry: false,
    staleTime: 0,
  });
  const editingQuery = useQuery({
    queryKey: [
      ...context,
      "installation",
      editing?.installation.id,
      editing?.revision,
    ],
    queryFn: () =>
      marketplaceInstallation(editing!.installation.id, {
        revision: editing!.revision,
      }),
    enabled: !!editing && !denied,
    retry: false,
    staleTime: 0,
  });
  const editingView = editingQuery.data;
  const chosenSource = sources.data?.find(
    (s) => `${s.id}@${s.version}` === source,
  );
  const canPublish = useQuery({
    queryKey: [...context, "publish-access", source, packageId, author],
    queryFn: () =>
      marketplacePublicationAccess({
        source: { id: chosenSource!.id, version: chosenSource!.version },
        package_id: packageId,
        author,
        idempotency_key: crypto.randomUUID(),
      }),
    enabled: !!chosenSource && !!packageId && !!author && !denied,
    retry: false,
  });
  const failure = [
    packages.error,
    installs.error,
    sources.error,
    detail.error,
    canPublish.error,
    editingQuery.error,
  ].find(
    (error) => error instanceof ApiError && [401, 403].includes(error.status),
  );
  useEffect(() => {
    if (denied) cache.removeQueries({ queryKey: ["marketplace"] });
  }, [denied, cache]);
  // Clear protected local drafts before rendering a revoked query result.
  if (failure && !denied) {
    setDenied(true);
    setSelected(undefined);
    setEditing(undefined);
    setSource("");
    setConfig("{}");
    setBindings("[]");
  }
  async function run(
    action: () => Promise<unknown>,
    success: string = copy.saved,
  ) {
    setBusy(true);
    setMessage("");
    try {
      await action();
      setMessage(success);
      await cache.invalidateQueries({ queryKey: context });
    } catch (error) {
      if (error instanceof ApiError && [401, 403].includes(error.status)) {
        setDenied(true);
        setSelected(undefined);
        setEditing(undefined);
        setSource("");
        setConfig("{}");
        setBindings("[]");
        cache.removeQueries({ queryKey: ["marketplace"] });
        setMessage(copy.operationUnavailable);
      } else
        setMessage(
          error instanceof ApiError && error.status === 409
            ? copy.conflict
            : String(error),
        );
    } finally {
      setBusy(false);
    }
  }
  const bindingInput = () =>
    JSON.parse(bindings) as MarketplaceDependencyBinding[];
  if (denied)
    return (
      <Panel title={copy.packages}>
        <p role="alert">{copy.unavailable}</p>
        <button
          onClick={() => {
            setDenied(false);
            setMessage("");
          }}
        >
          {copy.retry}
        </button>
      </Panel>
    );
  return (
    <div className="marketplace-scoped">
      {message && <p role="status">{message}</p>}
      <Panel title={copy.packages}>
        <Field label={copy.search}>
          <input
            value={search}
            onChange={(event) => {
              setSearch(event.target.value);
              setSelected(undefined);
            }}
          />
        </Field>
        {packages.isError && <p role="alert">{copy.unavailable}</p>}
        {!packages.isPending && !packages.isError && !packages.data?.length && (
          <p>{copy.empty}</p>
        )}
        <div className="cards">
          {packages.data?.map((item) => (
            <article className="entity-card" key={item.key}>
              <h3>{local(item.name)}</h3>
              <p>{local(item.description)}</p>
              <small>
                {item.owner_tenant} · {item.version} · {item.author}
              </small>
              <p>{item.capabilities.join(", ")}</p>
              {item.actions.includes("read") ? (
                <button
                  onClick={() => {
                    setSelected(item.key);
                    setAudience(undefined);
                    setEditing(undefined);
                    setConfig("{}");
                    setBindings("[]");
                  }}
                >
                  {copy.details}
                </button>
              ) : (
                <p>{copy.unavailable}</p>
              )}
            </article>
          ))}
        </div>
      </Panel>
      {selected && detail.data && (
        <Panel title={local(detail.data.summary.name)}>
          <p>{copy.dependencies}</p>
          <p>
            {copy.permission}: {detail.data.manifest.permissions.join(", ")}
          </p>
          <details>
            <summary>{copy.manifest}</summary>
            <RecordView value={detail.data.manifest} />
          </details>
          <Field label={copy.config}>
            <textarea
              value={config}
              onChange={(e) => setConfig(e.target.value)}
            />
          </Field>
          <Field label={copy.bindings}>
            <textarea
              value={bindings}
              onChange={(e) => setBindings(e.target.value)}
            />
          </Field>
          {detail.data.summary.actions.includes("install") && (
            <button
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  // Fetch again at the user's mutation boundary; server authorization is final.
                  const latest = await marketplaceDetail(selected);
                  await marketplaceInstallScoped(selected, {
                    digest: latest.summary.digest,
                    config: json(config),
                    bindings: bindingInput(),
                    idempotency_key: crypto.randomUUID(),
                  });
                }, copy.installed)
              }
            >
              {copy.install}
            </button>
          )}
          {detail.data.summary.actions.includes("share") && (
            <>
              <Field label={copy.tenants}>
                <textarea
                  value={audience ?? detail.data.audience.tenants.join("\n")}
                  onChange={(e) => setAudience(e.target.value)}
                />
              </Field>
              <button
                disabled={busy}
                onClick={() =>
                  void run(() =>
                    marketplaceShare(selected, {
                      expected_revision: detail.data!.audience.revision,
                      tenants: tenants(
                        audience ?? detail.data!.audience.tenants.join("\n"),
                      ),
                    }),
                  )
                }
              >
                {copy.share}
              </button>
            </>
          )}
          {detail.data.summary.actions.includes("consent") && (
            <details>
              <summary>{copy.consent}</summary>
              <p>{copy.consentHelp}</p>
              <Field label={copy.redistributor}>
                <input
                  value={redistributor}
                  onChange={(e) => setRedistributor(e.target.value)}
                />
              </Field>
              <Field label={copy.consentRevision}>
                <input
                  type="number"
                  min={0}
                  value={consentRevision}
                  onChange={(e) => setConsentRevision(Number(e.target.value))}
                />
              </Field>
              <Field label={copy.tenants}>
                <textarea
                  value={consentAudience}
                  onChange={(e) => setConsentAudience(e.target.value)}
                />
              </Field>
              <button
                disabled={busy || !redistributor}
                onClick={() =>
                  void run(async () => {
                    const next = await marketplaceConsent(
                      selected,
                      redistributor,
                      {
                        expected_revision: consentRevision,
                        tenants: tenants(consentAudience),
                      },
                    );
                    setConsentRevision(next.revision);
                  })
                }
              >
                {copy.consent}
              </button>
            </details>
          )}
          <button onClick={() => setSelected(undefined)}>{copy.close}</button>
        </Panel>
      )}
      <Panel title={copy.installations}>
        <p>{copy.pinned}</p>
        {installs.data?.map((item) => (
          <article className="entity-card" key={item.installation.id}>
            <h3>{local(item.entry.name)}</h3>
            <p>
              {item.approved &&
              item.installation.active_revision === item.revision
                ? copy.active
                : item.approved
                  ? copy.retained
                  : item.installation.active_revision === item.revision
                    ? copy.revoked
                    : item.installation.active_revision
                      ? copy.changed
                      : copy.pending}
            </p>
            <p>
              {copy.revision}: {item.revision} · {copy.activeRevision}:{" "}
              {item.installation.active_revision ?? "—"}
            </p>
            {item.installation.active_revision &&
              item.installation.active_revision !== item.revision && (
                <button
                  onClick={() =>
                    void run(async () => {
                      const active = await marketplaceInstallation(
                        item.installation.id,
                        { revision: item.installation.active_revision! },
                      );
                      setEditing(active);
                      setConfig(JSON.stringify(active.config, null, 2));
                      setBindings(JSON.stringify(active.bindings, null, 2));
                    })
                  }
                >
                  {copy.active}
                </button>
              )}
            {item.actions.includes("configure") && (
              <button
                onClick={() => {
                  setEditing(item);
                  setSelected(undefined);
                  setConfig(JSON.stringify(item.config, null, 2));
                  setBindings(JSON.stringify(item.bindings, null, 2));
                }}
              >
                {copy.configure}
              </button>
            )}
          </article>
        ))}
        {editingView && (
          <div>
            <h3>
              {local(editingView.entry.name)} · {copy.revision}{" "}
              {editingView.revision}
            </h3>
            <Field label={copy.config}>
              <textarea
                value={config}
                onChange={(e) => setConfig(e.target.value)}
              />
            </Field>
            <Field label={copy.bindings}>
              <textarea
                value={bindings}
                onChange={(e) => setBindings(e.target.value)}
              />
            </Field>
            {editingView.actions.includes("configure") && (
              <button
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    const result = await marketplaceConfigure(
                      editingView.installation.id,
                      {
                        expected_revision:
                          editingView.installation.latest_revision,
                        config: json(config),
                        bindings: bindingInput(),
                        idempotency_key: crypto.randomUUID(),
                      },
                    );
                    setEditing(result);
                  })
                }
              >
                {copy.configure}
              </button>
            )}
            <button onClick={() => setEditing(undefined)}>{copy.close}</button>
          </div>
        )}
      </Panel>
      {!!sources.data?.length && (
        <Panel title={copy.publish}>
          <p>{copy.publishHelp}</p>
          <Field label={copy.source}>
            <select
              value={source}
              onChange={(e) => {
                setSource(e.target.value);
                const item = sources.data?.find(
                  (s) => `${s.id}@${s.version}` === e.target.value,
                );
                setPackageId(item?.id ?? "");
              }}
            >
              <option value="">—</option>
              {sources.data.map((item) => (
                <option
                  key={`${item.id}@${item.version}`}
                  value={`${item.id}@${item.version}`}
                >
                  {local(item.name)} · {item.version}
                </option>
              ))}
            </select>
          </Field>
          <Field label={copy.packageId}>
            <input
              value={packageId}
              onChange={(e) => setPackageId(e.target.value)}
            />
          </Field>
          <Field label={copy.author}>
            <input value={author} onChange={(e) => setAuthor(e.target.value)} />
          </Field>
          {canPublish.data?.allowed && (
            <p>
              {copy.publicationVersion}: {canPublish.data.version}
            </p>
          )}
          {canPublish.data?.allowed && (
            <button
              disabled={busy || !source || !packageId || !author}
              onClick={() =>
                void run(async () => {
                  const entry = sources.data!.find(
                    (s) => `${s.id}@${s.version}` === source,
                  )!;
                  await marketplacePublishRegistered({
                    source: { id: entry.id, version: entry.version },
                    package_id: packageId,
                    author,
                    idempotency_key: crypto.randomUUID(),
                  });
                }, copy.published)
              }
            >
              {copy.publish}
            </button>
          )}
          {canPublish.data?.allowed === false && (
            <p>{copy.operationUnavailable}</p>
          )}
        </Panel>
      )}
    </div>
  );
}

export function MarketplaceAdministration() {
  const { locale, local } = useI18n();
  const copy = marketplaceCopy[locale];
  const cache = useQueryClient();
  const [tenant, setTenant] = useState("");
  const [draft, setDraft] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [message, setMessage] = useState("");
  const [legacyId, setLegacyId] = useState("");
  const [legacyVersion, setLegacyVersion] = useState("1.0.0");
  const gate = useQuery({
    queryKey: ["marketplace", "operator", "compatibility"],
    queryFn: () => marketplaceCompatibility(),
    retry: false,
  });
  const installs = useQuery({
    queryKey: ["marketplace", "operator", tenant],
    queryFn: () => marketplaceAdministration({ tenant }),
    enabled: !!tenant,
    retry: false,
  });
  async function run(action: () => Promise<unknown>) {
    try {
      await action();
      setMessage(copy.saved);
      await cache.invalidateQueries({ queryKey: ["marketplace"] });
    } catch (error) {
      setMessage(
        error instanceof ApiError && error.status === 409
          ? copy.conflict
          : String(error),
      );
    }
  }
  return (
    <>
      {message && <p role="status">{message}</p>}
      {gate.data && (
        <Panel title={copy.rollout}>
          <p>{gate.data.enabled ? copy.enabled : copy.disabled}</p>
          <label>
            <input
              type="checkbox"
              checked={confirmed}
              onChange={(e) => setConfirmed(e.target.checked)}
            />
            {copy.compatible}
          </label>
          <button
            disabled={!gate.data.enabled && !confirmed}
            onClick={() =>
              void run(() =>
                marketplaceSetCompatibility({
                  enabled: !gate.data!.enabled,
                  expected_revision: gate.data!.revision,
                  compatible_instances_confirmed: confirmed,
                }),
              )
            }
          >
            {gate.data.enabled ? copy.disable : copy.enable}
          </button>
        </Panel>
      )}
      <Panel title={copy.admin}>
        <Field label={copy.tenant}>
          <input value={draft} onChange={(e) => setDraft(e.target.value)} />
        </Field>
        <button onClick={() => setTenant(draft.trim())}>{copy.load}</button>
        <p>{copy.pinned}</p>
        {installs.data?.map((item) => (
          <article
            className="entity-card"
            key={`${item.installation.id}:${item.revision}`}
          >
            <h3>
              {local(item.entry.name)} · {copy.revision} {item.revision}
            </h3>
            <p>
              {item.approved
                ? item.installation.active_revision === item.revision
                  ? copy.active
                  : copy.retained
                : item.installation.active_revision === item.revision
                  ? copy.revoked
                  : item.revision > 1
                    ? copy.changed
                    : copy.pending}
            </p>
            <details>
              <summary>{copy.details}</summary>
              <RecordView value={item.entry} />
            </details>
            {[true, false].map((enabled) => (
              <button
                key={String(enabled)}
                disabled={!enabled && !item.approved}
                onClick={() =>
                  void run(async () => {
                    const catalog = await authorizationCatalog(tenant);
                    const binding = catalog.find(
                      (b) =>
                        b.entry_id === item.entry.id &&
                        b.entry_version === item.entry.version,
                    );
                    await marketplaceActivate(item.installation.id, {
                      tenant,
                      revision: item.revision,
                      expected_activation_revision:
                        item.installation.activation_revision,
                      expected_catalog_revision: binding?.revision ?? 0,
                      enabled,
                    });
                  })
                }
              >
                {enabled ? copy.approve : copy.revoke}
              </button>
            ))}
          </article>
        ))}
        {tenant && (
          <details>
            <summary>{copy.adoption}</summary>
            <p>{copy.adoptionHelp}</p>
            <Field label={copy.legacyId}>
              <input
                value={legacyId}
                onChange={(e) => setLegacyId(e.target.value)}
              />
            </Field>
            <Field label={copy.legacyVersion}>
              <input
                value={legacyVersion}
                onChange={(e) => setLegacyVersion(e.target.value)}
              />
            </Field>
            <button
              disabled={!legacyId}
              onClick={() =>
                void run(() =>
                  marketplaceAdopt({
                    tenant,
                    source: { id: legacyId, version: legacyVersion },
                    idempotency_key: crypto.randomUUID(),
                  }),
                )
              }
            >
              {copy.adopt}
            </button>
          </details>
        )}
      </Panel>
    </>
  );
}
