import { useQuery } from "@tanstack/react-query";
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

/**
 * The research operations an Agent binds and whether this Node can run them.
 * Page reading is possible only when a bound Page reading provider is
 * available here; only then is a URL intent offered.
 */
export function useResearch(agent: Agent | undefined, data: State) {
  const bound = researchBindings(agent, data.registry);
  const providers = useQuery({
    queryKey: ["research-providers"],
    queryFn: ({ signal }) =>
      apiFetch<ResearchProviders>("/api/research/providers", { signal }),
    enabled: bound.length > 0,
    staleTime: 60_000,
    retry: false,
  });
  const available = (binding: ResearchBinding) =>
    providers.data?.providers.find(
      (provider) => provider.provider === binding.provider,
    )?.available === true;
  return {
    bound,
    providers: providers.data,
    available,
    canRead: bound.some(
      (binding) => binding.operation === "fetch" && available(binding),
    ),
  };
}

/**
 * The Web research of the selected Agent in a Thread: its bound research
 * operations, their availability on this Node, the per-Run limits and, for a
 * Run, what it has used and which disclosures it is waiting on.
 */
export function ResearchPanel({
  research,
  run,
}: {
  research: ReturnType<typeof useResearch>;
  run?: string | null;
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
  return (
    <section
      aria-label={ja ? "Web 調査" : "Web research"}
      className="grid min-w-0 gap-1.5"
    >
      <h4 className="text-[13px] font-semibold">
        {ja ? "Web 調査" : "Web research"}
      </h4>
      <ul className="grid gap-0.5">
        {research.bound.map((binding) => (
          <li key={`${binding.provider} ${binding.operation}`}>
            {(OPERATION[binding.operation] ?? [binding.operation])[ja ? 1 : 0]}{" "}
            · <code>{binding.provider}</code> ·{" "}
            {research.providers === undefined
              ? "…"
              : research.available(binding)
                ? ja
                  ? "この Node で利用できます"
                  : "Available on this Node"
                : ja
                  ? "この Node では Operator の設定が必要です"
                  : "Needs Operator setup on this Node"}
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
          {of(used?.snapshots, limits.snapshots)}
          {usage.data &&
            ` · ${ja ? "推定調査費用" : "estimated research cost"} ${dollars(usage.data.estimated_micro_usd)} ${ja ? "（見積もり。請求額ではありません）" : "(estimate, not an invoice)"}`}
        </Hint>
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
