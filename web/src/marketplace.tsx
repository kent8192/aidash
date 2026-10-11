import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ShieldCheck } from "lucide-react";
import {
  marketplaceBrowse,
  marketplaceDetail,
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
  marketplaceActivate,
  marketplaceAdopt,
  authorizationCatalog,
} from "./generated/aidash";
import type {
  MarketplaceAudience,
  MarketplaceInstallationRevision,
  MarketplaceDependencyBinding,
  Entry,
} from "./generated/models";
import {
  ApiError,
  apiFetch,
  authenticatedFetch,
  dashboardContext,
} from "./transport";
import { Badge as StatusBadge } from "./components/ui/badge";
import { Button } from "./components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./components/ui/dialog";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./components/ui/table";
import { Textarea } from "./components/ui/textarea";
import { cn } from "./lib/utils";
import { useI18n, Panel, Field, type StatusTone } from "./ui";
import { RecordView } from "./record-view";
import { marketplaceCopy } from "./marketplace-copy";
import { HostPackages } from "./host-packages";
import {
  Alert,
  Check,
  Disclosure,
  Facts,
  Hint,
  Notice,
  Pager,
  StateBadge,
} from "./components/patterns";

const json = (text: string): Record<string, unknown> => {
  const value: unknown = JSON.parse(text);
  if (!value || Array.isArray(value) || typeof value !== "object")
    throw new Error("JSON must be an object");
  return value as Record<string, unknown>;
};
const tenants = (text: string) => [
  ...new Set(text.split(/\s+/).filter(Boolean)),
];

async function pageOf<T>(
  url: string,
): Promise<{ items: T[]; nextOffset?: number }> {
  const response = await authenticatedFetch(url);
  const next = response.headers.get("x-aidash-next-offset");
  return {
    items: (await response.json()) as T[],
    nextOffset: next === null ? undefined : Number(next),
  };
}
const subheading = "text-xs font-semibold text-foreground";

type InstallState = "active" | "retained" | "revoked" | "changed" | "pending";
const installTone: Record<InstallState, StatusTone> = {
  active: "success",
  retained: "neutral",
  revoked: "danger",
  changed: "warning",
  pending: "warning",
};
function PageControls({
  offsets,
  next,
  busy,
  setOffsets,
  previous,
  forward,
}: {
  offsets: number[];
  next?: number;
  busy: boolean;
  setOffsets: (value: number[]) => void;
  previous: string;
  forward: string;
}) {
  return (
    <Pager
      page={offsets.length}
      previous={{
        disabled: busy || offsets.length === 1,
        go: () => setOffsets(offsets.slice(0, -1)),
        label: previous,
      }}
      next={{
        disabled: busy || next === undefined,
        go: () => setOffsets([...offsets, next!]),
        label: forward,
      }}
    />
  );
}
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
  const [installing, setInstalling] = useState(false);
  const [denied, setDenied] = useState(false);
  const [config, setConfig] = useState("{}");
  const [bindings, setBindings] = useState("[]");
  const [source, setSource] = useState("");
  const [sourceOffsets, setSourceOffsets] = useState([0]);
  const sourceOffset = sourceOffsets.at(-1)!;
  const [packageId, setPackageId] = useState("");
  const [author, setAuthor] = useState("");
  const [audience, setAudience] = useState<string>();
  const [redistributor, setRedistributor] = useState("");
  const [consentDraft, setConsentDraft] = useState<{
    key: string;
    revision: number;
    audience: string;
  }>();
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
    queryKey: [...context, "sources", sourceOffset],
    queryFn: () =>
      pageOf<Entry>(`/api/marketplace/sources?offset=${sourceOffset}&limit=50`),
    enabled: !denied,
    retry: false,
    staleTime: 0,
  });
  const detail = useQuery({
    queryKey: [...context, "detail", selected],
    queryFn: () => marketplaceDetail(selected!),
    enabled: !!selected && !denied,
    retry: false,
    staleTime: 0,
  });
  const consentQuery = useQuery({
    queryKey: [...context, "consent", selected, redistributor],
    queryFn: () =>
      apiFetch<MarketplaceAudience>(
        `/api/marketplace/packages/${encodeURIComponent(selected!)}/consents/${encodeURIComponent(redistributor)}`,
      ),
    enabled: !!selected && !!redistributor && !denied,
    retry: false,
    staleTime: 0,
  });
  const consent =
    !consentQuery.isFetching && !consentQuery.isError
      ? consentQuery.data
      : undefined;
  const consentKey = `${selected}:${redistributor}:${consentQuery.dataUpdatedAt}`;
  const currentDraft =
    consent && consentDraft?.key === consentKey ? consentDraft : undefined;
  const consentRevision = currentDraft?.revision ?? consent?.revision ?? 0;
  const consentAudience =
    currentDraft?.audience ?? (consent ? [...consent.tenants].join(" ") : "");
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
  const chosenSource = sources.data?.items.find(
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
    consentQuery.error,
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
    setInstalling(false);
    setSelected(undefined);
    setEditing(undefined);
    setSource("");
    setConfig("{}");
    setBindings("[]");
    setConsentDraft(undefined);
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
        setInstalling(false);
        setSelected(undefined);
        setEditing(undefined);
        setSource("");
        setConfig("{}");
        setBindings("[]");
        setConsentDraft(undefined);
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
  const scopedState = (item: MarketplaceInstallationRevision): InstallState =>
    item.approved && item.installation.active_revision === item.revision
      ? "active"
      : item.approved
        ? "retained"
        : item.installation.active_revision === item.revision
          ? "revoked"
          : item.installation.active_revision
            ? "changed"
            : "pending";
  if (denied)
    return (
      <Panel title={copy.packages}>
        <Alert
          retry={() => {
            setDenied(false);
            setMessage("");
          }}
          retryLabel={copy.retry}
        >
          {copy.unavailable}
        </Alert>
      </Panel>
    );
  const view = selected ? detail.data : undefined;
  return (
    <div className="grid min-w-0 gap-6">
      {message && <Notice role="status">{message}</Notice>}
      <div
        className={cn(
          "grid min-w-0 items-start gap-6",
          view && "xl:grid-cols-[minmax(0,1fr)_minmax(320px,380px)]",
        )}
      >
        <Panel
          title={copy.packages}
          action={
            packages.data && (
              <span className="font-mono text-[11px] text-faint tabular">
                {packages.data.length}
              </span>
            )
          }
        >
          <div className="max-w-sm">
            <Field label={copy.search}>
              <Input
                type="search"
                value={search}
                onChange={(event) => {
                  setSearch(event.target.value);
                  setSelected(undefined);
                }}
              />
            </Field>
          </div>
          {packages.isError && <Alert>{copy.unavailable}</Alert>}
          {!packages.isPending &&
            !packages.isError &&
            !packages.data?.length && <Hint>{copy.empty}</Hint>}
          {!!packages.data?.length && (
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead>{copy.colPackage}</TableHead>
                  <TableHead className="hidden md:table-cell">
                    {copy.colKind}
                  </TableHead>
                  <TableHead className="hidden sm:table-cell">
                    {copy.colVersion}
                  </TableHead>
                  <TableHead className="hidden lg:table-cell">
                    {copy.colPublisher}
                  </TableHead>
                  <TableHead className="hidden lg:table-cell">
                    {copy.colPermissions}
                  </TableHead>
                  <TableHead className="w-0" />
                </TableRow>
              </TableHeader>
              <TableBody>
                {packages.data.map((item) => (
                  <TableRow
                    key={item.key}
                    data-state={item.key === selected ? "selected" : undefined}
                  >
                    <TableCell className="py-2">
                      <div className="max-w-48 truncate font-medium text-foreground sm:max-w-72">
                        {local(item.name)}
                      </div>
                      <div className="max-w-48 truncate text-xs text-muted-foreground sm:max-w-72">
                        {local(item.description)}
                      </div>
                    </TableCell>
                    <TableCell className="hidden md:table-cell">
                      <StatusBadge tone="neutral">{item.kind}</StatusBadge>
                    </TableCell>
                    <TableCell className="hidden font-mono text-xs sm:table-cell">
                      {item.version}
                    </TableCell>
                    <TableCell className="hidden max-w-48 text-xs text-muted-foreground lg:table-cell">
                      <div className="truncate">{item.author}</div>
                      <div className="truncate font-mono text-[11px] text-faint">
                        {item.owner_tenant}
                      </div>
                    </TableCell>
                    <TableCell className="hidden max-w-56 text-xs lg:table-cell">
                      <span
                        className={cn(
                          "line-clamp-2",
                          item.permissions.length
                            ? "text-foreground"
                            : "text-faint",
                        )}
                      >
                        {item.permissions.length
                          ? item.permissions.join(", ")
                          : copy.noPermissions}
                      </span>
                    </TableCell>
                    <TableCell className="text-right">
                      {item.actions.includes("read") ? (
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => {
                            setSelected(item.key);
                            setAudience(undefined);
                            setEditing(undefined);
                            setConfig("{}");
                            setBindings("[]");
                          }}
                        >
                          {copy.details}
                        </Button>
                      ) : (
                        <span className="text-xs text-faint">
                          {copy.unavailable}
                        </span>
                      )}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </Panel>
        {selected && view && (
          <aside className="min-w-0 xl:border-l xl:border-border xl:pl-6">
            <Panel
              title={local(view.summary.name)}
              action={
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelected(undefined)}
                >
                  {copy.close}
                </Button>
              }
            >
              <Hint>{local(view.summary.description)}</Hint>
              <Facts
                items={[
                  [
                    copy.colKind,
                    <StatusBadge key="kind" tone="neutral">
                      {view.summary.kind}
                    </StatusBadge>,
                  ],
                  [copy.colVersion, view.summary.version, true],
                  [
                    copy.colPublisher,
                    <>
                      {view.summary.author}{" "}
                      <span className="font-mono text-faint">
                        {view.summary.owner_tenant}
                      </span>
                    </>,
                  ],
                  [copy.digest, view.summary.digest, true],
                ]}
              />
              <div className="grid gap-1.5">
                <h3 className={subheading}>{copy.permission}</h3>
                <PermissionList
                  permissions={view.manifest.permissions}
                  empty={copy.noPermissions}
                />
              </div>
              {view.summary.capabilities.length > 0 && (
                <div className="grid gap-1.5">
                  <h3 className={subheading}>{copy.capabilities}</h3>
                  <div className="flex flex-wrap gap-1">
                    {view.summary.capabilities.map((capability) => (
                      <StatusBadge key={capability} variant="secondary">
                        {capability}
                      </StatusBadge>
                    ))}
                  </div>
                </div>
              )}
              <Hint>{copy.dependencies}</Hint>
              <Disclosure summary={copy.manifest}>
                <RecordView value={view.manifest} />
              </Disclosure>
              {view.summary.actions.includes("install") && (
                <div>
                  <Button onClick={() => setInstalling(true)}>
                    <ShieldCheck aria-hidden />
                    {copy.reviewInstall}
                  </Button>
                </div>
              )}
              {view.summary.actions.includes("share") && (
                <div className="grid gap-2 border-t border-border pt-3">
                  <h3 className={subheading}>{copy.sharing}</h3>
                  <Field label={copy.tenants}>
                    <Textarea
                      className="min-h-16 font-mono text-xs"
                      value={audience ?? view.audience.tenants.join("\n")}
                      onChange={(e) => setAudience(e.target.value)}
                    />
                  </Field>
                  <div>
                    <Button
                      variant="outline"
                      disabled={busy}
                      onClick={() =>
                        void run(() =>
                          marketplaceShare(selected, {
                            expected_revision: detail.data!.audience.revision,
                            tenants: [
                              ...new Set([
                                detail.data!.summary.owner_tenant,
                                ...tenants(
                                  audience ??
                                    detail.data!.audience.tenants.join("\n"),
                                ),
                              ]),
                            ],
                          }),
                        )
                      }
                    >
                      {copy.share}
                    </Button>
                  </div>
                </div>
              )}
              {view.summary.actions.includes("consent") && (
                <Disclosure
                  className="border-t border-border pt-3"
                  summary={copy.consent}
                >
                  <Hint>{copy.consentHelp}</Hint>
                  <Field label={copy.redistributor}>
                    <Input
                      value={redistributor}
                      onChange={(e) => setRedistributor(e.target.value)}
                    />
                  </Field>
                  <Field label={copy.consentRevision}>
                    <Input
                      type="number"
                      min={0}
                      className="font-mono tabular"
                      value={consentRevision}
                      onChange={(e) =>
                        setConsentDraft({
                          key: consentKey,
                          revision: Number(e.target.value),
                          audience: consentAudience,
                        })
                      }
                    />
                  </Field>
                  <Field label={copy.tenants}>
                    <Textarea
                      className="min-h-16 font-mono text-xs"
                      value={consentAudience}
                      onChange={(e) =>
                        setConsentDraft({
                          key: consentKey,
                          revision: consentRevision,
                          audience: e.target.value,
                        })
                      }
                    />
                  </Field>
                  <div>
                    <Button
                      variant="outline"
                      disabled={busy || !redistributor || !consent}
                      onClick={() =>
                        void run(async () => {
                          await marketplaceConsent(selected, redistributor, {
                            expected_revision: consentRevision,
                            tenants: tenants(consentAudience),
                          });
                        })
                      }
                    >
                      {copy.consent}
                    </Button>
                  </div>
                </Disclosure>
              )}
            </Panel>
            <Dialog
              open={installing}
              onOpenChange={(open) => {
                if (!open && !busy) setInstalling(false);
              }}
            >
              <DialogContent className="max-w-xl" closeLabel={copy.close}>
                <DialogHeader>
                  <DialogTitle>{copy.install}</DialogTitle>
                  <DialogDescription className="text-xs">
                    {copy.installHelp}
                  </DialogDescription>
                </DialogHeader>
                <div className="grid gap-1 rounded-md border border-border bg-surface px-3 py-2.5">
                  <div className="flex flex-wrap items-baseline gap-x-2">
                    <span className="font-medium">
                      {local(view.summary.name)}
                    </span>
                    <span className="font-mono text-xs text-muted-foreground">
                      {view.summary.version}
                    </span>
                  </div>
                  <div className="break-all font-mono text-[11px] text-faint">
                    {copy.digest}: {view.summary.digest}
                  </div>
                </div>
                <div className="grid gap-1.5">
                  <h3 className={subheading}>{copy.permission}</h3>
                  <PermissionList
                    permissions={view.manifest.permissions}
                    empty={copy.noPermissions}
                  />
                </div>
                <Hint>{copy.dependencies}</Hint>
                <Field label={copy.config}>
                  <Textarea
                    className="min-h-20 font-mono text-xs"
                    value={config}
                    onChange={(e) => setConfig(e.target.value)}
                  />
                </Field>
                <Field label={copy.bindings}>
                  <Textarea
                    className="min-h-20 font-mono text-xs"
                    value={bindings}
                    onChange={(e) => setBindings(e.target.value)}
                  />
                </Field>
                <DialogFooter>
                  <Button
                    variant="ghost"
                    disabled={busy}
                    onClick={() => setInstalling(false)}
                  >
                    {copy.cancel}
                  </Button>
                  <Button
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
                        setInstalling(false);
                      }, copy.installed)
                    }
                  >
                    {copy.install}
                  </Button>
                </DialogFooter>
              </DialogContent>
            </Dialog>
          </aside>
        )}
      </div>
      <Panel title={copy.installations}>
        <Hint>{copy.pinned}</Hint>
        {!!installs.data?.length && (
          <ul className="divide-y divide-border border-y border-border">
            {installs.data.map((item) => {
              const state = scopedState(item);
              return (
                <li
                  className="flex flex-wrap items-center gap-x-4 gap-y-2 py-2.5"
                  key={item.installation.id}
                >
                  <div className="min-w-0 flex-1">
                    <h3 className="truncate text-[13px] font-medium text-foreground">
                      {local(item.entry.name)}
                    </h3>
                    <p className="font-mono text-[11px] text-faint tabular">
                      {copy.revision} {item.revision} · {copy.activeRevision}{" "}
                      {item.installation.active_revision ?? "—"}
                    </p>
                  </div>
                  <StateBadge tone={installTone[state]}>
                    {copy[state]}
                  </StateBadge>
                  <div className="flex flex-wrap gap-2">
                    {item.installation.active_revision &&
                      item.installation.active_revision !== item.revision && (
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() =>
                            void run(async () => {
                              const active = await marketplaceInstallation(
                                item.installation.id,
                                {
                                  revision: item.installation.active_revision!,
                                },
                              );
                              setEditing(active);
                              setConfig(JSON.stringify(active.config, null, 2));
                              setBindings(
                                JSON.stringify(active.bindings, null, 2),
                              );
                            })
                          }
                        >
                          {copy.active}
                        </Button>
                      )}
                    {item.actions.includes("configure") && (
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={() => {
                          setEditing(item);
                          setSelected(undefined);
                          setConfig(JSON.stringify(item.config, null, 2));
                          setBindings(JSON.stringify(item.bindings, null, 2));
                        }}
                      >
                        {copy.configure}
                      </Button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
        {editingView && (
          <div className="grid gap-3 rounded-lg border border-border bg-surface p-4">
            <h3 className="text-[13px] font-semibold text-foreground">
              {local(editingView.entry.name)}{" "}
              <span className="font-mono text-xs font-normal text-muted-foreground">
                · {copy.revision} {editingView.revision}
              </span>
            </h3>
            <Field label={copy.config}>
              <Textarea
                className="min-h-24 font-mono text-xs"
                value={config}
                onChange={(e) => setConfig(e.target.value)}
              />
            </Field>
            <Field label={copy.bindings}>
              <Textarea
                className="min-h-20 font-mono text-xs"
                value={bindings}
                onChange={(e) => setBindings(e.target.value)}
              />
            </Field>
            <div className="flex flex-wrap gap-2">
              {editingView.actions.includes("configure") && (
                <Button
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
                </Button>
              )}
              <Button variant="ghost" onClick={() => setEditing(undefined)}>
                {copy.close}
              </Button>
            </div>
          </div>
        )}
      </Panel>
      {sources.data &&
        (sources.data.items.length > 0 ||
          sourceOffset > 0 ||
          sources.data.nextOffset !== undefined) && (
          <Panel
            title={copy.publish}
            action={
              <PageControls
                offsets={sourceOffsets}
                next={sources.data.nextOffset}
                busy={busy || sources.isFetching}
                setOffsets={(offsets) => {
                  setSourceOffsets(offsets);
                  setSource("");
                  setPackageId("");
                }}
                previous={copy.previous}
                forward={copy.next}
              />
            }
          >
            <Hint>{copy.publishHelp}</Hint>
            <div className="grid max-w-3xl gap-3 sm:grid-cols-3">
              <Field label={copy.source}>
                <NativeSelect
                  value={source}
                  onChange={(e) => {
                    setSource(e.target.value);
                    const item = sources.data?.items.find(
                      (s) => `${s.id}@${s.version}` === e.target.value,
                    );
                    setPackageId(item?.id ?? "");
                  }}
                >
                  <option value="">—</option>
                  {sources.data.items.map((item) => (
                    <option
                      key={`${item.id}@${item.version}`}
                      value={`${item.id}@${item.version}`}
                    >
                      {local(item.name)} · {item.version}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
              <Field label={copy.packageId}>
                <Input
                  className="font-mono"
                  value={packageId}
                  onChange={(e) => setPackageId(e.target.value)}
                />
              </Field>
              <Field label={copy.author}>
                <Input
                  value={author}
                  onChange={(e) => setAuthor(e.target.value)}
                />
              </Field>
            </div>
            {canPublish.data?.allowed && (
              <p className="text-xs text-muted-foreground">
                {copy.publicationVersion}: {canPublish.data.version}
              </p>
            )}
            {canPublish.data?.allowed && (
              <div>
                <Button
                  disabled={busy || !source || !packageId || !author}
                  onClick={() =>
                    void run(async () => {
                      const entry = sources.data!.items.find(
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
                </Button>
              </div>
            )}
            {canPublish.data?.allowed === false && (
              <Hint>{copy.operationUnavailable}</Hint>
            )}
          </Panel>
        )}
    </div>
  );
}

function PermissionList({
  permissions,
  empty,
}: {
  permissions: string[];
  empty: string;
}) {
  return permissions.length ? (
    <ul className="grid gap-1">
      {permissions.map((permission) => (
        <li
          key={permission}
          className="flex items-center gap-2 font-mono text-xs text-foreground"
        >
          <span aria-hidden className="size-1.5 rounded-full bg-warning" />
          {permission}
        </li>
      ))}
    </ul>
  ) : (
    <p className="text-xs text-faint">{empty}</p>
  );
}

export function MarketplaceAdministration() {
  const { locale, local } = useI18n();
  const copy = marketplaceCopy[locale];
  const cache = useQueryClient();
  const [tenant, setTenant] = useState("");
  const [draft, setDraft] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [revisionOffsets, setRevisionOffsets] = useState([0]);
  const revisionOffset = revisionOffsets.at(-1)!;
  const [message, setMessage] = useState("");
  const [legacyId, setLegacyId] = useState("");
  const [legacyVersion, setLegacyVersion] = useState("1.0.0");
  const gate = useQuery({
    queryKey: ["marketplace", "operator", "compatibility"],
    queryFn: () => marketplaceCompatibility(),
    retry: false,
  });
  const installs = useQuery({
    queryKey: ["marketplace", "operator", tenant, revisionOffset],
    queryFn: () =>
      pageOf<MarketplaceInstallationRevision>(
        `/api/marketplace/administration?tenant=${encodeURIComponent(tenant)}&offset=${revisionOffset}&limit=50`,
      ),
    enabled: !!tenant,
    retry: false,
    staleTime: 0,
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
  const adminState = (item: MarketplaceInstallationRevision): InstallState =>
    item.approved
      ? item.installation.active_revision === item.revision
        ? "active"
        : "retained"
      : item.installation.active_revision === item.revision
        ? "revoked"
        : item.revision > 1
          ? "changed"
          : "pending";
  return (
    <div className="grid min-w-0 gap-6">
      {message && <Notice role="status">{message}</Notice>}
      {gate.data && (
        <Panel
          title={copy.rollout}
          action={
            <StateBadge tone={gate.data.enabled ? "success" : "neutral"}>
              {gate.data.enabled ? copy.enabled : copy.disabled}
            </StateBadge>
          }
        >
          <Check
            checked={confirmed}
            onChange={(e) => setConfirmed(e.target.checked)}
          >
            {copy.compatible}
          </Check>
          <div>
            <Button
              variant="outline"
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
            </Button>
          </div>
        </Panel>
      )}
      <Panel title={copy.admin}>
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            setTenant(draft.trim());
            setRevisionOffsets([0]);
            void cache.invalidateQueries({
              queryKey: ["marketplace", "operator", draft.trim()],
            });
          }}
        >
          <div className="w-full max-w-xs">
            <Field label={copy.tenant}>
              <Input
                className="font-mono"
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
              />
            </Field>
          </div>
          <Button type="submit" variant="outline">
            {copy.load}
          </Button>
        </form>
        <Hint>{copy.pinned}</Hint>
        {installs.isError && <Alert>{copy.unavailable}</Alert>}
        {tenant && !installs.isError && (
          <HostPackages
            key={tenant}
            tenant={tenant}
            revisions={installs.data?.items ?? []}
            onSaved={async () => {
              await cache.invalidateQueries({ queryKey: ["marketplace"] });
            }}
          />
        )}
        {!installs.isError && !!installs.data?.items.length && (
          <ul className="divide-y divide-border border-y border-border">
            {installs.data.items.map((item) => {
              const state = adminState(item);
              return (
                <li
                  className="grid gap-2 py-2.5"
                  key={`${item.installation.id}:${item.revision}`}
                >
                  <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
                    <div className="min-w-0 flex-1">
                      <h3 className="truncate text-[13px] font-medium text-foreground">
                        {local(item.entry.name)} · {copy.revision}{" "}
                        {item.revision}
                      </h3>
                      <p className="truncate font-mono text-[11px] text-faint">
                        {item.entry.id}@{item.entry.version}
                      </p>
                    </div>
                    <StateBadge tone={installTone[state]}>
                      {copy[state]}
                    </StateBadge>
                    <div className="flex flex-wrap gap-2">
                      {[true, false].map((enabled) => (
                        <Button
                          variant={enabled ? "outline" : "ghost"}
                          size="sm"
                          key={String(enabled)}
                          disabled={!enabled && !item.approved}
                          onClick={() =>
                            void run(async () => {
                              const catalog =
                                await authorizationCatalog(tenant);
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
                                expected_catalog_revision:
                                  binding?.revision ?? 0,
                                enabled,
                              });
                            })
                          }
                        >
                          {enabled ? copy.approve : copy.revoke}
                        </Button>
                      ))}
                    </div>
                  </div>
                  <Disclosure summary={copy.details}>
                    <RecordView value={item.entry} />
                  </Disclosure>
                </li>
              );
            })}
          </ul>
        )}
        {installs.data && !installs.isError && (
          <PageControls
            offsets={revisionOffsets}
            next={installs.data.nextOffset}
            busy={installs.isFetching}
            setOffsets={setRevisionOffsets}
            previous={copy.previous}
            forward={copy.next}
          />
        )}
        {tenant && (
          <Disclosure
            className="border-t border-border pt-3"
            summary={copy.adoption}
          >
            <div className="grid max-w-xl gap-3">
              <Hint>{copy.adoptionHelp}</Hint>
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label={copy.legacyId}>
                  <Input
                    className="font-mono"
                    value={legacyId}
                    onChange={(e) => setLegacyId(e.target.value)}
                  />
                </Field>
                <Field label={copy.legacyVersion}>
                  <Input
                    className="font-mono"
                    value={legacyVersion}
                    onChange={(e) => setLegacyVersion(e.target.value)}
                  />
                </Field>
              </div>
              <div>
                <Button
                  variant="outline"
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
                </Button>
              </div>
            </div>
          </Disclosure>
        )}
      </Panel>
    </div>
  );
}
