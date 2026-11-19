import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "./transport";
import { Panel, useI18n } from "./ui";
import { Alert, Hint, Loading } from "./components/patterns";
import { Button } from "./components/ui/button";

type Level =
  | "no_restriction"
  | "no_training"
  | "safety_retention"
  | "zero_data_retention";
type Policy = {
  level: Level;
  forbid_plan_connections: boolean;
  forbid_keyless_endpoints: boolean;
};
type Settings = { policy: Policy; effective: Policy; deployment_floor: Level };
type Affected = { id: string; version: string | null; reason: string };
type Impact = {
  effective: Policy;
  agents: Affected[];
  catalog_approvals: Affected[];
  destinations: Affected[];
};
const levels: Level[] = [
  "no_restriction",
  "no_training",
  "safety_retention",
  "zero_data_retention",
];

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
  const [confirmed, setConfirmed] = useState(false);
  const labels = {
    no_restriction: "No Restriction",
    no_training: "No Training",
    safety_retention: "Safety Retention",
    zero_data_retention: "Zero Data Retention",
  };
  const restrictions: Record<Level, string> = ja
    ? {
        no_restriction:
          "OpenRouter に学習・保持の条件を求めません。Provider tools を使えるのはこのレベルだけです。",
        no_training:
          "学習に使わない送信先だけに送ります（data_collection: deny）。",
        safety_retention:
          "保持しない送信先か、不正利用の防止のためだけに保持する送信先（Safety Retention）に送ります。OpenRouter の ZDR 一覧を毎回確認します。",
        zero_data_retention:
          "保持しない送信先だけに送ります。Safety Retention の送信先は除きます。OpenRouter の ZDR 一覧を毎回確認し、満たす経路がないモデルは呼び出しません。",
      }
    : {
        no_restriction:
          "Asks OpenRouter for no training or retention terms. Provider tools are available only at this level.",
        no_training:
          "Sends only to endpoints that never train on content (data_collection: deny).",
        safety_retention:
          "Sends only to endpoints that keep nothing, or keep content only to prevent abuse (Safety Retention). Each call checks OpenRouter's current ZDR list.",
        zero_data_retention:
          "Sends only to endpoints that keep nothing; Safety Retention endpoints are excluded. Each call checks OpenRouter's current ZDR list, and a model with no such route is not called.",
      };
  const frozen = !canUpdate || save.isPending;
  const affected = preview.data
    ? preview.data.agents.length +
      preview.data.catalog_approvals.length +
      preview.data.destinations.length
    : 0;
  // Raising the level stops what cannot meet it; the person confirms first (#963).
  const raising =
    levels.indexOf(policy.level) > levels.indexOf(settings.policy.level);
  const needsConfirmation = raising && affected > 0;
  return (
    <div className="space-y-4">
      <Hint>
        {ja
          ? `Deployment の最低レベル: ${labels[settings.deployment_floor]}`
          : `Deployment floor: ${labels[settings.deployment_floor]}`}
      </Hint>
      <fieldset disabled={frozen} className="space-y-3">
        <legend className="text-sm font-medium">
          {ja
            ? "OpenRouter へのモデル呼び出しに求める条件"
            : "What every OpenRouter model call asks for"}
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
              onChange={() => {
                setConfirmed(false);
                setPolicy({ ...policy, level });
              }}
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
      <Hint>
        {ja
          ? "このレベルは OpenRouter 経由のモデル呼び出し（Run、Decider、圧縮、埋め込み）だけに適用されます。Tenant の HTTP／MCP ツール、Agent Plugin のリモート MCP、ページ読み取りは対象外です。OpenRouter は ZDR を含むすべての通信で児童性的虐待コンテンツ（CSAM）を検査し、該当した内容を審査と法的保全のために保持することがあります。どのレベルでもこの例外は避けられません。"
          : "The level applies only to model calls through OpenRouter (Runs, Deciders, compaction, embeddings). Tenant HTTP/MCP tools, Agent Plugin remote MCP servers and page reading are outside it. OpenRouter screens all traffic, zero data retention included, for child sexual abuse material and may keep flagged content for review and legal preservation; no level avoids that exception."}
      </Hint>
      {policy.level === "zero_data_retention" && (
        <Hint>
          {ja
            ? "二重の安全策として、Tenant 自身の OpenRouter アカウントも「ZDR only」に設定することをおすすめします。"
            : 'As a second safeguard, set the Tenant\'s own OpenRouter account to "ZDR only" as well.'}
        </Hint>
      )}
      <fieldset disabled={frozen} className="space-y-2">
        <legend className="text-sm font-medium">
          {ja ? "OpenRouter 以外の送信先" : "Destinations outside OpenRouter"}
        </legend>
        <label className="flex gap-2 text-sm">
          <input
            type="checkbox"
            checked={policy.forbid_plan_connections}
            onChange={(event) => {
              setConfirmed(false);
              setPolicy({
                ...policy,
                forbid_plan_connections: event.target.checked,
              });
            }}
          />
          {ja
            ? "ChatGPT サブスクリプション（Plan Connection）を禁止する"
            : "Forbid ChatGPT subscriptions (Plan Connections)"}
        </label>
        <Hint>
          {ja
            ? "ChatGPT サインインには API の ZDR 契約が適用されません。個人プランでは、本人がオプトアウトしない限り内容が学習に使われる場合があります。Plan Connection が内容を運んだ Run には記録が残ります。"
            : "ChatGPT sign-in carries no API ZDR agreement, and personal plans may train on content unless the person opts out. Runs record when a Plan Connection carried their content."}
        </Hint>
        <label className="flex gap-2 text-sm">
          <input
            type="checkbox"
            checked={policy.forbid_keyless_endpoints}
            onChange={(event) => {
              setConfirmed(false);
              setPolicy({
                ...policy,
                forbid_keyless_endpoints: event.target.checked,
              });
            }}
          />
          {ja ? "Keyless endpoint を禁止する" : "Forbid Keyless endpoints"}
        </label>
        <Hint>
          {ja
            ? "Aidash は Keyless endpoint の保持について何も約束しません。Deployment が自ら動かすモデルは、内容が Deployment の外に出ないため禁止されません。どちらのスイッチも、委任チェーンのどれか1つの Tenant がオンにしていれば適用され、オンにした直後の呼び出しから拒否されます。"
            : "Aidash promises nothing about what a Keyless endpoint keeps. Deployment-hosted models are never forbidden, since nothing sent to them leaves the Deployment. Either switch applies when any Tenant in a delegation chain turns it on, and refuses the next call."}
        </Hint>
      </fieldset>
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
      {save.isError && <Alert>{String(save.error)}</Alert>}
      {needsConfirmation && (
        <label className="flex gap-2 text-sm">
          <input
            type="checkbox"
            checked={confirmed}
            disabled={frozen}
            onChange={(event) => setConfirmed(event.target.checked)}
          />
          {ja
            ? "上に挙げたものが次の呼び出しから使えなくなることを確認しました"
            : "I understand that everything listed above stops working from its next call"}
        </label>
      )}
      {canUpdate && (
        <Button
          disabled={
            save.isPending ||
            !preview.isSuccess ||
            (needsConfirmation && !confirmed)
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
