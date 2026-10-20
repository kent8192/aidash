import { useQuery } from "@tanstack/react-query";
import { registryDeployment } from "../node-selection";
import { Hint, Notice } from "../components/patterns";
import { Button } from "../components/ui/button";
import type { ResearchProviders, RunResearch } from "../generated/models";
import {
  MAX_URL_INTENTS,
  dollars,
  researchBindings,
  type ResearchBinding,
} from "../research";
import { apiFetch } from "../transport";
import type { State } from "../types";
import { useI18n } from "../ui";
import { every } from "./inbox";

type Agent = State["registry"][number];

const OPERATION: Record<string, [string, string]> = {
  search: ["Web search", "Web 検索"],
  fetch: ["Page reading", "ページ読み取り"],
  find: ["Find in a read page", "読んだページ内の検索"],
};

/** Why a bound research operation can or cannot run here. */
export type Readiness = "ready" | "not_set_up" | "not_approved" | "unknown";

/**
 * The research operations an Agent binds and whether each can run here: its
 * Research provider must be set up in this Deployment, and this Tenant must
 * have a Catalog Approval for its definition. Only then is a URL intent
 * offered for Page reading.
 */
export function useResearch(agent: Agent | undefined, data: State) {
  const bound = researchBindings(
    agent,
    data.registry,
    registryDeployment(data),
  );
  const providers = useQuery({
    queryKey: ["research-providers"],
    queryFn: ({ signal }) =>
      apiFetch<ResearchProviders>("/api/research/providers", { signal }),
    enabled: bound.length > 0,
    // Another administrator may approve or revoke a definition at any time,
    // and no event announces it here, so readiness is read again.
    refetchInterval: 30_000,
    retry: false,
  });
  const readiness = (binding: ResearchBinding): Readiness => {
    const provider = providers.data?.providers.find(
      (item) => item.provider === binding.provider,
    );
    if (!provider) return "unknown";
    if (!provider.set_up) return "not_set_up";
    const approved =
      provider.operations.find((item) => item.operation === binding.operation)
        ?.approved_definitions ?? [];
    // Ready only through a bound definition this Tenant has approved here.
    return binding.definitions.some((definition) =>
      approved.some(
        (item) =>
          item.id === definition.id && item.version === definition.version,
      ),
    )
      ? "ready"
      : "not_approved";
  };
  return {
    bound,
    providers: providers.data,
    readiness,
    canRead: bound.some(
      (binding) =>
        binding.operation === "fetch" && readiness(binding) === "ready",
    ),
  };
}

const READINESS: Record<Readiness, [string, string]> = {
  ready: ["Ready", "利用できます"],
  not_set_up: [
    "Not set up in this Deployment",
    "この Deployment では未設定です",
  ],
  not_approved: ["Not approved for use here", "ここでの利用は未承認です"],
  unknown: ["…", "…"],
};

/**
 * The Web research of an Agent version in a Thread: its bound research
 * operations, their availability on this Node, the per-Run limits and, for a
 * Run, what it has used and which disclosures it is waiting on. `next` marks
 * the version selected for the next Run when it differs from the shown Run's.
 */
export function ResearchPanel({
  research,
  run,
  next = false,
}: {
  research: ReturnType<typeof useResearch>;
  run?: string | null;
  next?: boolean;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const usage = useQuery({
    queryKey: ["run-research", run],
    queryFn: ({ signal }) =>
      apiFetch<RunResearch>(`/api/runs/${run}/research`, { signal }),
    enabled: !!run && research.bound.length > 0,
    refetchInterval: 5000,
    retry: false,
  });
  const waiting = useQuery({
    queryKey: ["core-approvals", "run-waiting", run],
    queryFn: ({ signal }) =>
      every("/api/capabilities/approvals?state=pending", signal),
    enabled: !!run && research.bound.length > 0,
    refetchInterval: 5000,
    retry: false,
  });
  if (research.bound.length === 0) return null;
  const limits = usage.data?.limits ?? research.providers?.limits;
  const used = usage.data?.usage;
  const pending = (waiting.data ?? []).filter(
    (item) => item.kind === "disclosure" && item.run_id === run,
  );
  const of = (value: number | undefined, limit: number | undefined) =>
    used ? `${value}/${limit}` : `${limit}`;
  const title = next
    ? ja
      ? "次の Run の Web 調査"
      : "Web research for the next Run"
    : ja
      ? "Web 調査"
      : "Web research";
  return (
    <section aria-label={title} className="grid min-w-0 gap-1.5">
      <h4 className="text-[13px] font-semibold">{title}</h4>
      <ul className="grid gap-0.5">
        {research.bound.map((binding) => (
          <li key={`${binding.provider} ${binding.operation}`}>
            {(OPERATION[binding.operation] ?? [binding.operation])[ja ? 1 : 0]}{" "}
            · <code>{binding.provider}</code> ·{" "}
            {READINESS[research.readiness(binding)][ja ? 1 : 0]}
          </li>
        ))}
      </ul>
      {limits && (
        <Hint role={used ? "status" : undefined}>
          {used
            ? ja
              ? "この Run の使用量"
              : "This Run"
            : ja
              ? "Run ごとの上限"
              : "Per-Run limits"}
          : {ja ? "検索" : "searches"}{" "}
          {of(used?.search_attempts, limits.search_attempts)} ·{" "}
          {ja ? "ページ読み取り" : "page readings"}{" "}
          {of(used?.fetch_attempts, limits.fetch_attempts)} ·{" "}
          {ja ? "Source record" : "sources"} {of(used?.sources, limits.sources)}{" "}
          · {ja ? "スナップショット" : "snapshots"}{" "}
          {of(used?.snapshots, limits.snapshots)} ·{" "}
          {ja ? "ページ画像の閲覧" : "page views"}{" "}
          {of(used?.page_views, limits.page_views)}
          {usage.data &&
            ` · ${ja ? "推定調査費用" : "estimated research cost"} ${dollars(usage.data.estimated_micro_usd)} ${ja ? "（見積もり。請求額ではなく、Research account holder が支払います）" : "(an estimate, not an invoice; the Research account holder pays)"}`}
        </Hint>
      )}
      {usage.data?.revoked_at && (
        <Notice role="status">
          {ja
            ? "この Run の Research evidence は取り消されたため、調査は終了しています。"
            : "This Run's research evidence was revoked, so its research is closed."}
        </Notice>
      )}
      {(usage.data?.uncertain_attempts ?? 0) > 0 && (
        <Notice tone="warning">
          {ja
            ? `結果が不明な試行が ${usage.data!.uncertain_attempts} 件あります。自動では再送しません。`
            : `${usage.data!.uncertain_attempts} attempts have an unknown outcome. They are never sent again automatically.`}
        </Notice>
      )}
      {pending.map((item) => (
        <Hint key={item.id} role="status">
          {ja ? "Web 開示の承認待ち" : "Waiting for Web disclosure approval by"}
          {ja ? "：" : " "}
          {item.approver ??
            (ja ? "承認できる人がいません" : "no eligible approver")}{" "}
          · <code>{item.targets.join(", ")}</code>
        </Hint>
      ))}
      {(usage.data?.url_intents ?? []).map((intent) => (
        <Hint key={`${intent.requested_at} ${intent.pages.join(" ")}`}>
          {ja ? "読み取りの依頼" : "Asked to read"}
          {ja ? "（" : " ("}
          {intent.requested_by}
          {ja ? "）：" : "): "}
          {intent.pages.map((page) => (
            <code key={page} className="mr-1 [overflow-wrap:anywhere]">
              {page}
            </code>
          ))}
        </Hint>
      ))}
      {usage.isError && <Hint>{usage.error.message}</Hint>}
    </section>
  );
}

/**
 * One chip per readable URL in the input. Selecting one asks the Run to read
 * that page; the Web disclosure it leads to still needs its own approval.
 */
export function UrlIntentPicker({
  candidates,
  selected,
  onChange,
}: {
  candidates: string[];
  selected: string[];
  onChange: (pages: string[]) => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  if (candidates.length === 0) return null;
  return (
    <div
      role="group"
      aria-label={ja ? "このページを読む" : "Read this page"}
      className="flex flex-wrap items-center gap-1.5"
    >
      {candidates.map((url) => {
        const on = selected.includes(url);
        return (
          <Button
            key={url}
            type="button"
            size="sm"
            variant={on ? "default" : "outline"}
            aria-pressed={on}
            disabled={!on && selected.length >= MAX_URL_INTENTS}
            onClick={() =>
              onChange(
                on
                  ? selected.filter((page) => page !== url)
                  : [...selected, url],
              )
            }
          >
            {ja ? "このページを読む" : "Read this page"}:{" "}
            <span className="max-w-[24ch] truncate font-mono">{url}</span>
          </Button>
        );
      })}
      <Hint>
        {ja
          ? "依頼しても送信は承認されません。各ページの送信には Web 開示の承認が必要です。"
          : "Asking does not approve sending: each page still needs its own Web disclosure approval."}
      </Hint>
    </div>
  );
}
