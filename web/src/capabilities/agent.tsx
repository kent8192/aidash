import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "../transport";
import { Field, useI18n } from "../ui";
import { post } from "./client";
import {
  CapabilityConfiguration,
  emptyCore,
  type CoreConfiguration,
} from "./configuration";
type Agent = {
  id: string;
  version: string;
  name: Record<string, string>;
  config: Partial<CoreConfiguration>;
};
export function AgentCapabilities() {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const query = useQuery({
    queryKey: ["core-configurable-agents"],
    queryFn: ({ signal }) =>
      apiFetch<Agent[]>("/api/registry?kind=agent", { signal }),
    retry: false,
  });
  const [agent, setAgent] = useState<Agent>();
  const [configuration, setConfiguration] =
    useState<CoreConfiguration>(emptyCore);
  const [version, setVersion] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState("");
  const request = useRef<{ body: string; key: string } | undefined>(undefined);
  return (
    <details className="core-panel">
      <summary>
        {ja
          ? "Agent の実行機能・Skill・参照原本"
          : "Agent capabilities, Skills and original references"}
      </summary>
      <Field label={ja ? "設定を引き継ぐ Agent" : "Source Agent version"}>
        <select
          value={agent ? `${agent.id}@${agent.version}` : ""}
          onChange={(e) => {
            const next = query.data?.find(
              (a) => `${a.id}@${a.version}` === e.target.value,
            );
            setAgent(next);
            setSaved("");
            setConfiguration({
              core_capabilities: {
                ...emptyCore.core_capabilities,
                ...next?.config.core_capabilities,
              },
              skill_attachments: next?.config.skill_attachments ?? [],
              skill_roots: next?.config.skill_roots ?? [],
              reference_attachments: next?.config.reference_attachments ?? [],
            });
            setVersion(
              next?.version.replace(/(\d+)$/, (n) => String(Number(n) + 1)) ??
                "",
            );
          }}
        >
          <option value="">{ja ? "Agent を選択" : "Select an Agent"}</option>
          {query.data?.map((a) => (
            <option key={`${a.id}@${a.version}`} value={`${a.id}@${a.version}`}>
              {a.name.ja ?? a.name.en ?? a.id} · {a.version}
            </option>
          ))}
        </select>
      </Field>
      {agent && (
        <>
          <Field
            label={ja ? "保存する新しいバージョン" : "New immutable version"}
          >
            <input
              value={version}
              onChange={(e) => setVersion(e.target.value)}
            />
          </Field>
          <CapabilityConfiguration
            privateReferences
            value={configuration}
            change={setConfiguration}
          />
          <p>
            {ja
              ? "変更にはこの Agent の設定権限が必要です。保存後も新バージョンの利用承認が必要で、機能を有効にするだけでは実行権限は増えません。"
              : "Configuring this Agent requires permission. The new version still needs catalog approval; enabling a feature creates no execution grant."}
          </p>
          <button
            type="button"
            disabled={busy || !version || version === agent.version}
            onClick={async () => {
              setBusy(true);
              setError("");
              try {
                const body = {
                  source_version: agent.version,
                  new_version: version,
                  ...configuration,
                };
                const signature = JSON.stringify(body);
                if (request.current?.body !== signature)
                  request.current = {
                    body: signature,
                    key: crypto.randomUUID(),
                  };
                await post(
                  `/agents/${encodeURIComponent(agent.id)}/capabilities`,
                  { ...body, idempotency_key: request.current.key },
                );
                setSaved(
                  ja
                    ? `${version} を保存しました。管理者の利用承認後に選択できます。`
                    : `Saved ${version}. It becomes available after catalog approval.`,
                );
                await client.invalidateQueries({
                  queryKey: ["core-configurable-agents"],
                });
              } catch (e) {
                setError(String(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            {ja ? "新しいバージョンとして保存" : "Save a new version"}
          </button>
        </>
      )}
      {saved && <p role="status">{saved}</p>}
      {(error || query.isError) && (
        <p role="alert">{error || query.error?.message}</p>
      )}
    </details>
  );
}
