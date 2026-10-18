import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { useState } from "react";
import type { ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, authenticatedFetch } from "./transport";
import { Panel, useI18n } from "./ui";
import { Alert, GroupChoice, Hint, Pager } from "./components/patterns";
import { TenantInvitations, TenantLabelForm } from "./tenancy";
import {
  MEMBER_LEVEL_ACTIONS,
  useTenantAdministration,
} from "./tenant-administration";

type Registration = {
  id: string;
  identity_id: string;
  status: string;
  created_at: string;
  expires_at: string;
  /** The Tenant the request is addressed to; absent when only Deployment Administrators decide it. */
  tenant?: string | null;
  /** The Invitation the request was made with, if any. */
  invitation_id?: string | null;
  /** The groups that Invitation proposed. */
  proposed_groups?: string[];
};
type Identity = {
  id: string;
  issuer: string;
  subject: string;
  gcip_tenant: string | null;
  verified_email: string | null;
  display_name: string | null;
  disabled_at: string | null;
  display_erased_at: string | null;
};
type Mapping = {
  id: string;
  identity_id: string;
  tenant: string;
  subject: string;
  enabled: boolean;
  revision: number;
};
type Grant = { identity_id: string; enabled: boolean; revision: number };
/** The server's page size for Mappings and Registration Requests. */
const PAGE_SIZE = 200;

/** The distinct, non-empty groups named in a comma-separated field. */
function approvalGroups(text: string): string[] {
  return [
    ...new Set(
      text
        .split(",")
        .map((group) => group.trim())
        .filter(Boolean),
    ),
  ];
}

export function DashboardIdentityAdministration() {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const client = useQueryClient();
  const [tenant, setTenant] = useState("");
  const [subject, setSubject] = useState("");
  // Groups for a subject the approval creates, per request. They start from
  // what the request's Invitation proposed, and otherwise from none: any
  // group, `administrators` included, is the reviewer's explicit choice.
  const [groups, setGroups] = useState<Record<string, string>>({});
  const groupsOf = (request: Registration) =>
    groups[request.id] ?? (request.proposed_groups ?? []).join(", ");
  const [mappingOffset, setMappingOffset] = useState(0);
  const [registrationOffset, setRegistrationOffset] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const registrations = useQuery({
    queryKey: ["dashboard-registrations", registrationOffset],
    queryFn: () =>
      apiFetch<Registration[]>(
        `/api/dashboard/registrations?offset=${registrationOffset}`,
      ),
  });
  const identities = useQuery({
    queryKey: ["dashboard-identities"],
    queryFn: async () => {
      const all: Identity[] = [];
      for (let offset = 0; ; offset += 200) {
        const page = await apiFetch<Identity[]>(
          `/api/dashboard/identities?offset=${offset}`,
        );
        all.push(...page);
        if (page.length < 200) return all;
      }
    },
  });
  const mappings = useQuery({
    queryKey: ["dashboard-mappings", mappingOffset],
    queryFn: () =>
      apiFetch<Mapping[]>(`/api/dashboard/mappings?offset=${mappingOffset}`),
  });
  const grants = useQuery({
    queryKey: ["dashboard-deployment-administrator-grants"],
    queryFn: () =>
      apiFetch<Grant[]>("/api/dashboard/deployment-administrator-grants"),
  });
  const identity = (id: string) =>
    identities.data?.find((item) => item.id === id);
  const act = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
      await client.invalidateQueries({ queryKey: ["dashboard-registrations"] });
      await client.invalidateQueries({ queryKey: ["dashboard-identities"] });
      await client.invalidateQueries({ queryKey: ["dashboard-mappings"] });
      await client.invalidateQueries({
        queryKey: ["dashboard-deployment-administrator-grants"],
      });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      await client.invalidateQueries({
        queryKey: ["dashboard-deployment-administrator-grants"],
      });
    } finally {
      setBusy(false);
    }
  };
  const field = "grid gap-1 text-xs font-medium text-muted-foreground";
  const row = "flex flex-wrap items-center gap-x-3 gap-y-2 py-2.5";
  const group = "text-[11px] font-medium text-faint";
  return (
    <Panel title={english ? "Dashboard identities" : "ダッシュボード ID"}>
      <Hint>
        {english
          ? "Approve a verified identity only for an existing user subject. Deployment Administrator access is separate."
          : "確認済みの外部 ID を既存の user subject に紐づけます。デプロイメント管理者権限は別に設定します。"}
      </Hint>
      {error && <Alert>{error}</Alert>}
      <ul className="divide-y divide-border">
        {(registrations.data ?? [])
          .filter((request) => request.status === "pending")
          .map((request) => (
            <li
              key={request.id}
              className="grid gap-3 py-3 md:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)]"
            >
              <div className="grid min-w-0 content-start gap-0.5">
                <strong className="truncate font-mono text-[13px] font-medium">
                  {identity(request.identity_id)?.subject ??
                    request.identity_id}
                </strong>
                <p className="truncate font-mono text-[11px] text-faint">
                  {identity(request.identity_id)?.issuer}
                </p>
                <p className="truncate font-mono text-[11px] text-faint">
                  {identity(request.identity_id)?.gcip_tenant}
                </p>
                <p className="truncate text-xs text-muted-foreground">
                  {[
                    identity(request.identity_id)?.display_name,
                    identity(request.identity_id)?.verified_email,
                  ]
                    .filter(Boolean)
                    .join(" · ")}
                </p>
                {request.tenant && (
                  <p className="truncate font-mono text-[11px] text-faint">
                    {english ? "Addressed to" : "申請先"}: {request.tenant}
                  </p>
                )}
                {request.invitation_id && (
                  <p className="truncate text-[11px] text-faint">
                    {english ? "Invited into" : "招待時のグループ"}:{" "}
                    {(request.proposed_groups ?? []).join(", ") || "—"}
                  </p>
                )}
                <small className="text-[11px] text-faint">
                  {english ? "Expires" : "期限"}:{" "}
                  <span className="font-mono tabular">
                    {new Date(request.expires_at).toLocaleString(locale)}
                  </span>
                </small>
              </div>
              <div className="grid content-start gap-2 sm:grid-cols-2">
                <label className={field}>
                  {english ? "Tenant" : "テナント"}
                  <Input
                    value={request.tenant ?? tenant}
                    disabled={Boolean(request.tenant)}
                    onChange={(event) => setTenant(event.target.value)}
                  />
                </label>
                <label className={field}>
                  {english ? "User subject" : "user subject"}
                  <Input
                    value={subject}
                    onChange={(event) => setSubject(event.target.value)}
                  />
                </label>
                <label className={field}>
                  {english
                    ? "Groups, if the subject is new (comma-separated)"
                    : "subject が新しい場合のグループ（カンマ区切り）"}
                  <Input
                    value={groupsOf(request)}
                    onChange={(event) =>
                      setGroups({ ...groups, [request.id]: event.target.value })
                    }
                  />
                </label>
                <div className="flex gap-2 sm:col-span-2">
                  <Button
                    size="sm"
                    disabled={busy || !(request.tenant ?? tenant) || !subject}
                    onClick={() => {
                      void act(() =>
                        apiFetch(
                          `/api/dashboard/registrations/${request.id}/approve`,
                          {
                            method: "POST",
                            headers: { "content-type": "application/json" },
                            body: JSON.stringify({
                              tenant: request.tenant ?? tenant,
                              subject,
                              // Used only when the subject does not exist yet.
                              ...(approvalGroups(groupsOf(request)).length
                                ? { groups: approvalGroups(groupsOf(request)) }
                                : {}),
                            }),
                          },
                        ),
                      );
                    }}
                  >
                    {english ? "Approve mapping" : "紐づけを承認"}
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onClick={() => {
                      void act(() =>
                        apiFetch(
                          `/api/dashboard/registrations/${request.id}/reject`,
                          { method: "POST" },
                        ),
                      );
                    }}
                  >
                    {english ? "Reject" : "却下"}
                  </Button>
                </div>
              </div>
            </li>
          ))}
      </ul>
      <Pager
        className="justify-start"
        page={registrationOffset / PAGE_SIZE + 1}
        previous={{
          disabled: registrationOffset === 0 || registrations.isFetching,
          go: () =>
            setRegistrationOffset(Math.max(0, registrationOffset - PAGE_SIZE)),
        }}
        next={{
          disabled:
            (registrations.data?.length ?? 0) < PAGE_SIZE ||
            registrations.isFetching,
          go: () => setRegistrationOffset(registrationOffset + PAGE_SIZE),
        }}
      />
      <h3 className={group}>{english ? "Mappings" : "紐づけ"}</h3>
      <ul className="divide-y divide-border border-y border-border">
        {(mappings.data ?? []).map((mapping) => (
          <li className={row} key={mapping.id}>
            <span className="min-w-0 flex-1 truncate font-mono text-xs">
              {identity(mapping.identity_id)?.subject ?? mapping.identity_id}{" "}
              <span className="text-faint">→</span> {mapping.tenant} /{" "}
              {mapping.subject}
            </span>
            {mapping.enabled && (
              <Button
                variant="outline"
                size="sm"
                disabled={busy}
                onClick={() => {
                  void act(() =>
                    authenticatedFetch(
                      `/api/dashboard/mappings/${mapping.id}/disable`,
                      {
                        method: "POST",
                        headers: { "content-type": "application/json" },
                        body: JSON.stringify({
                          expected_revision: mapping.revision,
                        }),
                      },
                    ),
                  );
                }}
              >
                {english ? "Disable" : "無効化"}
              </Button>
            )}
          </li>
        ))}
      </ul>
      <Pager
        className="justify-start"
        page={mappingOffset / PAGE_SIZE + 1}
        previous={{
          disabled: mappingOffset === 0 || mappings.isFetching,
          go: () => setMappingOffset(Math.max(0, mappingOffset - PAGE_SIZE)),
        }}
        next={{
          disabled:
            (mappings.data?.length ?? 0) < PAGE_SIZE || mappings.isFetching,
          go: () => setMappingOffset(mappingOffset + PAGE_SIZE),
        }}
      />
      <h3 className={group}>
        {english
          ? "Deployment Administrator grants"
          : "デプロイメント管理者権限"}
      </h3>
      <ul className="divide-y divide-border border-y border-border">
        {(identities.data ?? []).map((item) => {
          const grant = grants.data?.find(
            (entry) => entry.identity_id === item.id,
          );
          const enabled = grant?.enabled ?? false;
          return (
            <li className={row} key={item.id}>
              <span className="grid min-w-0 flex-1 gap-0.5">
                <span className="truncate font-mono text-xs">
                  {item.subject} ({item.issuer})
                  {item.gcip_tenant && ` / ${item.gcip_tenant}`}
                </span>
                {(item.display_name || item.verified_email) && (
                  <span className="truncate text-xs text-muted-foreground">
                    {[item.display_name, item.verified_email]
                      .filter(Boolean)
                      .join(" · ")}
                  </span>
                )}
                {item.display_erased_at && (
                  <span className="truncate text-[11px] text-faint">
                    {english
                      ? "Display attributes erased"
                      : "表示属性を消去済み"}
                    :{" "}
                    <span className="font-mono tabular">
                      {new Date(item.display_erased_at).toLocaleString(locale)}
                    </span>
                  </span>
                )}
              </span>
              {item.disabled_at && (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() => {
                    void act(async () => {
                      await authenticatedFetch(
                        `/api/dashboard/identities/${item.id}/restore`,
                        { method: "POST" },
                      );
                      await client.invalidateQueries({
                        queryKey: ["dashboard-identities"],
                      });
                    });
                  }}
                >
                  {english
                    ? "Verify and restore identity"
                    : "外部 ID を確認して復旧"}
                </Button>
              )}
              {!item.display_erased_at && (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() => {
                    if (
                      !window.confirm(
                        english
                          ? "Permanently erase this identity's email and display name? Later sign-ins will not record them again."
                          : "この外部 ID のメールアドレスと表示名を完全に消去しますか？以後のサインインでも再取得しません。",
                      )
                    )
                      return;
                    void act(() =>
                      authenticatedFetch(
                        `/api/dashboard/identities/${item.id}/display-erasure`,
                        { method: "POST" },
                      ),
                    );
                  }}
                >
                  {english ? "Erase display attributes" : "表示属性を消去"}
                </Button>
              )}
              <Button
                variant={enabled ? "destructive" : "outline"}
                size="sm"
                disabled={busy || (item.disabled_at !== null && !enabled)}
                onClick={() => {
                  void act(() =>
                    authenticatedFetch(
                      `/api/dashboard/identities/${item.id}/deployment-administrator-grant`,
                      {
                        method: "POST",
                        headers: { "content-type": "application/json" },
                        body: JSON.stringify({
                          enabled: !enabled,
                          expected_revision: grant?.revision ?? 0,
                        }),
                      },
                    ),
                  );
                }}
              >
                {enabled
                  ? english
                    ? "Revoke Deployment Administrator"
                    : "デプロイメント管理者権限を解除"
                  : english
                    ? "Grant Deployment Administrator"
                    : "デプロイメント管理者権限を付与"}
              </Button>
            </li>
          );
        })}
      </ul>
    </Panel>
  );
}

type TenantIdentity = {
  id: string;
  verified_email: string | null;
  display_name: string | null;
};
type TenantRegistration = {
  id: string;
  identity: TenantIdentity;
  expires_at: string;
  /** The Assignable Groups the request's Invitation proposed. */
  proposed_groups: string[];
};
type TenantMapping = {
  id: string;
  identity: TenantIdentity;
  subject: string;
  enabled: boolean;
  revision: number;
  groups: string[];
  /** The caller's own External Identity holds this Mapping. */
  own_identity: boolean;
  /** The caller acts as, or was ever mapped to, this subject. */
  own_subject: boolean;
};

function person(identity: TenantIdentity) {
  return (
    [identity.display_name, identity.verified_email]
      .filter(Boolean)
      .join(" · ") || identity.id
  );
}

/** The Access and accounts screen for a Tenant Administrator's own Tenant.
 *  `fallback` replaces it for members who hold no administrator action.
 *  The server refuses to disable the caller's own Mappings or change its own
 *  subjects' groups, so neither control is offered for them. */
export function TenantIdentityAdministration({
  tenant,
  fallback,
}: {
  tenant: string;
  fallback: ReactNode;
}) {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const client = useQueryClient();
  const path = `/api/tenants/${encodeURIComponent(tenant)}`;
  const [subjects, setSubjects] = useState<Record<string, string>>({});
  const [groups, setGroups] = useState<Record<string, string[]>>({});
  const [mappingOffset, setMappingOffset] = useState(0);
  const [registrationOffset, setRegistrationOffset] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const overview = useTenantAdministration(tenant);
  const can = (action: string) =>
    overview.data?.actions.includes(action) ?? false;
  const registrations = useQuery({
    queryKey: ["tenant-registrations", tenant, registrationOffset],
    queryFn: () =>
      apiFetch<TenantRegistration[]>(
        `${path}/registrations?offset=${registrationOffset}`,
      ),
    enabled: can("registration.read"),
  });
  const mappings = useQuery({
    queryKey: ["tenant-mappings", tenant, mappingOffset],
    queryFn: () =>
      apiFetch<TenantMapping[]>(`${path}/mappings?offset=${mappingOffset}`),
    enabled: can("mapping.read"),
  });
  const act = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      await client.invalidateQueries({
        queryKey: ["tenant-administration", tenant],
      });
      await client.invalidateQueries({
        queryKey: ["tenant-registrations", tenant],
      });
      await client.invalidateQueries({ queryKey: ["tenant-mappings", tenant] });
      setBusy(false);
    }
  };
  if (
    overview.isError ||
    overview.data?.actions.every((action) =>
      MEMBER_LEVEL_ACTIONS.includes(action),
    )
  )
    return <>{fallback}</>;
  if (!overview.data) return null;
  const assignable = overview.data.assignable_groups;
  // An Invitation's proposal can name a group that is no longer assignable;
  // such a group is neither shown nor submitted with the approval.
  const chosen = (request: { id: string; proposed_groups: string[] }) =>
    (groups[request.id] ?? request.proposed_groups).filter((group) =>
      assignable.includes(group),
    );
  const revision = overview.data.policy_revision;
  const row = "flex flex-wrap items-center gap-x-3 gap-y-2 py-2.5";
  const heading = "text-[11px] font-medium text-faint";
  const field = "grid gap-1 text-xs font-medium text-muted-foreground";
  return (
    <Panel
      title={
        english
          ? `Tenant administration: ${overview.data.label ?? tenant}`
          : `テナント管理: ${overview.data.label ?? tenant}`
      }
    >
      <Hint>
        {english
          ? "Manage the people mapped into this Tenant. Invite people with a link, then approve their requests into new user subjects. You can assign only groups marked as assignable."
          : "このテナントに mapping された人を管理します。招待リンクで人を招き、届いた申請を新しい user subject として承認します。付与できるのは、割り当て可能な group だけです。"}
      </Hint>
      {error && <Alert>{error}</Alert>}
      <p className="font-mono text-[11px] text-faint">{tenant}</p>
      {can("tenant.update") && (
        <TenantLabelForm
          key={overview.data.label ?? ""}
          tenant={tenant}
          label={overview.data.label}
        />
      )}
      {(can("invitation.create") || can("invitation.read")) && (
        <TenantInvitations
          tenant={tenant}
          assignable={assignable}
          canCreate={can("invitation.create")}
          canRead={can("invitation.read")}
          canRevoke={can("invitation.revoke")}
        />
      )}
      {can("registration.read") && (
        <>
          <h3 className={heading}>
            {english ? "Registration requests" : "登録リクエスト"}
          </h3>
          <ul className="divide-y divide-border border-y border-border">
            {(registrations.data ?? []).map((request) => (
              <li
                key={request.id}
                className="grid gap-3 py-3 md:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)]"
              >
                <div className="grid min-w-0 content-start gap-0.5">
                  <strong className="truncate text-[13px] font-medium">
                    {person(request.identity)}
                  </strong>
                  <small className="text-[11px] text-faint">
                    {english ? "Expires" : "期限"}:{" "}
                    <span className="font-mono tabular">
                      {new Date(request.expires_at).toLocaleString(locale)}
                    </span>
                  </small>
                </div>
                <div className="grid content-start gap-2">
                  {can("registration.approve") && (
                    <>
                      <label className={field}>
                        {english ? "New user subject" : "新しい user subject"}
                        <Input
                          value={subjects[request.id] ?? ""}
                          onChange={(event) =>
                            setSubjects({
                              ...subjects,
                              [request.id]: event.target.value,
                            })
                          }
                        />
                      </label>
                      <GroupChoice
                        groups={assignable}
                        selected={chosen(request)}
                        disabled={busy}
                        change={(next) =>
                          setGroups({ ...groups, [request.id]: next })
                        }
                      />
                    </>
                  )}
                  <div className="flex gap-2">
                    {can("registration.approve") && (
                      <Button
                        size="sm"
                        disabled={busy || !subjects[request.id]}
                        onClick={() => {
                          void act(() =>
                            apiFetch(
                              `${path}/registrations/${request.id}/approve`,
                              {
                                method: "POST",
                                headers: { "content-type": "application/json" },
                                body: JSON.stringify({
                                  subject: subjects[request.id],
                                  groups: chosen(request),
                                }),
                              },
                            ),
                          );
                        }}
                      >
                        {english ? "Approve" : "承認"}
                      </Button>
                    )}
                    {can("registration.reject") && (
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={busy}
                        onClick={() => {
                          void act(() =>
                            apiFetch(
                              `${path}/registrations/${request.id}/reject`,
                              { method: "POST" },
                            ),
                          );
                        }}
                      >
                        {english ? "Reject" : "却下"}
                      </Button>
                    )}
                  </div>
                </div>
              </li>
            ))}
          </ul>
          <Pager
            className="justify-start"
            page={registrationOffset / PAGE_SIZE + 1}
            previous={{
              disabled: registrationOffset === 0 || registrations.isFetching,
              go: () =>
                setRegistrationOffset(
                  Math.max(0, registrationOffset - PAGE_SIZE),
                ),
            }}
            next={{
              disabled:
                (registrations.data?.length ?? 0) < PAGE_SIZE ||
                registrations.isFetching,
              go: () => setRegistrationOffset(registrationOffset + PAGE_SIZE),
            }}
          />
        </>
      )}
      {can("mapping.read") && (
        <>
          <h3 className={heading}>{english ? "Members" : "メンバー"}</h3>
          <ul className="divide-y divide-border border-y border-border">
            {(mappings.data ?? []).map((mapping) => (
              <li className={row} key={mapping.id}>
                <span className="grid min-w-0 flex-1 gap-0.5">
                  <span className="truncate text-xs">
                    {person(mapping.identity)}{" "}
                    <span className="text-faint">→</span>{" "}
                    <span className="font-mono">{mapping.subject}</span>
                  </span>
                  {!mapping.enabled && (
                    <span className="text-[11px] text-faint">
                      {english ? "Disabled" : "無効"}
                    </span>
                  )}
                </span>
                {can("subject.group.update") &&
                  revision !== null &&
                  !mapping.own_subject && (
                    <GroupChoice
                      groups={assignable}
                      selected={mapping.groups}
                      disabled={busy}
                      change={(next) => {
                        void act(() =>
                          apiFetch(
                            `${path}/subjects/${encodeURIComponent(mapping.subject)}/groups`,
                            {
                              method: "PUT",
                              headers: { "content-type": "application/json" },
                              body: JSON.stringify({
                                groups: next,
                                expected_policy_revision: revision,
                              }),
                            },
                          ),
                        );
                      }}
                    />
                  )}
                {can("mapping.disable") &&
                  mapping.enabled &&
                  !mapping.own_identity && (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={busy}
                      onClick={() => {
                        void act(() =>
                          authenticatedFetch(
                            `${path}/mappings/${mapping.id}/disable`,
                            {
                              method: "POST",
                              headers: { "content-type": "application/json" },
                              body: JSON.stringify({
                                expected_revision: mapping.revision,
                              }),
                            },
                          ),
                        );
                      }}
                    >
                      {english ? "Disable" : "無効化"}
                    </Button>
                  )}
              </li>
            ))}
          </ul>
          <Pager
            className="justify-start"
            page={mappingOffset / PAGE_SIZE + 1}
            previous={{
              disabled: mappingOffset === 0 || mappings.isFetching,
              go: () =>
                setMappingOffset(Math.max(0, mappingOffset - PAGE_SIZE)),
            }}
            next={{
              disabled:
                (mappings.data?.length ?? 0) < PAGE_SIZE || mappings.isFetching,
              go: () => setMappingOffset(mappingOffset + PAGE_SIZE),
            }}
          />
        </>
      )}
    </Panel>
  );
}
