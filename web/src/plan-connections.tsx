import { selfHostedPlanConnections } from "@slots/plan-connections";
import type { PlanConnectionStatus } from "./generated/models";
import {
  planConnectionStatus,
  planConnectionDisconnect,
  planAuthorizationStart,
  planAuthorizationComplete,
} from "./generated/aidash";
import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { Alert, Hint, Notice } from "./components/patterns";
import { Field, useI18n } from "./ui";
import { desktop, desktopAvailable } from "./desktop";
import {
  dashboardContext,
  transportGeneration,
  AUTHENTICATION_EXPIRED,
  CONNECTION_CHANGED,
  SESSION_CHANGED,
} from "./transport";

// The same subject under another Mapping or browser session is another owner.
function ownerKey(tenant: string, subject: string) {
  return [
    tenant,
    subject,
    dashboardContext(),
    sessionStorage.getItem("aidash-session-id"),
    transportGeneration(),
  ];
}
export function usePlanConnection(tenant: string, subject: string) {
  const [epoch, setEpoch] = useState(0);
  useEffect(() => {
    const changed = () => setEpoch((value) => value + 1);
    window.addEventListener(SESSION_CHANGED, changed);
    return () => window.removeEventListener(SESSION_CHANGED, changed);
  }, []);
  return useQuery({
    queryKey: ["personal-plan", ...ownerKey(tenant, subject), epoch],
    queryFn: ({ signal }) =>
      planConnectionStatus(encodeURIComponent(tenant), "chatgpt", { signal }),
    enabled: selfHostedPlanConnections,
    retry: false,
    gcTime: 0,
    refetchInterval: 30_000,
  });
}
const usageUrl = "https://chatgpt.com/settings/usage";
export function ManagePlanUsage() {
  const { locale } = useI18n();
  return (
    <a
      className="text-brand underline underline-offset-2"
      href={usageUrl}
      target="_blank"
      rel="noopener noreferrer"
    >
      {locale === "ja-JP" ? "使用量を管理" : "Manage usage"}
    </a>
  );
}
function guidance(status: PlanConnectionStatus, ja: boolean) {
  if (!status.enabled)
    return ja
      ? "この Deployment では Plan の使用が無効です。"
      : "Plan usage is disabled on this deployment.";
  if (status.connection?.state === "ineligible")
    return ja
      ? "お使いの ChatGPT プランまたはワークスペースは Aidash で利用できません。"
      : "Your ChatGPT plan or workspace is not eligible for Aidash.";
  if (status.connection?.state === "needs_reconnection")
    return ja
      ? "ChatGPT Plan の再接続が必要です。個人設定で再接続してください。"
      : "Your ChatGPT plan needs reconnection. Reconnect in personal settings.";
  return "";
}
export function PlanConnectionBanner({
  tenant,
  subject,
  required = false,
}: {
  tenant: string;
  subject: string;
  required?: boolean;
}) {
  const status = usePlanConnection(tenant, subject);
  const { locale } = useI18n();
  if (
    (!status.data?.connection && !status.isError && !required) ||
    status.isPending
  )
    return null;
  const message = status.isError
    ? locale === "ja-JP"
      ? "Plan 接続を確認できません。現在の ID と Mapping を確認してください。"
      : "Could not verify your Plan Connection. Check your current identity and Mapping."
    : !status.data?.connection && status.data?.enabled
      ? locale === "ja-JP"
        ? "Plan モデルを使うには、個人設定で ChatGPT Plan を接続してください。"
        : "Connect your ChatGPT plan in personal settings to use Plan models."
      : guidance(status.data!, locale === "ja-JP");
  if (!message) return null;
  return (
    <Notice role="status" tone="warning">
      <span>{message} </span>
      <a className="underline" href="/settings?view=planConnections">
        {locale === "ja-JP" ? "Plan 接続を開く" : "Open Plan Connections"}
      </a>
      {" · "}
      <ManagePlanUsage />
    </Notice>
  );
}

export function PlanConnectionsPage({
  tenant,
  subject,
}: {
  tenant: string;
  subject: string;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const status = usePlanConnection(tenant, subject);
  const [pending, setPending] = useState<{ authorizationUrl: string } | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const attempt = useRef(0);
  const listener = useRef<string | null>(null);
  const activeFlow = useRef<Attempt | null>(null);
  const lifetime = useRef(new AbortController());
  const cancel = () => {
    attempt.current += 1;
    activeFlow.current = null;
    lifetime.current.abort();
    lifetime.current = new AbortController();
    const id = listener.current;
    listener.current = null;
    if (id) void desktop.planCancel(id).catch(() => {});
    setPending(null);
    setBusy(false);
  };
  useEffect(() => {
    const changed = () => {
      cancel();
      setMessage("");
      setError("");
    };
    for (const event of [
      SESSION_CHANGED,
      CONNECTION_CHANGED,
      AUTHENTICATION_EXPIRED,
    ])
      window.addEventListener(event, changed);
    return () => {
      for (const event of [
        SESSION_CHANGED,
        CONNECTION_CHANGED,
        AUTHENTICATION_EXPIRED,
      ])
        window.removeEventListener(event, changed);
      cancel();
    };
  }, [tenant, subject]);
  const begin = () => {
    cancel();
    setMessage("");
    setError("");
    return {
      id: attempt.current,
      owner: JSON.stringify(ownerKey(tenant, subject)),
      generation: transportGeneration(),
      signal: lifetime.current.signal,
    };
  };
  type Attempt = ReturnType<typeof begin>;
  const holds = (flow: Attempt) =>
    flow.id === attempt.current &&
    flow.generation === transportGeneration() &&
    flow.owner === JSON.stringify(ownerKey(tenant, subject)) &&
    !flow.signal.aborted;
  const complete = async (callbackUrl: string, flow: Attempt) => {
    if (!holds(flow)) return;
    setBusy(true);
    try {
      const result = await planAuthorizationComplete(
        encodeURIComponent(tenant),
        "chatgpt",
        { callback_url: callbackUrl },
        { signal: flow.signal },
      );
      if (!holds(flow)) return;
      setPending(null);
      setMessage(
        result.outcome === "connected" || result.outcome === "reconnected"
          ? ja
            ? "ChatGPT Plan を接続しました。"
            : "ChatGPT plan connected."
          : result.outcome === "denied"
            ? ja
              ? "接続は許可されませんでした。"
              : "Connection was declined."
            : result.outcome === "plan_usage_not_granted"
              ? ja
                ? "Plan の使用が許可されていません。再接続して許可してください。"
                : "Plan usage was not granted. Reconnect to grant access."
              : ja
                ? "接続を完了できませんでした。もう一度お試しください。"
                : "Could not complete the connection. Try again.",
      );
      await status.refetch();
    } catch {
      if (holds(flow))
        setError(
          ja
            ? "接続を完了できませんでした。再接続してお試しください。"
            : "Could not complete the connection. Reconnect and try again.",
        );
    } finally {
      if (holds(flow)) setBusy(false);
    }
  };
  const connect = async () => {
    const flow = begin();
    activeFlow.current = flow;
    setBusy(true);
    setPending({ authorizationUrl: "" });
    try {
      let port: number | undefined;
      if (desktopAvailable()) {
        const opened = await desktop.planListen();
        if (!holds(flow)) {
          await desktop.planCancel(opened.id);
          return;
        }
        listener.current = opened.id;
        port = opened.port;
      }
      const started = await planAuthorizationStart(
        encodeURIComponent(tenant),
        "chatgpt",
        port ? { port } : {},
        { signal: flow.signal },
      );
      if (!holds(flow)) return;
      if (listener.current) {
        setPending({ authorizationUrl: "" });
        const callbackUrl = await desktop.planWait(
          listener.current,
          started.authorization_url,
        );
        if (!holds(flow)) return;
        listener.current = null;
        await complete(callbackUrl, flow);
      } else {
        setPending({ authorizationUrl: started.authorization_url });
        setBusy(false);
      }
    } catch {
      if (holds(flow)) {
        cancel();
        setError(
          ja
            ? "接続を開始できませんでした。現在の ID と Mapping を確認してください。"
            : "Could not start the connection. Check your current identity and Mapping.",
        );
      }
    }
  };
  const connection = status.data?.connection;
  const warning = status.data ? guidance(status.data, ja) : "";
  return (
    <div className="grid max-w-3xl gap-4">
      <Notice>
        {ja
          ? "これはあなた個人の Plan 接続です。あなたを起点とする委譲で実行するローカル Run（スケジュール・イベント・子 Run を含む）で使用でき、他の利用者に権限を与えません。ホストの運用者は Plan Token を読み取ることができます。コンテキストの圧縮には Tenant の OpenRouter 認証情報を使います。"
          : "This is your personal Plan Connection. It serves local Runs whose delegation starts with you, including scheduled, event-triggered and child Runs; it grants no authority to other subjects. Whoever operates this host can read the Plan Tokens. Context compaction uses the Tenant's OpenRouter binding."}
      </Notice>
      <Hint>
        {ja
          ? "接続失敗時に他の認証情報へ自動的に切り替えることはありません。"
          : "Connection failures never fall back to other credentials."}
      </Hint>
      {status.isPending && (
        <Hint role="status">
          {ja ? "接続状態を確認中…" : "Loading connection status…"}
        </Hint>
      )}
      {status.isError && (
        <Alert>
          {ja
            ? "接続状態を確認できません。現在の ID と Mapping を確認してください。"
            : "Could not load connection status. Check your current identity and Mapping."}
        </Alert>
      )}
      {error && <Alert>{error}</Alert>}
      {message && <Notice role="status">{message}</Notice>}
      {warning && (
        <Notice tone="warning" role="status">
          {warning}
        </Notice>
      )}
      {status.data && (
        <p>
          {connection
            ? connection.state === "active"
              ? ja
                ? "接続済み"
                : "Connected"
              : connection.state === "needs_reconnection"
                ? ja
                  ? "再接続が必要"
                  : "Needs reconnection"
                : ja
                  ? "利用対象外"
                  : "Ineligible"
            : ja
              ? "未接続"
              : "Not connected"}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-3">
        {status.data?.enabled &&
          connection?.state !== "ineligible" &&
          !pending && (
            <Button
              type="button"
              disabled={busy}
              onClick={() => void connect()}
            >
              {connection
                ? ja
                  ? "再接続"
                  : "Reconnect"
                : ja
                  ? "ChatGPT を接続"
                  : "Connect ChatGPT"}
            </Button>
          )}
        {connection && (
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={async () => {
              const flow = begin();
              setBusy(true);
              try {
                const result = await planConnectionDisconnect(
                  encodeURIComponent(tenant),
                  "chatgpt",
                  { expected_revision: connection.revision },
                  { signal: flow.signal },
                );
                if (!holds(flow)) return;
                setMessage(
                  result.revoked
                    ? ja
                      ? "接続を解除しました。"
                      : "Connection disconnected."
                    : ja
                      ? "ローカルの接続を削除しました。プロバイダ側での取り消しは確認できませんでした。"
                      : "Local connection deleted. Provider revocation could not be confirmed.",
                );
                await status.refetch();
              } catch {
                if (holds(flow))
                  setError(
                    ja
                      ? "接続を解除できませんでした。状態を再確認してください。"
                      : "Could not disconnect. Refresh the connection status and try again.",
                  );
              } finally {
                if (holds(flow)) setBusy(false);
              }
            }}
          >
            {ja ? "接続を解除" : "Disconnect"}
          </Button>
        )}
        <ManagePlanUsage />
      </div>
      {pending && (
        <div className="grid gap-3">
          {pending.authorizationUrl ? (
            <>
              <a
                className="text-brand underline"
                href={pending.authorizationUrl}
                target="_blank"
                rel="noopener noreferrer"
              >
                {ja ? "ブラウザで続ける" : "Continue in browser"}
              </a>
              <Hint>
                {ja
                  ? "同意後に開く 127.0.0.1 のページは接続エラーになります。そのアドレスを下に貼り付けてください。URL は一度だけ使え、保存されません。"
                  : "After consent, the 127.0.0.1 page will show a connection error. Paste that page's address below. The URL is single-use and is never saved."}
              </Hint>
              <form
                className="grid gap-3"
                onSubmit={(event) => {
                  event.preventDefault();
                  const form = event.currentTarget;
                  const callbackUrl = String(
                    new FormData(form).get("callback_url") ?? "",
                  ).trim();
                  form.reset();
                  setError("");
                  const flow = activeFlow.current;
                  if (flow) void complete(callbackUrl, flow);
                }}
              >
                <Field label={ja ? "コールバック URL" : "Callback URL"}>
                  <Input
                    name="callback_url"
                    type="password"
                    autoComplete="off"
                    required
                    maxLength={8192}
                    spellCheck={false}
                    disabled={busy}
                  />
                </Field>
                <Button type="submit" disabled={busy}>
                  {ja ? "接続を完了" : "Complete connection"}
                </Button>
              </form>
            </>
          ) : (
            <Hint role="status">
              {ja
                ? "ブラウザでの認証を待っています…"
                : "Waiting for browser authorization…"}
            </Hint>
          )}
          <Button type="button" variant="outline" onClick={cancel}>
            {ja ? "接続をキャンセル" : "Cancel connection"}
          </Button>
        </div>
      )}
    </div>
  );
}
