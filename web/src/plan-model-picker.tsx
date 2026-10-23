import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Field, useI18n } from "./ui";
import { Hint, Alert, Notice, pairClass } from "./components/patterns";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { dashboardContext, transportGeneration } from "./transport";
import { planConnectionApi } from "@slots/plan-connections";
import { usePlanConnection } from "./plan-connections";

export function PlanModelPicker({
  tenant,
  subject,
  onNameChange,
}: {
  tenant: string;
  subject: string;
  onNameChange: (name: string) => void;
}) {
  const { t, locale } = useI18n();
  const ja = locale === "ja-JP";
  const status = usePlanConnection(tenant, subject);
  const [chosen, choose] = useState("");
  const usable =
    !status.isError &&
    status.data?.enabled === true &&
    status.data.connection?.state === "active";
  const models = useQuery({
    queryKey: [
      "personal-plan-models",
      tenant,
      subject,
      dashboardContext(),
      transportGeneration(),
      status.data?.connection?.revision,
    ],
    queryFn: ({ signal }) => planConnectionApi.models(tenant, { signal }),
    enabled: usable,
    gcTime: 0,
    retry: false,
  });
  const model = usable
    ? models.data?.models.find((value) => value.slug === chosen)
    : undefined;
  return (
    <>
      <Notice>
        {ja
          ? "登録したモデルも、各 Run を開始した Subject 自身の Plan 接続が必要です。登録によって他の Subject にプランの使用権限を与えることはありません。"
          : "Each Run still needs its initiating subject's own Plan Connection. Registering a model grants no other subject access to your plan."}
      </Notice>
      {(!status.data?.enabled || status.data.connection?.state !== "active") &&
        !status.isPending && (
          <Notice tone="warning">
            <a href="/settings?view=planConnections" className="underline">
              {ja
                ? "個人設定で利用可能な ChatGPT Plan を接続してください。"
                : "Connect an eligible ChatGPT plan in personal settings."}
            </a>
          </Notice>
        )}
      {(status.isError || models.isError) && (
        <Alert>
          {ja
            ? "利用可能な Plan モデルを確認できません。接続状態を確認してください。"
            : "Could not load eligible plan models. Check your connection status."}
        </Alert>
      )}
      <Field label={t("modelId")}>
        <NativeSelect
          name="model_id"
          required
          value={model ? chosen : ""}
          disabled={!usable || !models.data}
          onChange={(event) => {
            choose(event.target.value);
            onNameChange(`chatgpt-${event.target.value}`);
          }}
        >
          <option value="">{t("choose")}</option>
          {usable &&
            models.data?.models.map((value) => (
              <option key={value.slug} value={value.slug}>
                {value.display_name} · {value.slug}
              </option>
            ))}
        </NativeSelect>
      </Field>
      {model?.defaulted && (
        <Notice tone="warning">
          {ja
            ? "プロバイダが上限を返さなかったため、暫定値を表示しています。登録前に確認してください。"
            : "The provider did not report limits. These are default values; review them before registering."}
        </Notice>
      )}
      <div className={pairClass} key={chosen}>
        <Field label={t("contextWindow")}>
          <Input
            name="context_window"
            type="number"
            min={1}
            required
            defaultValue={model?.context_window ?? ""}
          />
        </Field>
        <Field label={t("modelMaxOutputTokens")}>
          <Input
            name="max_output_tokens"
            type="number"
            min={1}
            required
            defaultValue={model?.max_output_tokens ?? ""}
          />
        </Field>
      </div>
      <Field label="Reasoning Effort">
        <NativeSelect name="reasoning_effort" defaultValue="">
          <option value="">{t("modelReasoningDefault")}</option>
          {["none", "minimal", "low", "medium", "high", "xhigh", "max"].map(
            (value) => (
              <option key={value}>{value}</option>
            ),
          )}
        </NativeSelect>
      </Field>
      <Hint>
        {ja
          ? "利用できない設定はプロバイダで拒否されます。自動的に他のモデルへ切り替えません。Plan 使用はストリーミングで実行し、推論出力を保存しません。"
          : "The provider rejects unsupported options; Aidash never switches to another model. Plan usage streams responses and does not retain reasoning output."}
      </Hint>
      <input type="hidden" name="endpoint" value="https://api.openai.com/v1" />
      <input type="hidden" name="modalities" value='["text"]' />
      <input type="hidden" name="cost" value="{}" />
    </>
  );
}
