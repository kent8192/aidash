import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { Alert, Hint } from "./components/patterns";
import {
  apiFetch,
  mappingContext,
  SESSION_CHANGED,
  sessionFetch,
} from "./transport";
import { useI18n } from "./ui";

/** A recorded Tenant Deletion (ADR 0060). */
export type TenantDeletion = {
  tenant: string;
  deleted_at: string;
  deleted_by_kind: "tenant_administrator" | "deployment_administrator";
  purge_after: string;
  purged_at: string | null;
};

/** A deleted Tenant this identity is mapped into, as the session lists it. */
export type DeletedTenant = {
  tenant: string;
  label?: string | null;
  mapping_id: string;
  personal: boolean;
  purge_after: string;
  restorable: boolean;
};

type TenantRecord = {
  name: string;
  label: string | null;
  personal: boolean;
  created_at: string;
  created_by_kind: string;
  deletion: TenantDeletion | null;
};

const messages = {
  "the Tenant would be left without a Tenant Administrator": {
    en: "This Tenant would be left without a Tenant Administrator.",
    ja: "このテナントに Tenant Administrator がいなくなります。",
  },
  "appoint another Tenant Administrator or delete the Tenant before leaving": {
    en: "You are the last Tenant Administrator. Make someone else an administrator, or delete the Tenant instead.",
    ja: "あなたは最後の Tenant Administrator です。先に別の人を管理者にするか、テナントを削除してください。",
  },
  "a member mapped through the Tenant Binding cannot leave": {
    en: "Your sign-in maps you into this Tenant, so you cannot leave it here.",
    ja: "サインインによってこのテナントに mapping されるため、ここでは退出できません。",
  },
  tenant_grace_period_ended: {
    en: "The grace period has ended; the Tenant can no longer be restored.",
    ja: "猶予期間が終わったため、このテナントは復元できません。",
  },
  tenant_purged: {
    en: "The Tenant's data has been purged; it can no longer be restored.",
    ja: "テナントのデータは消去済みのため、復元できません。",
  },
  tenant_deleted: {
    en: "This Tenant is already deleted.",
    ja: "このテナントは既に削除されています。",
  },
} as const;

/** The server's error, in the reader's language where Aidash knows it. */
async function failure(response: Response, english: boolean): Promise<string> {
  const body = (await response.json().catch(() => ({}))) as { error?: string };
  const known = body.error && messages[body.error as keyof typeof messages];
  if (known) return english ? known.en : known.ja;
  if (response.status === 403)
    return english
      ? "Your permissions do not allow this."
      : "現在の権限ではこの操作はできません。";
  return body.error ?? `HTTP ${response.status}`;
}

function changed(): void {
  window.dispatchEvent(new Event(SESSION_CHANGED));
}

/** Deletes a Tenant under `tenant.delete` once its name is typed. */
export function DeleteTenantForm({
  tenant,
  graceDays,
  onDeleted,
}: {
  tenant: string;
  /** Days the deleted Tenant stays restorable, when known. */
  graceDays?: number;
  onDeleted?: (deletion: TenantDeletion) => void;
}) {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <form
      className="grid gap-2 rounded-md border border-destructive/40 p-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (busy) return;
        setBusy(true);
        setError("");
        void sessionFetch(
          `/api/tenants/${encodeURIComponent(tenant)}/deletion`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ confirmation: typed }),
          },
        )
          .then(async (response) => {
            if (!response.ok) throw new Error(await failure(response, english));
            setTyped("");
            onDeleted?.((await response.json()) as TenantDeletion);
            changed();
          })
          .catch((reason) =>
            setError(reason instanceof Error ? reason.message : String(reason)),
          )
          .finally(() => setBusy(false));
      }}
    >
      <h3 className="text-[11px] font-medium text-destructive">
        {english ? "Delete this Tenant" : "このテナントを削除"}
      </h3>
      <Hint>
        {english
          ? `Nobody can use the Tenant from the moment it is deleted, and its running work is cancelled. Its administrators can restore it for ${graceDays ?? 30} days; afterwards its data is purged. Sign in with ChatGPT connections end now and must be connected again after a restore. Delete any OpenRouter keys at OpenRouter yourself.`
          : `削除した時点で誰もこのテナントを使えなくなり、実行中の作業は取り消されます。管理者は ${graceDays ?? 30} 日間は復元でき、その後データは消去されます。Sign in with ChatGPT の接続はすぐに終了し、復元後には接続し直す必要があります。OpenRouter のキーは OpenRouter 側で削除してください。`}
      </Hint>
      <label className="grid gap-1 text-xs font-medium text-muted-foreground">
        {english
          ? `Type the Tenant name ${tenant} to confirm`
          : `確認のため、テナント名 ${tenant} を入力してください`}
        <Input
          value={typed}
          autoComplete="off"
          spellCheck={false}
          className="font-mono"
          onChange={(event) => setTyped(event.target.value)}
        />
      </label>
      {error && <Alert>{error}</Alert>}
      <Button
        type="submit"
        variant="destructive"
        size="sm"
        disabled={busy || typed.trim() !== tenant}
      >
        {english ? "Delete Tenant" : "テナントを削除"}
      </Button>
    </form>
  );
}

/** Leaves the Tenant through the current Mapping. */
export function LeaveTenantButton({ tenant }: { tenant: string }) {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const leave = () => {
    setBusy(true);
    setError("");
    void sessionFetch(`/api/tenants/${encodeURIComponent(tenant)}/leave`, {
      method: "POST",
    })
      .then(async (response) => {
        if (!response.ok) throw new Error(await failure(response, english));
        changed();
      })
      .catch((reason) =>
        setError(reason instanceof Error ? reason.message : String(reason)),
      )
      .finally(() => {
        setBusy(false);
        setConfirming(false);
      });
  };
  return (
    <section className="grid gap-2">
      <h3 className="text-[11px] font-medium text-faint">
        {english ? "Leave this Tenant" : "このテナントから退出"}
      </h3>
      <Hint>
        {english
          ? "You lose access at once. You can come back only through a new Invitation."
          : "すぐにアクセスできなくなります。戻るには新しい招待が必要です。"}
      </Hint>
      {error && <Alert>{error}</Alert>}
      {confirming ? (
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="destructive"
            disabled={busy}
            onClick={leave}
          >
            {english ? "Leave now" : "退出する"}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() => setConfirming(false)}
          >
            {english ? "Cancel" : "キャンセル"}
          </Button>
        </div>
      ) : (
        <Button
          size="sm"
          variant="outline"
          className="justify-self-start"
          onClick={() => setConfirming(true)}
        >
          {english ? "Leave Tenant" : "テナントから退出"}
        </Button>
      )}
    </section>
  );
}

function deadline(value: string, locale: string): string {
  return new Date(value).toLocaleString(locale);
}

/** Restores a deleted Tenant through the caller's Mapping in it, or as a
 *  Deployment Administrator when no Mapping is named. */
async function restore(
  tenant: string,
  mapping: string | null,
  english: boolean,
): Promise<void> {
  const response = await sessionFetch(
    `/api/tenants/${encodeURIComponent(tenant)}/restore`,
    {
      method: "POST",
      headers: mapping
        ? { "x-aidash-context": mappingContext({ tenant, id: mapping }) }
        : {},
    },
  );
  if (!response.ok) throw new Error(await failure(response, english));
}

/** The deleted Tenants an External Identity is mapped into, with Restore for
 *  those it may restore, and a new Personal Tenant once the old one is gone. */
export function DeletedTenants({
  tenants,
  personalTenantDeleted,
}: {
  tenants: DeletedTenant[];
  personalTenantDeleted: boolean;
}) {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  if (tenants.length === 0 && !personalTenantDeleted) return null;
  const act = (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    void action()
      .then(changed)
      .catch((reason) =>
        setError(reason instanceof Error ? reason.message : String(reason)),
      )
      .finally(() => setBusy(false));
  };
  const personalRestorable = tenants.some(
    (tenant) => tenant.personal && tenant.restorable,
  );
  return (
    <section className="grid gap-2">
      <h3 className="text-[11px] font-medium text-faint">
        {english ? "Deleted Tenants" : "削除したテナント"}
      </h3>
      {error && <Alert>{error}</Alert>}
      <ul className="grid gap-1.5">
        {tenants.map((tenant) => (
          <li
            key={tenant.mapping_id}
            className="flex flex-wrap items-center justify-between gap-2 text-xs"
          >
            <span className="min-w-0">
              <span className="block truncate font-medium">
                {tenant.label ?? tenant.tenant}
              </span>
              <span className="block text-faint">
                {english ? "Restorable until " : "復元期限 "}
                <span className="font-mono tabular">
                  {deadline(tenant.purge_after, locale)}
                </span>
              </span>
            </span>
            {tenant.restorable && (
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() =>
                  act(() => restore(tenant.tenant, tenant.mapping_id, english))
                }
              >
                {english ? "Restore" : "復元"}
              </Button>
            )}
          </li>
        ))}
      </ul>
      {personalTenantDeleted && (
        <div className="grid gap-1.5">
          <Hint>
            {personalRestorable
              ? english
                ? "Your Personal Tenant is deleted. Restore it above to keep its data, or start a new, empty Personal Tenant."
                : "Personal Tenant は削除されています。データを残すなら上で復元し、空の Personal Tenant から始めることもできます。"
              : english
                ? "Your Personal Tenant is deleted. You can start a new, empty one."
                : "Personal Tenant は削除されています。空の Personal Tenant を新しく作れます。"}
          </Hint>
          <Button
            size="sm"
            variant="outline"
            className="justify-self-start"
            disabled={busy}
            onClick={() =>
              act(async () => {
                const response = await sessionFetch("/api/tenants/personal", {
                  method: "POST",
                });
                if (response.status !== 201)
                  throw new Error(await failure(response, english));
              })
            }
          >
            {english ? "New Personal Tenant" : "新しい Personal Tenant"}
          </Button>
        </div>
      )}
    </section>
  );
}

/** The Deployment Administrators' list of Tenants, with each deletion's
 *  deadline and Restore while it is restorable. */
export function DeploymentTenants() {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const client = useQueryClient();
  const [offset, setOffset] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const tenants = useQuery({
    queryKey: ["deployment-tenants", offset],
    queryFn: () => apiFetch<TenantRecord[]>(`/api/tenants?offset=${offset}`),
  });
  const restorable = (deletion: TenantDeletion) =>
    !deletion.purged_at && new Date(deletion.purge_after) > new Date();
  return (
    <section className="grid gap-2">
      <h3 className="text-[11px] font-medium text-faint">
        {english ? "Tenants" : "テナント"}
      </h3>
      {(error || tenants.error) && (
        <Alert>{error || tenants.error?.message}</Alert>
      )}
      <ul className="divide-y divide-border border-y border-border">
        {(tenants.data ?? []).map((tenant) => (
          <li
            key={tenant.name}
            className="flex flex-wrap items-center justify-between gap-2 py-2 text-xs"
          >
            <span className="min-w-0">
              <span className="block truncate font-medium">
                {tenant.label ?? tenant.name}
              </span>
              <span className="block font-mono text-[11px] text-faint">
                {tenant.name}
              </span>
              {tenant.deletion && (
                <span className="block text-destructive">
                  {tenant.deletion.purged_at
                    ? english
                      ? "Purged"
                      : "消去済み"
                    : english
                      ? `Deleted; purged after ${deadline(tenant.deletion.purge_after, locale)}`
                      : `削除済み。${deadline(tenant.deletion.purge_after, locale)} 以降に消去`}
                </span>
              )}
            </span>
            {tenant.deletion && restorable(tenant.deletion) && (
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => {
                  setBusy(true);
                  setError("");
                  void restore(tenant.name, null, english)
                    .catch((reason) =>
                      setError(
                        reason instanceof Error
                          ? reason.message
                          : String(reason),
                      ),
                    )
                    .finally(() => {
                      setBusy(false);
                      void client.invalidateQueries({
                        queryKey: ["deployment-tenants"],
                      });
                    });
                }}
              >
                {english ? "Restore" : "復元"}
              </Button>
            )}
          </li>
        ))}
      </ul>
      <div className="flex gap-2">
        <Button
          size="sm"
          variant="outline"
          disabled={offset === 0 || tenants.isFetching}
          onClick={() => setOffset(Math.max(0, offset - 200))}
        >
          {english ? "Previous" : "前へ"}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={(tenants.data?.length ?? 0) < 200 || tenants.isFetching}
          onClick={() => setOffset(offset + 200)}
        >
          {english ? "Next" : "次へ"}
        </Button>
      </div>
    </section>
  );
}
