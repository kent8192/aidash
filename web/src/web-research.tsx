import { Fragment, useRef, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "./transport";
import { post } from "./capabilities/client";
import { Field, useI18n } from "./ui";
import type { State, Run } from "./types";

type Disclosure = {
  approval_id: string;
  revision: number;
  request_digest: string;
  target: unknown;
  expires_at: string;
};
type Usage = {
  search_attempts: number;
  page_attempts: number;
  estimated_micro_usd: number;
  cache_bytes: number;
  observation_count: number;
  classification: string;
  context_digest: string;
  enabled: boolean;
  search_available: boolean;
  page_extractor_configured: boolean;
  node_month_estimate_micro_usd: number;
  monthly_limit_micro_usd: number;
  search_limit: number;
  page_limit: number;
  disclosures: Disclosure[];
  uncertain_operations: string[];
};
type Evidence = {
  evidence_ref: string;
  title: string;
  url: string;
  fetched_at: string;
  text: string;
  line_start: number;
  line_end: number;
  pdf_pages: number[];
  completeness: string;
};

function Citation({
  run,
  reference,
  number,
}: {
  run: string;
  reference: string;
  number: number;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [open, setOpen] = useState(false);
  const query = useQuery({
    queryKey: ["web-evidence", run, reference],
    queryFn: ({ signal }) =>
      apiFetch<Evidence>(`/api/runs/${run}/web/evidence/${reference}`, {
        signal,
      }),
    enabled: open,
    retry: false,
    staleTime: 0,
    refetchInterval: open ? 2000 : false,
  });
  const value = !query.isFetching && query.isSuccess ? query.data : undefined;
  let link: URL | undefined;
  try {
    if (value) {
      const url = new URL(value.url);
      if (["https:", "http:"].includes(url.protocol)) link = url;
    }
  } catch {
    /* Render invalid URLs as plain text. */
  }
  return (
    <span className="web-citation">
      <button
        type="button"
        aria-expanded={open}
        aria-label={`${ja ? "引用" : "Citation"} ${number}`}
        onClick={() => setOpen(!open)}
      >
        [{number}]
      </button>
      {open && (
        <span className="web-excerpt" role="note">
          {query.isPending && (ja ? "引用を確認中…" : "Checking citation…")}
          {query.isError && (
            <span role="alert">
              {ja
                ? "この引用は現在参照できません。"
                : "This citation is currently unavailable."}
            </span>
          )}
          {value && (
            <>
              {link ? (
                <a
                  href={link.toString()}
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  {value.title || link.hostname}
                </a>
              ) : (
                <strong>{value.title}</strong>
              )}
              <small>
                {link?.hostname} · {ja ? "取得" : "Fetched"}{" "}
                {new Date(value.fetched_at).toLocaleString(locale)} ·{" "}
                {value.completeness}
              </small>
              <small>
                {ja ? "行" : "Lines"} {value.line_start}–{value.line_end}
                {value.pdf_pages.length
                  ? ` · PDF ${value.pdf_pages.join(", ")}`
                  : ""}
              </small>
              <span className="web-excerpt-text">{value.text}</span>
            </>
          )}
        </span>
      )}
    </span>
  );
}
export function ResearchText({
  run,
  text,
  render,
}: {
  run: string;
  text: string;
  render: (text: string) => ReactNode;
}) {
  const pattern =
    /(\[\[web:ev_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\]\])/g;
  const references = new Map<string, number>();
  return (
    <>
      {text.split(pattern).map((part, index) => {
        if (!part.startsWith("[[web:ev_") || !part.endsWith("]]"))
          return <Fragment key={index}>{render(part)}</Fragment>;
        const reference = part.slice(6, -2);
        if (!references.has(reference))
          references.set(reference, references.size + 1);
        return (
          <Citation
            key={index}
            run={run}
            reference={reference}
            number={references.get(reference)!}
          />
        );
      })}
    </>
  );
}

export function WebResearch({
  workspace,
  thread,
  data,
}: {
  workspace: string;
  thread: string;
  data: State;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [agent, setAgent] = useState("");
  const [description, setDescription] = useState("");
  const [openUrl, setOpenUrl] = useState("");
  const [selected, setSelected] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const pending = useRef<{ digest: string; key: string } | undefined>(
    undefined,
  );
  const agents = data.registry.filter(
    (entry) =>
      entry.kind === "agent" &&
      ["web_search", "web_open", "web_find"].some(
        (flag) =>
          (
            entry.config.core_capabilities as
              | Record<string, unknown>
              | undefined
          )?.[flag] === true,
      ),
  );
  const runs = useQuery({
    queryKey: ["web-thread-runs", workspace, thread],
    queryFn: ({ signal }) =>
      apiFetch<Run[]>(
        `/api/workspaces/${workspace}/threads/${thread}/web/runs`,
        { signal },
      ),
    refetchInterval: 2000,
    retry: false,
  });
  const run = runs.data?.find((run) => run.id === selected) ?? runs.data?.[0];
  const usage = useQuery({
    queryKey: ["web-usage", run?.id],
    enabled: !!run,
    queryFn: ({ signal }) =>
      apiFetch<Usage>(`/api/runs/${run!.id}/web/usage`, { signal }),
    refetchInterval: 2000,
    retry: false,
  });
  const act = async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError("");
    try {
      await action();
      await runs.refetch();
      await usage.refetch();
      await client.invalidateQueries({ queryKey: ["state"] });
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  };
  if (!agents.length && !runs.data?.length) return null;
  const terminal =
    !run || ["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase);
  return (
    <section
      className="core-panel web-research"
      aria-label={ja ? "Web 調査" : "Web research"}
    >
      <h3>{ja ? "Web 調査" : "Web research"}</h3>
      <p>
        {ja
          ? "公開ページを調査し、実際に読んだ抜粋を引用します。非公開情報を含む場合は、送信する検索条件・URL ごとに承認が必要です。"
          : "Research public pages and cite excerpts actually read. Private or unclassified context requires approval for each exact query or URL."}
      </p>
      <Field label={ja ? "調査する Agent" : "Research Agent"}>
        <select value={agent} onChange={(e) => setAgent(e.target.value)}>
          <option value="">
            {ja ? "選択してください" : "Choose an Agent"}
          </option>
          {agents.map((entry) => (
            <option
              key={`${entry.id}@${entry.version}`}
              value={`${entry.id}@${entry.version}`}
            >
              {entry.name[ja ? "ja" : "en"] || entry.id} · {entry.version}
            </option>
          ))}
        </select>
      </Field>
      <Field label={ja ? "調べたいこと" : "Research request"}>
        <textarea
          value={description}
          maxLength={16384}
          onChange={(e) => setDescription(e.target.value)}
        />
      </Field>
      <Field
        label={
          ja
            ? "読み取りを依頼する公開 URL（任意）"
            : "Public URL to open (optional)"
        }
      >
        <input
          type="url"
          value={openUrl}
          maxLength={4096}
          onChange={(e) => setOpenUrl(e.target.value)}
        />
      </Field>
      {!!openUrl && (
        <small>
          {ja
            ? "この URL を一度読み取るよう依頼します。別の URL への移動には送信先の確認が必要です。"
            : "This requests one reading of this exact URL. A different redirect destination requires its own disclosure check."}
        </small>
      )}
      <button
        type="button"
        disabled={busy || !agent || !description.trim()}
        onClick={() =>
          void act(async () => {
            const split = agent.lastIndexOf("@");
            const input = {
              agent: {
                id: agent.slice(0, split),
                version: agent.slice(split + 1),
              },
              description,
              open_url: openUrl.trim() || null,
            };
            const digest = JSON.stringify(input);
            if (pending.current?.digest !== digest)
              pending.current = { digest, key: crypto.randomUUID() };
            const run = await post<Run>(
              `/workspaces/${workspace}/threads/${thread}/web/runs`,
              { ...input, idempotency_key: pending.current.key },
            );
            pending.current = undefined;
            setSelected(run.id);
            setDescription("");
            setOpenUrl("");
          })
        }
      >
        {ja ? "調査を開始" : "Start research"}
      </button>
      {!!runs.data?.length && (
        <Field label={ja ? "調査履歴" : "Research runs"}>
          <select
            value={run?.id ?? ""}
            onChange={(e) => setSelected(e.target.value)}
          >
            {runs.data.map((run) => (
              <option key={run.id} value={run.id}>
                {run.agent_id} · {run.phase} · {run.id.slice(0, 8)}
              </option>
            ))}
          </select>
        </Field>
      )}
      {run && (
        <p role="status">
          {run.phase} · {run.control}
        </p>
      )}
      {usage.data && (
        <>
          <p>
            {ja ? "検索" : "Search"} {usage.data.search_attempts}/
            {usage.data.search_limit} ·{" "}
            {ja ? "ページ取得" : "Page HTTP attempts"}{" "}
            {usage.data.page_attempts}/{usage.data.page_limit} ·{" "}
            {ja ? "この実行の見積もり" : "Run estimate"} $
            {(usage.data.estimated_micro_usd / 1e6).toFixed(4)}
          </p>
          <p>
            {ja
              ? "このノードの月額見積もり（請求額ではありません）"
              : "Node month estimate (not an invoice)"}{" "}
            ${(usage.data.node_month_estimate_micro_usd / 1e6).toFixed(4)} / $
            {(usage.data.monthly_limit_micro_usd / 1e6).toFixed(2)}
          </p>
          {!!usage.data.uncertain_operations.length && (
            <p role="alert">
              {ja
                ? "送信結果を確認できない要求があります。自動では再送しません。"
                : "Some dispatch outcomes are uncertain. These requests will not be replayed automatically."}
            </p>
          )}
          {!usage.data.enabled && (
            <p role="alert">
              {ja
                ? "管理者が Web 調査を無効にしています。"
                : "Web research is disabled by the operator."}
            </p>
          )}
          {!usage.data.search_available && (
            <p role="status">
              {ja
                ? "Brave の契約確認・有効な料金設定・キーの設定が必要です。取得済みの引用は閲覧できます。"
                : "Brave search needs current verified terms, pricing and credentials. Delivered citations remain readable."}
            </p>
          )}
          {!usage.data.page_extractor_configured && (
            <p role="status">
              {ja
                ? "ページ取得には隔離された抽出環境の設定が必要です。"
                : "Reading new pages needs a configured isolated extractor."}
            </p>
          )}
          <p>
            {ja ? "現在の文脈" : "Current context"}: {usage.data.classification}
          </p>
          {!terminal && (
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                void act(() =>
                  post(`/runs/${run!.id}/web/context`, {
                    expected_digest: usage.data!.context_digest,
                    public: usage.data!.classification !== "public",
                  }),
                )
              }
            >
              {usage.data.classification === "public"
                ? ja
                  ? "非公開情報を含む文脈として扱う"
                  : "Classify context as nonpublic"
                : ja
                  ? "現在の指示・会話・資料は全て公開情報だと確認する"
                  : "Confirm all current instructions, conversation and sources are public"}
            </button>
          )}
          {usage.data.disclosures.map((disclosure) => (
            <article key={disclosure.approval_id}>
              <h4>{ja ? "この送信の承認" : "Approve this disclosure"}</h4>
              <pre>{JSON.stringify(disclosure.target, null, 2)}</pre>
              <small>
                {ja ? "有効期限" : "Expires"}:{" "}
                {new Date(disclosure.expires_at).toLocaleString(locale)}
              </small>
              {[true, false].map((allow) => (
                <button
                  key={String(allow)}
                  type="button"
                  disabled={busy}
                  onClick={() =>
                    void act(() =>
                      post(
                        `/runs/${run!.id}/web/disclosures/${disclosure.approval_id}/decision`,
                        {
                          expected_revision: disclosure.revision,
                          request_digest: disclosure.request_digest,
                          allow_once: allow,
                        },
                      ),
                    )
                  }
                >
                  {allow
                    ? ja
                      ? "この要求だけ許可"
                      : "Allow this request once"
                    : ja
                      ? "拒否"
                      : "Deny"}
                </button>
              ))}
            </article>
          ))}
        </>
      )}
      {!terminal && (
        <button
          type="button"
          disabled={busy}
          onClick={() =>
            void act(() =>
              post(`/runs/${run!.id}/control`, { action: "cancel" }),
            )
          }
        >
          {ja ? "調査を停止" : "Stop research"}
        </button>
      )}
      {(error || usage.error || runs.error) && (
        <p role="alert">
          {error || usage.error?.message || runs.error?.message}
        </p>
      )}
    </section>
  );
}
