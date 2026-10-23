import { useState } from "react";
import { selfHostedPlanConnections } from "@slots/plan-connections";
import { NativeSelect } from "./components/ui/native-select";
import { Badge } from "./components/ui/badge";
import { Hint, Notice } from "./components/patterns";
import { Field, useEntityLabel, useI18n } from "./ui";
import { usePlanConnection } from "./plan-connections";
import type { Entry, State } from "./types";

function PersonalPlanGuidance({
  tenant,
  subject,
}: {
  tenant: string;
  subject: string;
}) {
  const status = usePlanConnection(tenant, subject);
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const usable =
    !status.isError &&
    status.data?.enabled &&
    status.data.connection?.state === "active";
  return (
    <Notice>
      {!status.isPending && !usable && (
        <>
          <Badge tone="warning">
            {status.isError
              ? ja
                ? "接続を確認"
                : "Check connection"
              : !status.data?.connection
                ? ja
                  ? "未接続"
                  : "Not connected"
                : status.data.connection.state === "needs_reconnection"
                  ? ja
                    ? "再接続が必要"
                    : "Needs reconnection"
                  : status.data.connection.state === "ineligible"
                    ? ja
                      ? "利用条件を確認"
                      : "Check eligibility"
                    : ja
                      ? "Plan 使用は無効"
                      : "Plan usage disabled"}
          </Badge>
          {" · "}
          <a className="underline" href="/settings?view=planConnections">
            {ja ? "ChatGPT Plan の個人設定" : "Personal ChatGPT plan settings"}
          </a>
        </>
      )}
      <Hint>
        {ja
          ? "モデルの選択は Plan の利用権限を与えません。Run の実行には、開始した本人を起点とする有効な接続と利用資格が必要です。"
          : "Selecting a model grants no plan authority. Execution still requires the initiating owner's own eligible connection."}
      </Hint>
    </Notice>
  );
}

/** A Registry reference selects metadata; its provider/source never grants credential access. */
export function RegistryModelPicker({
  entries,
  access,
}: {
  entries: Entry[];
  access: State["access"];
}) {
  const { t, locale } = useI18n();
  const label = useEntityLabel(entries);
  const [selectedSource, selectSource] = useState<string>();
  const [selectedModel, selectModel] = useState("");
  const groups = new Map<
    string,
    { provider: string; source: string; entries: Entry[] }
  >();
  for (const entry of entries) {
    const config = (entry.config ?? {}) as {
      provider?: string;
      provider_credential?: string;
      plan_connection?: string;
    };
    const source = config.plan_connection
      ? "plan_connection"
      : config.provider_credential
        ? "provider_credential"
        : "keyless";
    if (
      source === "plan_connection" &&
      (!selfHostedPlanConnections || access.kind !== "subject")
    )
      continue;
    const provider =
      config.plan_connection ??
      config.provider_credential ??
      config.provider ??
      "openai";
    const key = `${source}:${provider}`;
    if (!groups.has(key)) groups.set(key, { provider, source, entries: [] });
    groups.get(key)!.entries.push(entry);
  }
  const source =
    selectedSource ??
    (groups.size === 1 ? groups.keys().next().value : "") ??
    "";
  const group = groups.get(source);
  const sourceLabel = (value: string) =>
    value === "plan_connection"
      ? locale === "ja-JP"
        ? "個人 Plan 接続"
        : "Personal Plan Connection"
      : value === "provider_credential"
        ? locale === "ja-JP"
          ? "Tenant のプロバイダ認証情報"
          : "Tenant Provider Credential"
        : "Keyless";
  const providerLabel = (value: string) =>
    value === "chatgpt"
      ? "ChatGPT"
      : value === "openrouter"
        ? "OpenRouter"
        : value === "openai"
          ? t("openaiCompatible")
          : value;
  const validModel = group?.entries.some(
    (entry) => `${entry.id}@${entry.version}` === selectedModel,
  )
    ? selectedModel
    : "";
  return (
    <>
      <Field label={t("provider")}>
        <NativeSelect
          value={source}
          required
          onChange={(event) => {
            selectSource(event.target.value);
            selectModel("");
          }}
        >
          <option value="">{t("choose")}</option>
          {[...groups].map(([key, value]) => (
            <option key={key} value={key}>
              {providerLabel(value.provider)} · {sourceLabel(value.source)}
            </option>
          ))}
        </NativeSelect>
      </Field>
      {group?.source === "plan_connection" && access.kind === "subject" && (
        <PersonalPlanGuidance tenant={access.tenant} subject={access.subject} />
      )}
      <Field label={t("model")}>
        <NativeSelect
          name="model"
          required
          value={validModel}
          disabled={!group}
          onChange={(event) => selectModel(event.target.value)}
        >
          <option value="">{t("choose")}</option>
          {group?.entries.map((entry) => (
            <option
              key={`${entry.id}@${entry.version}`}
              value={`${entry.id}@${entry.version}`}
            >
              {label(entry)}
            </option>
          ))}
        </NativeSelect>
      </Field>
    </>
  );
}
