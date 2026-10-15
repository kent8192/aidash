import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Alert, Hint } from "./components/patterns";
import { AuthCard } from "./shell/auth-card";
import {
  apiFetch,
  authenticatedFetch,
  sessionFetch,
  signIn,
} from "./transport";
import { useI18n } from "./ui";

/** A Tenant the creation API made; `mapping_id` is the Tenant Creator's Mapping. */
export type CreatedTenant = {
  name: string;
  label: string;
  created_at: string;
  mapping_id: string | null;
};
type Invitation = {
  id: string;
  groups: string[];
  created_at: string;
  expires_at: string;
  state: "active" | "redeemed" | "revoked" | "expired";
  registration_id: string | null;
};
type IssuedInvitation = {
  id: string;
  tenant: string;
  link: string;
  secret: string;
  groups: string[];
  expires_at: string;
};

async function failure(response: Response, english: boolean): Promise<string> {
  const body = (await response.json().catch(() => ({}))) as { error?: string };
  if (response.status === 429) {
    const minutes = Math.ceil(
      Number(response.headers.get("retry-after") ?? "60") / 60,
    );
    return english
      ? `You have created many Tenants recently. Try again in about ${minutes} minutes.`
      : `最近多くのテナントを作成しています。約 ${minutes} 分後にもう一度お試しください。`;
  }
  if (body.error === "tenant_limit_reached")
    return english
      ? "You have reached the number of Tenants you can create."
      : "作成できるテナントの上限に達しています。";
  if (response.status === 403)
    return english
      ? "This account cannot create Tenants here."
      : "このアカウントではテナントを作成できません。";
  return body.error ?? `HTTP ${response.status}`;
}

/** Creates a Tenant under a name Aidash assigns; the label is the readable one. */
export function CreateTenantForm({
  onCreated,
}: {
  onCreated: (tenant: CreatedTenant) => void;
}) {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <form
      className="grid gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (busy) return;
        setBusy(true);
        setError("");
        void sessionFetch("/api/tenants", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ label }),
        })
          .then(async (response) => {
            if (response.status !== 201)
              throw new Error(await failure(response, english));
            setLabel("");
            onCreated((await response.json()) as CreatedTenant);
          })
          .catch((reason) =>
            setError(reason instanceof Error ? reason.message : String(reason)),
          )
          .finally(() => setBusy(false));
      }}
    >
      <label className="grid gap-1 text-xs font-medium text-muted-foreground">
        {english ? "New Tenant name" : "新しいテナントの表示名"}
        <Input
          value={label}
          maxLength={64}
          required
          onChange={(event) => setLabel(event.target.value)}
        />
      </label>
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      <Button
        type="submit"
        variant="outline"
        size="sm"
        disabled={busy || !label.trim()}
      >
        {english ? "Create Tenant" : "テナントを作成"}
      </Button>
    </form>
  );
}

/** Changes the Tenant label under `tenant.update`. */
export function TenantLabelForm({
  tenant,
  label,
}: {
  tenant: string;
  label: string | null;
}) {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const client = useQueryClient();
  // The caller keys this form by the saved label, so a change resets it.
  const [value, setValue] = useState(label ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <form
      className="flex flex-wrap items-end gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        setBusy(true);
        setError("");
        void apiFetch(`/api/tenants/${encodeURIComponent(tenant)}/label`, {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ label: value }),
        })
          .then(() =>
            client.invalidateQueries({
              queryKey: ["tenant-administration", tenant],
            }),
          )
          .catch((reason) =>
            setError(reason instanceof Error ? reason.message : String(reason)),
          )
          .finally(() => setBusy(false));
      }}
    >
      <label className="grid flex-1 gap-1 text-xs font-medium text-muted-foreground">
        {english ? "Tenant label" : "テナントの表示名"}
        <Input
          value={value}
          maxLength={64}
          onChange={(event) => setValue(event.target.value)}
        />
      </label>
      <Button
        type="submit"
        size="sm"
        variant="outline"
        disabled={busy || !value.trim() || value === label}
      >
        {english ? "Save" : "保存"}
      </Button>
      {error && (
        <p role="alert" className="basis-full text-xs text-destructive">
          {error}
        </p>
      )}
    </form>
  );
}

const lifetimes = [
  { seconds: 3_600, en: "1 hour", ja: "1 時間" },
  { seconds: 86_400, en: "1 day", ja: "1 日" },
  { seconds: 604_800, en: "7 days", ja: "7 日" },
];

/** Issues, lists and revokes a Tenant's Invitations. The link is shown once. */
export function TenantInvitations({
  tenant,
  assignable,
  canCreate,
  canRead,
  canRevoke,
}: {
  tenant: string;
  assignable: string[];
  canCreate: boolean;
  canRead: boolean;
  canRevoke: boolean;
}) {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const client = useQueryClient();
  const path = `/api/tenants/${encodeURIComponent(tenant)}/invitations`;
  const [groups, setGroups] = useState<string[]>(
    assignable.includes("members") ? ["members"] : [],
  );
  const [lifetime, setLifetime] = useState(604_800);
  const [issued, setIssued] = useState<IssuedInvitation | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const invitations = useQuery({
    queryKey: ["tenant-invitations", tenant],
    queryFn: () => apiFetch<Invitation[]>(path),
    enabled: canRead,
  });
  const act = async (action: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      await client.invalidateQueries({
        queryKey: ["tenant-invitations", tenant],
      });
      setBusy(false);
    }
  };
  const states: Record<Invitation["state"], string> = english
    ? {
        active: "Active",
        redeemed: "Used",
        revoked: "Revoked",
        expired: "Expired",
      }
    : {
        active: "有効",
        redeemed: "使用済み",
        revoked: "取り消し済み",
        expired: "期限切れ",
      };
  return (
    <section className="grid gap-2">
      <h3 className="text-[11px] font-medium text-faint">
        {english ? "Invitations" : "招待"}
      </h3>
      <Hint>
        {english
          ? "An Invitation is a single-use link. Whoever opens it can ask to join; an administrator still approves the request. Share it only with the person you invite."
          : "招待は 1 回だけ使えるリンクです。開いた人は参加を申請でき、管理者が承認して初めて参加します。招待する本人にだけ共有してください。"}
      </Hint>
      {error && <Alert>{error}</Alert>}
      {canCreate && (
        <div className="grid gap-2">
          <div className="flex flex-wrap items-center gap-2">
            {assignable.map((group) => (
              <label
                key={group}
                className="inline-flex items-center gap-1.5 text-xs"
              >
                <input
                  type="checkbox"
                  className="size-3.5 accent-primary"
                  checked={groups.includes(group)}
                  disabled={busy}
                  onChange={(event) =>
                    setGroups(
                      event.target.checked
                        ? [...groups, group]
                        : groups.filter((name) => name !== group),
                    )
                  }
                />
                {group}
              </label>
            ))}
            <NativeSelect
              aria-label={english ? "Link lifetime" : "リンクの有効期間"}
              value={String(lifetime)}
              onChange={(event) => setLifetime(Number(event.target.value))}
            >
              {lifetimes.map((option) => (
                <option key={option.seconds} value={option.seconds}>
                  {english ? option.en : option.ja}
                </option>
              ))}
            </NativeSelect>
            <Button
              size="sm"
              disabled={busy}
              onClick={() =>
                void act(async () => {
                  setIssued(
                    await apiFetch<IssuedInvitation>(path, {
                      method: "POST",
                      headers: { "content-type": "application/json" },
                      body: JSON.stringify({
                        groups,
                        expires_in_seconds: lifetime,
                      }),
                    }),
                  );
                })
              }
            >
              {english ? "Create invitation link" : "招待リンクを作成"}
            </Button>
          </div>
          {issued && (
            <div className="grid gap-1 rounded-md bg-raised p-2">
              <span className="text-[11px] text-faint">
                {english
                  ? "Copy this link now. It is not shown again."
                  : "このリンクを今コピーしてください。再表示はできません。"}
              </span>
              <code className="select-all break-all font-mono text-xs">
                {issued.link}
              </code>
              <Button
                size="sm"
                variant="outline"
                className="justify-self-start"
                onClick={() => void navigator.clipboard?.writeText(issued.link)}
              >
                {english ? "Copy" : "コピー"}
              </Button>
            </div>
          )}
        </div>
      )}
      {canRead && (
        <ul className="divide-y divide-border border-y border-border">
          {(invitations.data ?? []).map((invitation) => (
            <li
              key={invitation.id}
              className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2 text-xs"
            >
              <span className="font-medium">{states[invitation.state]}</span>
              <span className="font-mono text-faint">
                {invitation.groups.join(", ") || "—"}
              </span>
              <span className="text-faint">
                {english ? "Expires" : "期限"}{" "}
                {new Date(invitation.expires_at).toLocaleString(locale)}
              </span>
              {canRevoke && invitation.state === "active" && (
                <Button
                  size="sm"
                  variant="outline"
                  className="ml-auto"
                  disabled={busy}
                  onClick={() =>
                    void act(async () => {
                      const response = await authenticatedFetch(
                        `${path}/${invitation.id}/revoke`,
                        { method: "POST" },
                      );
                      if (!response.ok)
                        throw new Error(`HTTP ${response.status}`);
                    })
                  }
                >
                  {english ? "Revoke" : "取り消す"}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

const SECRET_KEY = "aidash-invitation";

/** The Tenant name in an Invitation link path, if this page is one. */
export function invitationTenant(pathname: string): string | null {
  const match = /^\/invitations\/([^/]+)\/?$/.exec(pathname);
  return match ? decodeURIComponent(match[1]) : null;
}

/** The secret travels in the fragment, so it never reaches server logs. It is
 *  kept for this tab across sign-in and removed from the address bar. */
function invitationSecret(tenant: string): string | null {
  const fragment = window.location.hash.slice(1);
  if (fragment) {
    sessionStorage.setItem(SECRET_KEY, JSON.stringify({ tenant, fragment }));
    window.history.replaceState(
      window.history.state,
      "",
      window.location.pathname + window.location.search,
    );
    return fragment;
  }
  try {
    const stored = JSON.parse(sessionStorage.getItem(SECRET_KEY) ?? "null") as {
      tenant: string;
      fragment: string;
    } | null;
    return stored?.tenant === tenant ? stored.fragment : null;
  } catch {
    return null;
  }
}

type Stage =
  | { kind: "loading" }
  | { kind: "signed-out" }
  | { kind: "invalid" }
  | { kind: "refused"; message: string }
  | { kind: "ready"; label: string | null }
  | { kind: "requested" };

/** Opening an Invitation link: sign in, see the inviting Tenant, request to join. */
export function InvitationLanding({ tenant }: { tenant: string }) {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const [secret] = useState(() => invitationSecret(tenant));
  const [stage, setStage] = useState<Stage>(() =>
    secret ? { kind: "loading" } : { kind: "invalid" },
  );
  const [busy, setBusy] = useState(false);
  const link = JSON.stringify({ tenant, secret });
  useEffect(() => {
    if (!secret) return;
    let cancelled = false;
    void (async () => {
      const session = await sessionFetch("/auth/session", {
        cache: "no-store",
      });
      if (cancelled) return;
      if (session.status === 401) {
        setStage({ kind: "signed-out" });
        return;
      }
      const response = await sessionFetch("/auth/invitations/inspect", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: link,
      });
      if (cancelled) return;
      if (response.ok) {
        const preview = (await response.json()) as { label: string | null };
        setStage({ kind: "ready", label: preview.label });
      } else if (response.status === 403)
        setStage({
          kind: "refused",
          message: english
            ? "This account cannot join other Tenants through an invitation."
            : "このアカウントは招待によってほかのテナントに参加できません。",
        });
      else setStage({ kind: "invalid" });
    })().catch(() => !cancelled && setStage({ kind: "invalid" }));
    return () => {
      cancelled = true;
    };
  }, [english, link, secret]);
  const title = english ? "Invitation" : "招待";
  return (
    <AuthCard title={title}>
      {stage.kind === "loading" && (
        <p className="text-muted-foreground">
          {english ? "Checking the link…" : "リンクを確認しています…"}
        </p>
      )}
      {stage.kind === "signed-out" && (
        <div className="grid gap-3">
          <p className="text-muted-foreground">
            {english
              ? "Sign in to see who invited you."
              : "招待元を確認するにはサインインしてください。"}
          </p>
          <Button
            type="button"
            onClick={() =>
              void signIn(`/invitations/${encodeURIComponent(tenant)}`)
            }
          >
            {english ? "Sign in" : "サインイン"}
          </Button>
        </div>
      )}
      {stage.kind === "invalid" && (
        <Alert>
          {english
            ? "This invitation link is invalid, expired or already used. Ask for a new one."
            : "この招待リンクは無効か、期限切れか、すでに使われています。新しいリンクを依頼してください。"}
        </Alert>
      )}
      {stage.kind === "refused" && <Alert>{stage.message}</Alert>}
      {stage.kind === "ready" && (
        <div className="grid gap-3">
          <p>
            {english ? "You are invited to " : "招待先: "}
            <strong>{stage.label ?? tenant}</strong>
          </p>
          <p className="text-xs text-muted-foreground">
            {english
              ? "Requesting to join uses this link. An administrator of the Tenant approves the request before you can use it."
              : "参加を申請するとこのリンクは使用済みになります。テナントの管理者が承認すると利用できるようになります。"}
          </p>
          <Button
            type="button"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void sessionFetch("/auth/invitations/redeem", {
                method: "POST",
                headers: { "content-type": "application/json" },
                body: link,
              })
                .then(async (response) => {
                  if (response.ok) {
                    sessionStorage.removeItem(SECRET_KEY);
                    setStage({ kind: "requested" });
                  } else if (response.status === 409)
                    setStage({
                      kind: "refused",
                      message:
                        ((await response.json()) as { error?: string }).error ??
                        `HTTP ${response.status}`,
                    });
                  else setStage({ kind: "invalid" });
                })
                .finally(() => setBusy(false));
            }}
          >
            {english ? "Request to join" : "参加を申請"}
          </Button>
        </div>
      )}
      {stage.kind === "requested" && (
        <div className="grid gap-3">
          <p>
            {english
              ? "Your request was sent. You can use the Tenant once an administrator approves it."
              : "申請を送信しました。管理者が承認するとテナントを利用できます。"}
          </p>
          <Button
            type="button"
            variant="outline"
            onClick={() => window.location.assign("/")}
          >
            {english ? "Go to the dashboard" : "ダッシュボードへ"}
          </Button>
        </div>
      )}
    </AuthCard>
  );
}
