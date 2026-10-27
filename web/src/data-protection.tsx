import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "./transport";
import { Panel, useI18n } from "./ui";
import { Alert, Hint, Loading } from "./components/patterns";
import { Button } from "./components/ui/button";

type Level = "standard" | "no_training" | "zero_data_retention";
type Policy = {
  data_region_restricted?: boolean;
  level: Level;
  model_call_zdr: boolean;
  plan_connection_exempt: boolean;
};
type Settings = {
  policy: Policy;
  effective: Policy;
  deployment_floor: Level;
  deployment_region: "eu" | "us" | "jp" | null;
};
type Affected = { id: string; version: string | null; reason: string };
type Impact = {
  effective: Policy;
  usable_models: number;
  agents: Affected[];
  catalog_approvals: Affected[];
  destinations: Affected[];
};
const levels: Level[] = ["standard", "no_training", "zero_data_retention"];

export function DataProtectionSettings({
  tenant,
  canUpdate,
}: {
  tenant: string;
  canUpdate: boolean;
}) {
  const { locale } = useI18n();
  const query = useQuery({
    queryKey: ["data-protection", tenant],
    queryFn: () =>
      apiFetch<Settings>(
        `/api/tenants/${encodeURIComponent(tenant)}/data-protection`,
      ),
    retry: false,
  });
  return (
    <Panel
      title={locale === "ja-JP" ? "データ保護レベル" : "Data Protection Level"}
    >
      {query.isError && <Alert>{String(query.error)}</Alert>}
      {query.isPending && <Loading />}
      {query.data && (
        <PolicyForm
          key={JSON.stringify([tenant, query.data])}
          tenant={tenant}
          settings={query.data}
          canUpdate={canUpdate}
        />
      )}
    </Panel>
  );
}

function PolicyForm({
  tenant,
  settings,
  canUpdate,
}: {
  tenant: string;
  settings: Settings;
  canUpdate: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [policy, setPolicy] = useState(settings.effective);
  const url = `/api/tenants/${encodeURIComponent(tenant)}/data-protection`;
  const send = (method: string, body: Policy) => ({
    method,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  const preview = useQuery({
    queryKey: ["data-protection-impact", tenant, policy],
    queryFn: () => apiFetch<Impact>(`${url}/impact`, send("POST", policy)),
    retry: false,
  });
  const save = useMutation({
    mutationFn: () => apiFetch<Settings>(url, send("PUT", policy)),
    onSuccess: async () => {
      await client.invalidateQueries({ queryKey: ["data-protection", tenant] });
    },
  });
  const labels = {
    standard: "Standard",
    no_training: "No Training",
    zero_data_retention: "Zero Data Retention",
  };
  const restrictions: Record<Level, string> = ja
    ? {
        standard:
          "送信先の学習・保持に制限を設けません。モデル呼出しの ZDR は既定でオンです。",
        no_training:
          "Tenant の Keyless エンドポイント、Provider tools、学習禁止が確認されていない Research サービスを使用できません。",
        zero_data_retention:
          "No Training の制限に加え、リクエストの無保持が確認されていない Research サービスを使用できません。OpenRouter の ZDR 対応経路を毎回確認します。",
      }
    : {
        standard:
          "No restriction on destination training or retention. Model-call ZDR starts on.",
        no_training:
          "Blocks Tenant Keyless endpoints, Provider tools and Research services without recorded no-training terms.",
        zero_data_retention:
          "Also blocks Research services without recorded no-retention terms. Each OpenRouter call checks current ZDR routes.",
      };
  const frozen = !canUpdate || save.isPending;
  return (
    <div className="space-y-4">
      <Hint>
        {ja
          ? `Deployment の最低レベル: ${labels[settings.deployment_floor]}`
          : `Deployment floor: ${labels[settings.deployment_floor]}`}
      </Hint>
      <fieldset disabled={frozen} className="space-y-3">
        <legend className="text-sm font-medium">
          {ja ? "レベルと利用制限" : "Level and restrictions"}
        </legend>
        {levels.map((level) => (
          <label
            key={level}
            htmlFor={`data-protection-${tenant}-${level}`}
            aria-label={`${labels[level]}: ${restrictions[level]}`}
            className="flex gap-3 text-sm"
          >
            <input
              type="radio"
              id={`data-protection-${tenant}-${level}`}
              name={`data-protection-${tenant}`}
              value={level}
              checked={policy.level === level}
              disabled={
                levels.indexOf(level) <
                levels.indexOf(settings.deployment_floor)
              }
              onChange={() =>
                setPolicy({
                  ...policy,
                  level,
                  model_call_zdr:
                    level === "zero_data_retention" || policy.model_call_zdr,
                })
              }
            />
            <span>
              <strong>{labels[level]}</strong>
              <span className="mt-1 block text-muted-foreground">
                {restrictions[level]}
              </span>
            </span>
          </label>
        ))}
      </fieldset>
      <fieldset disabled={frozen} className="space-y-2">
        <legend className="text-sm font-medium">
          {ja ? "データの送信地域" : "Data Region restriction"}
        </legend>
        <label className="flex gap-2 text-sm">
          <input
            type="radio"
            name={`data-region-${tenant}`}
            checked={!policy.data_region_restricted}
            onChange={() =>
              setPolicy({ ...policy, data_region_restricted: false })
            }
          />
          Any (Recommended)
        </label>
        <label className="flex gap-2 text-sm">
          <input
            type="radio"
            name={`data-region-${tenant}`}
            checked={!!policy.data_region_restricted}
            disabled={!settings.deployment_region}
            onChange={() =>
              setPolicy({ ...policy, data_region_restricted: true })
            }
          />
          {settings.deployment_region?.toUpperCase() ??
            (ja ? "Deployment Region 未宣言" : "No Deployment Region declared")}
        </label>
        <Hint>
          {ja
            ? "オンにすると地域外の OpenRouter モデルと Provider tools、地域が記録されていない Research サービス、Tenant の Keyless が利用できなくなります。JP には OpenRouter の地域エンドポイントがありません。Plan Connection は免除時のみ許可され、残るモデルの数には含まれません。下のプレビューは現在の地域カタログを確認します。"
            : "Enabling blocks OpenRouter models and Provider tools unavailable in-region, Research services without matching regional terms, and Tenant Keyless endpoints. OpenRouter has no JP endpoint. Plan Connections require the exemption and never count as a remaining model. The preview below checks the current regional catalog."}
        </Hint>
        <Hint>
          {ja
            ? "Deployment Region は DB、Secret Store、バックアップ、ログ、Execution Pods、Egress gateway、broker を含みます。External Identities、Billing Accounts、Marketplace は Deployment 間で共有され、地域の対象外です。Self-hosted の宣言は Aidash では検証できません。OpenRouter の EU は EU/EEA・英国・スイスを含み、アカウント・請求・認証・運用メタデータは地域外で扱われる場合があります。BYOK の処理地域と observability 送信先は OpenRouter アカウント側でも確認してください。"
            : "The Deployment Region covers its database, Secret Store, backups, logs, Execution Pods, Egress gateway and broker. External Identities, Billing Accounts and Marketplace are shared outside this boundary. Aidash cannot verify Self-hosted declarations. OpenRouter Europe includes EU/EEA, UK and Switzerland; account, billing, authentication and operational metadata may remain outside the region. Verify BYOK deployment regions and observability destinations in your OpenRouter account."}
        </Hint>
      </fieldset>
      <fieldset className="space-y-2">
        <legend className="text-sm font-medium">
          {ja ? "OpenRouter モデル呼出しの ZDR" : "OpenRouter model-call ZDR"}
        </legend>
        <label className="flex gap-2 text-sm">
          <input
            type="checkbox"
            checked={policy.model_call_zdr}
            disabled={frozen || policy.level === "zero_data_retention"}
            onChange={(event) =>
              setPolicy({ ...policy, model_call_zdr: event.target.checked })
            }
          />
          {ja ? "ゼロデータ保持を要求する" : "Require zero data retention"}
        </label>
        <Hint>
          {ja
            ? "Standard でオフにするとデータポリシーを送りません。No Training でオフにすると data_collection: deny を要求します。この指定は非一時的な保持も除外します。Zero Data Retention ではオフにできません。"
            : "When off, Standard sends no data policy; No Training requests data_collection: deny, which also excludes non-transient retention. Zero Data Retention requires this switch on."}
        </Hint>
      </fieldset>
      <fieldset className="space-y-2">
        <legend className="text-sm font-medium">
          {ja ? "Plan Connection の免除" : "Plan Connection exemption"}
        </legend>
        <label className="flex gap-2 text-sm">
          <input
            type="checkbox"
            checked={policy.plan_connection_exempt}
            disabled={frozen}
            onChange={(event) =>
              setPolicy({
                ...policy,
                plan_connection_exempt: event.target.checked,
              })
            }
          />
          {ja
            ? "Plan Connection をレベルと送信地域の制限から免除する"
            : "Exempt Plan Connections from the level and Data Region restriction"}
        </label>
        <Hint>
          {ja
            ? "ChatGPT サインインには API の ZDR 契約が適用されません。個人プランでは学習に使われる場合があります。免除は Run に記録され、委譲先を含む全 Tenant の許可が必要です。地域制限のない Standard ではオフでも使用でき、免除は記録されません。モデル呼出しの ZDR は適用されません。"
            : "ChatGPT sign-in does not inherit API ZDR agreements; personal plans may train on content. Runs record the exemption, and every Tenant carrying content in a delegation must grant it. Without a region restriction, Standard permits use with this switch off and records no exemption. Model-call ZDR does not apply."}
        </Hint>
      </fieldset>
      <Hint>
        {ja
          ? "OpenRouter アカウントの入出力ログ等の設定は Tenant 自身で管理してください。Tenant の HTTP／MCP tools と Agent Plugin のリモート MCP はこのポリシーの対象外です。"
          : "The Tenant manages its own OpenRouter account settings, including Input & Output Logging. Tenant HTTP/MCP tools and Agent Plugin remote MCP servers are outside this policy."}
      </Hint>
      <div aria-live="polite">
        <h4 className="text-sm font-medium">
          {ja
            ? "この設定で利用できなくなるもの"
            : "Unavailable with this policy"}
        </h4>
        {preview.isPending && <Loading />}
        {preview.isError && <Alert>{String(preview.error)}</Alert>}
        {preview.data &&
          (
            [
              [ja ? "Agents" : "Agents", preview.data.agents],
              [
                ja ? "Catalog Approvals" : "Catalog Approvals",
                preview.data.catalog_approvals,
              ],
              [ja ? "送信先" : "Destinations", preview.data.destinations],
            ] as const
          ).map(([label, items]) => (
            <div key={label} className="mt-2 text-sm">
              <strong>{label}</strong>
              {items.length ? (
                <ul className="list-disc pl-5">
                  {items.map((item, index) => (
                    <li key={`${item.id}-${item.version}-${index}`}>
                      {item.id}
                      {item.version ? `@${item.version}` : ""} — {item.reason}
                    </li>
                  ))}
                </ul>
              ) : (
                <Hint>{ja ? "該当なし" : "None"}</Hint>
              )}
            </div>
          ))}
      </div>
      {policy.data_region_restricted && preview.data?.usable_models === 0 && (
        <Alert>
          {ja
            ? "利用可能なモデルが残らないため、地域制限を有効にできません。"
            : "Cannot enable the restriction: no usable model remains."}
        </Alert>
      )}
      {save.isError && <Alert>{String(save.error)}</Alert>}
      {canUpdate && (
        <Button
          disabled={
            save.isPending ||
            !preview.isSuccess ||
            (!!policy.data_region_restricted &&
              preview.data?.usable_models === 0)
          }
          onClick={() => save.mutate()}
        >
          {ja ? "保存" : "Save"}
        </Button>
      )}
      <Hint>
        {ja
          ? "変更後のポリシーは既存 Run にも次の送信から適用されます。"
          : "The current policy applies to existing Runs from their next send."}
      </Hint>
    </div>
  );
}
