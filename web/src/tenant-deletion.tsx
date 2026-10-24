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

/** How far a purge got, once it started or failed (ADR 0061). */
export type PurgeProgress = {
  step:
    | "provider_credentials"
    | "capability_objects"
    | "rows"
    | "finish"
    | null;
  failures: number;
  error: string | null;
  failed_at: string | null;
  /** Failed several times in a row and needs attention. */
  stalled: boolean;
};

type TenantRecord = {
  name: string;
  label: string | null;
  personal: boolean;
  created_at: string;
  created_by_kind: string;
  deletion: TenantDeletion | null;
  /** Decided by the server's clock, not this device's. */
  restorable: boolean;
  /** Deleted and not purged yet, so a Deployment Administrator may purge it. */
  purgeable: boolean;
  purge: PurgeProgress | null;
};

const purgeSteps = {
  provider_credentials: {
    en: "ending Provider Credentials",
    ja: "Provider Credential の終了",
  },
  capability_objects: {
    en: "erasing capability object files",
    ja: "capability オブジェクトファイルの削除",
  },
  rows: { en: "deleting the Tenant's data", ja: "テナントのデータの削除" },
  finish: { en: "finishing", ja: "完了処理" },
} as const;

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
  deleter = "tenant_administrator",
  onDeleted,
}: {
  tenant: string;
  /** Days the deleted Tenant stays restorable, when known. */
  graceDays?: number;
  /** Who deletes it, which decides who may restore it (ADR 0060). */
  deleter?: TenantDeletion["deleted_by_kind"];
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
      <Hint>{deletionNotice(english, deleter, graceDays)}</Hint>
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

/** What deleting a Tenant does, in the reader's language. Only a Deployment
 *  Administrator restores a Tenant a Deployment Administrator deleted. */
function deletionNotice(
  english: boolean,
  deleter: TenantDeletion["deleted_by_kind"],
  graceDays: number | undefined,
): string {
  const period = english
    ? graceDays === undefined
      ? "during this Deployment's grace period"
      : `for ${graceDays} days`
    : graceDays === undefined
      ? "この Deployment の猶予期間中は"
      : `${graceDays} 日間は`;
  if (english) {
    const restorer =
      deleter === "deployment_administrator"
        ? "Only a Deployment Administrator can restore it"
        : "Its Tenant Administrators or a Deployment Administrator can restore it";
    return `Nobody can use the Tenant from the moment it is deleted, and its running work is cancelled. ${restorer} ${period}; after that its data is purged and it can no longer be restored. Its administration history, authorization decisions, policy revisions and catalog history are kept for a limited time after the purge. Sign in with ChatGPT connections end now and must be connected again after a restore. Delete any OpenRouter keys at OpenRouter yourself; Aidash cannot reach them, nor provider-side caches or platform logs.`;
  }
  const restorer =
    deleter === "deployment_administrator"
      ? "Deployment Administrator だけが"
      : "Tenant Administrator または Deployment Administrator が";
  return `削除した時点で誰もこのテナントを使えなくなり、実行中の作業は取り消されます。${period}${restorer}復元でき、その後はテナントのデータが消去され、復元できません。管理履歴・認可の判定・ポリシーの改訂・カタログ履歴は、消去後も一定期間だけ保持されます。Sign in with ChatGPT の接続はすぐに終了し、復元後には接続し直す必要があります。OpenRouter のキーは OpenRouter 側で削除してください。Aidash は OpenRouter のキー、プロバイダ側のプロンプトキャッシュ、プラットフォームのログには手が届きません。`;
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

/** Purges a deleted Tenant's data now, as a Deployment Administrator. */
async function purge(tenant: string, english: boolean): Promise<void> {
  const response = await sessionFetch(
    `/api/tenants/${encodeURIComponent(tenant)}/purge`,
    { method: "POST" },
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

/** Where a purge stands, naming the step and error of one that keeps failing. */
function purgeProgress(
  purge: PurgeProgress,
  english: boolean,
  locale: string,
): string {
  const step = purge.step ? purgeSteps[purge.step] : null;
  const name = step ? (english ? step.en : step.ja) : "";
  if (purge.failures === 0)
    return english ? `Purging: ${name}` : `消去中: ${name}`;
  const when = purge.failed_at ? deadline(purge.failed_at, locale) : "";
  const error = purge.error ?? "";
  return english
    ? `${purge.stalled ? "Purge stalled" : "Purge failing"} while ${name}, ${purge.failures} failures, last ${when}: ${error}`
    : `${purge.stalled ? "消去が停滞しています" : "消去に失敗しています"}（${name}、${purge.failures} 回、最後 ${when}）: ${error}`;
}

/** The Deployment Administrators' list of Tenants, with each deletion's
 *  deadline, Restore while it is restorable, Purge now until it is purged, and
 *  the step and error of a purge that keeps failing. */
export function DeploymentTenants() {
  const { locale } = useI18n();
  const english = locale === "en-US";
  const client = useQueryClient();
  const [offset, setOffset] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [confirming, setConfirming] = useState<string | null>(null);
  const tenants = useQuery({
    queryKey: ["deployment-tenants", offset],
    queryFn: () => apiFetch<TenantRecord[]>(`/api/tenants?offset=${offset}`),
  });
  const act = (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    void action()
      .catch((reason) =>
        setError(reason instanceof Error ? reason.message : String(reason)),
      )
      .finally(() => {
        setBusy(false);
        void client.invalidateQueries({ queryKey: ["deployment-tenants"] });
      });
  };
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
                    : tenant.restorable
                      ? english
                        ? `Deleted; restorable until ${deadline(tenant.deletion.purge_after, locale)}`
                        : `削除済み。${deadline(tenant.deletion.purge_after, locale)} まで復元できます`
                      : english
                        ? `Deleted; the restore deadline ${deadline(tenant.deletion.purge_after, locale)} has passed`
                        : `削除済み。復元期限（${deadline(tenant.deletion.purge_after, locale)}）を過ぎました`}
                </span>
              )}
              {tenant.purge && (
                <span
                  className={
                    tenant.purge.stalled
                      ? "block text-destructive"
                      : "block text-faint"
                  }
                >
                  {purgeProgress(tenant.purge, english, locale)}
                </span>
              )}
            </span>
            <span className="flex flex-wrap items-center gap-2">
              {tenant.restorable && (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy}
                  onClick={() =>
                    act(
                      // A Mapping in the restored Tenant reappears in the session.
                      () => restore(tenant.name, null, english).then(changed),
                    )
                  }
                >
                  {english ? "Restore" : "復元"}
                </Button>
              )}
              {tenant.purgeable &&
                (confirming === tenant.name ? (
                  <>
                    <Button
                      size="sm"
                      variant="destructive"
                      disabled={busy}
                      onClick={() =>
                        act(() =>
                          purge(tenant.name, english).then(() =>
                            setConfirming(null),
                          ),
                        )
                      }
                    >
                      {english ? "Purge permanently" : "完全に消去する"}
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={busy}
                      onClick={() => setConfirming(null)}
                    >
                      {english ? "Cancel" : "キャンセル"}
                    </Button>
                  </>
                ) : (
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={busy}
                    onClick={() => setConfirming(tenant.name)}
                  >
                    {english ? "Purge now" : "今すぐ消去"}
                  </Button>
                ))}
            </span>
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
