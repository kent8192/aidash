import { keylessOrigin } from "./keyless";
import { useI18n } from "./ui";
import type { Entry, Run, State } from "./types";
import { ManagePlanUsage } from "./plan-connections";
import { Notice } from "./components/patterns";

export function ModelSource({
  entry,
}: {
  entry: Pick<Entry, "kind" | "config">;
}) {
  const { locale } = useI18n();
  if (entry.kind !== "model" && entry.kind !== "embedding") return null;
  const config = entry.config as {
    provider_credential?: string;
    plan_connection?: string;
  } | null;
  return (
    <span className="text-xs text-muted-foreground">
      {config?.plan_connection
        ? locale === "ja-JP"
          ? "個人 Plan 接続"
          : "Personal Plan Connection"
        : config?.provider_credential
          ? locale === "ja-JP"
            ? "Tenant のプロバイダ認証情報"
            : "Tenant Provider Credential"
          : keylessOrigin(entry)
            ? "Keyless"
            : ""}
    </span>
  );
}
export function RunPlanUsage({ run, data }: { run: Run; data: State }) {
  const { locale } = useI18n();
  const agent = data.registry.find(
    (entry) =>
      entry.kind === "agent" &&
      entry.id === run.agent_id &&
      entry.version === run.agent_version,
  );
  const ref = (
    agent?.config as { model?: { id: string; version: string } } | undefined
  )?.model;
  const model =
    ref &&
    data.registry.find(
      (entry) =>
        entry.kind === "model" &&
        entry.id === ref.id &&
        entry.version === ref.version,
    );
  if (
    !(model?.config as { plan_connection?: string } | undefined)
      ?.plan_connection
  )
    return null;
  return (
    <Notice>
      <span>
        {locale === "ja-JP" ? "ChatGPT Plan を使用" : "Using ChatGPT plan"}
      </span>
      {" · "}
      <ManagePlanUsage />
      <p>
        {locale === "ja-JP"
          ? "失敗や使用量の上限到達時は Run を停止し、他の認証情報には切り替えません。"
          : "Failures and usage limits stop the Run without falling back to other credentials."}
      </p>
    </Notice>
  );
}
