import { Input } from "../components/ui/input";
import { panelClass } from "./display";
import {
  Alert,
  Check,
  Disclosure,
  Hint,
  Pre,
  inlineFormClass,
} from "../components/patterns";
import { Button } from "../components/ui/button";
import { useState } from "react";
import { useInfiniteQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "../transport";
import { Field, useI18n } from "../ui";
import { post } from "./client";
import type { ApprovalCard as Approval } from "../generated/models";
import { requestPrice, runGrantScope } from "../research";
import { useViewerSubject } from "../approval-queries";
import {
  approvalCopy,
  givenByText,
  refusedByText,
} from "../approval-settings-copy";
import {
  decisionScope,
  givenBy,
  refusedBy,
  type DecisionChoice,
} from "../approval-settings-model";
import {
  isToolCard,
  toolCallView,
  toolGrantScope,
  toolLabel,
  toolStateText,
} from "./tool-approval-model";

const DISCLOSE = "research.disclose";

/** Whether a card is a Web disclosure approval or a Run grant made from one. */
function disclosure(item: Approval) {
  return item.kind === "disclosure" || item.action === DISCLOSE;
}

function title(item: Approval, ja: boolean) {
  if (item.kind === "tool_call")
    return ja ? "ツール呼び出しの承認" : "Tool call approval";
  if (item.kind === "grant" && isToolCard(item))
    return ja ? "ツールの Run grant" : "Tool Run grant";
  if (item.kind === "grant")
    return disclosure(item)
      ? ja
        ? "Web 開示の Run grant"
        : "Web disclosure Run grant"
      : ja
        ? "接続先の Run grant"
        : "Outbound Run grant";
  if (item.kind === "disclosure")
    return item.operation === "search"
      ? ja
        ? "Web 検索の開示承認"
        : "Web search disclosure"
      : ja
        ? "ページ読み取りの開示承認"
        : "Page reading disclosure";
  return ja ? "外部接続の承認" : "Outbound request";
}

function FactList({ rows }: { rows: readonly (readonly [string, string])[] }) {
  return (
    <dl className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-0.5">
      {rows.map(([label, text]) => (
        <div key={label} className="contents">
          <dt className="text-muted-foreground">{label}</dt>
          <dd className="[overflow-wrap:anywhere]">{text}</dd>
        </div>
      ))}
    </dl>
  );
}

const QUERY_FIELDS: [string, string, string][] = [
  ["query", "Query", "検索語"],
  ["language", "Language", "言語"],
  ["country", "Country", "国"],
  ["count", "Results", "件数"],
  ["page", "Page", "ページ"],
  ["freshness", "Freshness", "期間"],
  ["include_domains", "Only domains", "対象ドメイン"],
  ["exclude_domains", "Excluded domains", "除外ドメイン"],
];

/**
 * The search query as the host validated it, field by field. It is read from
 * the provider-independent form recorded with the approval, never from a
 * provider's request body.
 */
function QuerySummary({ query, ja }: { query: unknown; ja: boolean }) {
  if (!query || typeof query !== "object") return null;
  const fields = query as Record<string, unknown>;
  const rows = QUERY_FIELDS.flatMap(([key, en, jp]) => {
    const value = fields[key];
    if (value == null || (Array.isArray(value) && value.length === 0))
      return [];
    const text = Array.isArray(value)
      ? value.join(", ")
      : typeof value === "object"
        ? Object.values(value as Record<string, unknown>).join(" – ")
        : String(value);
    return [[ja ? jp : en, text] as const];
  });
  return <FactList rows={rows} />;
}

/** What an approver needs to decide one Web disclosure without the Run. */
function DisclosureFacts({ item, ja }: { item: Approval; ja: boolean }) {
  const none = ja ? "なし" : "None";
  const facts: [string, string][] = [
    [ja ? "送信先" : "Destination", item.targets.join(", ")],
    [ja ? "Provider" : "Provider", item.provider ?? none],
    // The Research account holder pays; its account ID is kept for audit only.
    [
      ja ? "課金先" : "Charged to",
      item.estimated_micro_usd == null
        ? none
        : ja
          ? `この Deployment の ${item.provider ?? ""} アカウント`
          : `This Deployment's ${item.provider ?? ""} account`,
    ],
    [
      ja ? "推定費用（1回）" : "Estimated cost (this request)",
      item.estimated_micro_usd == null
        ? none
        : requestPrice(item.estimated_micro_usd, ja),
    ],
    [
      "Agent",
      item.agent_id ? `${item.agent_id}@${item.agent_version ?? ""}` : none,
    ],
  ];
  return (
    <>
      <FactList rows={facts} />
      {item.kind === "disclosure" && (
        <>
          <QuerySummary query={item.query} ja={ja} />
          {item.disclosure && (
            <Disclosure
              summary={ja ? "送信される内容（正確な値）" : "Exact content sent"}
            >
              <Pre>{item.disclosure}</Pre>
            </Disclosure>
          )}
        </>
      )}
    </>
  );
}

/** What an approver needs to decide one call of a Tenant's tool without the Run. */
function ToolFacts({ item, ja }: { item: Approval; ja: boolean }) {
  const none = ja ? "なし" : "None";
  const call = item.kind === "tool_call" ? toolCallView(item.call) : null;
  const facts: [string, string][] = [
    [ja ? "ツール" : "Tool", toolLabel(item)],
    [ja ? "操作" : "Operation", item.operation ?? none],
    [
      "Agent",
      item.agent_id ? `${item.agent_id}@${item.agent_version ?? ""}` : none,
    ],
  ];
  if (call) facts.push([ja ? "呼び出し" : "Call", call.name ?? none]);
  return (
    <>
      <FactList rows={facts} />
      {call && (
        <>
          <Hint id={`${item.id}-arguments`}>
            {ja
              ? "引数（許可すると、この内容のまま実行されます）"
              : "Arguments (the call runs exactly as shown once allowed)"}
          </Hint>
          <Pre
            role="region"
            aria-labelledby={`${item.id}-arguments`}
            // A bounded block scrolls, so a keyboard user must be able to focus it.
            tabIndex={0}
          >
            {call.arguments ?? none}
          </Pre>
        </>
      )}
    </>
  );
}

function stateText(item: Approval, ja: boolean, locale: string) {
  const when = (value: string | null | undefined) =>
    value ? new Date(value).toLocaleString(locale) : "";
  const host = item.targets.join(", ");
  const tool = toolStateText(item, ja, when);
  if (tool) return tool;
  switch (item.state) {
    case "closed":
      return item.closed_reason === "evidence_revoked"
        ? ja
          ? "閉じられました。この Run の Research evidence が取り消されたためです。送信されません。"
          : "Closed because this Run's research evidence was revoked. Nothing is sent."
        : ja
          ? "閉じられました。送信されません。"
          : "Closed. Nothing is sent.";
    case "blocked":
      // Nobody may approve: the fix is the Tenant's policy, not a person.
      return disclosure(item)
        ? ja
          ? `この Tenant には ${host} への Web 開示を承認できる人がいません。送信されません。`
          : `Nobody in this Tenant may approve Web disclosures to ${host}. Nothing is sent.`
        : ja
          ? `この Tenant には ${host} への接続を承認できる人がいません。送信されません。`
          : `Nobody in this Tenant may approve requests to ${host}. Nothing is sent.`;
    case "denied":
      return ja
        ? `拒否されました（${item.decided_by ?? "?"}・${when(item.decided_at)}）。送信されません。`
        : `Denied by ${item.decided_by ?? "?"} at ${when(item.decided_at)}. Nothing is sent.`;
    case "expired":
      return ja
        ? "期限切れです。送信されません。"
        : "Expired. Nothing is sent.";
    case "pending":
      return ja
        ? `判断待ち · 期限 ${when(item.expires_at)}`
        : `Awaiting a decision · expires ${when(item.expires_at)}`;
    default:
      return `${item.state}${item.expires_at ? ` · ${when(item.expires_at)}` : ""}`;
  }
}

/** One capability approval or Run grant, with the decisions its reader may take. */
export function ApprovalCardView({ item }: { item: Approval }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const words = approvalCopy[locale];
  const viewer = useViewerSubject();
  const advance = givenBy(item.given_by);
  const refusal = refusedBy(item.refused_by);
  const client = useQueryClient();
  const [runGrant, setRunGrant] = useState(false);
  const [minutes, setMinutes] = useState(15);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const research = disclosure(item);
  const tool = isToolCard(item);
  // What an "allow for this Run" decision covers, when the card can grant one.
  const scope = research
    ? runGrantScope(item, ja)
    : tool
      ? toolGrantScope(item, ja)
      : null;
  const action = async (choice: string) => {
    setBusy(true);
    setError("");
    try {
      if (choice === "revoke")
        await post(
          `/capabilities/${item.kind === "grant" ? "grants" : "approvals"}/${item.id}/revoke`,
          {
            idempotency_key: crypto.randomUUID(),
            expected_revision: item.revision,
          },
        );
      else
        await post(`/capabilities/approvals/${item.id}/decide`, {
          idempotency_key: crypto.randomUUID(),
          expected_revision: item.revision,
          choice,
          ...decisionScope(choice as DecisionChoice, {
            targets: item.targets,
            minutes,
            now: Date.now(),
          }),
        });
      await client.invalidateQueries({ queryKey: ["core-approvals"] });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <article aria-label={title(item, ja)}>
      <strong>{title(item, ja)}</strong>
      {research ? (
        <DisclosureFacts item={item} ja={ja} />
      ) : tool ? (
        <ToolFacts item={item} ja={ja} />
      ) : (
        <p>{item.targets.join(", ")}</p>
      )}
      {item.kind === "grant" && scope && <Hint>{scope}</Hint>}
      <Hint>
        {ja ? "要求者" : "Requester"}: {item.requester} ·{" "}
        {ja ? "承認者" : "Approver"}:{" "}
        {item.approver ??
          (ja
            ? "承認できる人がいないため実行できません"
            : "Blocked: no eligible approver")}
      </Hint>
      <Hint role="status">{stateText(item, ja, locale)}</Hint>
      {advance && (
        <Hint>
          {givenByText(
            words,
            advance,
            item.viewer_is_approver,
            item.decided_by ?? "?",
          )}
        </Hint>
      )}
      {refusal && (
        <Hint>
          {refusedByText(
            words,
            refusal,
            // Nobody approves a refused card; the person whose rule or mode
            // refused it is the one who decided it.
            viewer !== null && item.decided_by === viewer,
            item.decided_by ?? "?",
          )}
        </Hint>
      )}
      {item.state === "pending" && item.viewer_is_approver && (
        <>
          <Check
            checked={runGrant}
            onChange={(e) => setRunGrant(e.target.checked)}
          >
            {scope
              ? `${ja ? "この Run で許可" : "Allow for this Run"}: ${scope}`
              : ja
                ? "この Run で同じ接続先を期限付きで許可"
                : "Grant the same targets for this Run until expiry"}
          </Check>
          {runGrant && (
            <Field
              label={
                ja ? "有効期間（分、最大60）" : "Duration (minutes, up to 60)"
              }
            >
              <Input
                type="number"
                min={1}
                max={60}
                value={minutes}
                onChange={(e) =>
                  setMinutes(Math.max(1, Math.min(60, Number(e.target.value))))
                }
              />
            </Field>
          )}
          <div className={inlineFormClass}>
            <Button
              variant="outline"
              type="button"
              disabled={busy}
              onClick={() => void action(runGrant ? "allow_run" : "allow_once")}
            >
              {runGrant
                ? ja
                  ? "期限付きで許可"
                  : "Allow for this Run"
                : ja
                  ? "今回だけ許可"
                  : "Allow once"}
            </Button>
            {item.allow_always && (
              <Button
                variant="outline"
                type="button"
                disabled={busy}
                onClick={() => void action("allow_always")}
              >
                {words.alwaysAllow}
              </Button>
            )}
            <Button
              variant="outline"
              type="button"
              disabled={busy}
              onClick={() => void action("deny")}
            >
              {ja ? "拒否" : "Deny"}
            </Button>
          </div>
          {item.allow_always && <Hint>{words.alwaysAllowScope}</Hint>}
        </>
      )}
      {/* A disclosure approval is decided, not revoked; its Run grant is. */}
      {["active", "approved", "pending", "attempted"].includes(item.state) &&
        item.kind !== "disclosure" && (
          <Button
            variant="outline"
            type="button"
            disabled={busy}
            onClick={() => void action("revoke")}
          >
            {ja ? "許可を取り消す" : "Revoke"}
          </Button>
        )}
      {error && <Alert>{error}</Alert>}
    </article>
  );
}

export function CapabilityApprovals({ area }: { area?: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const query = useInfiniteQuery({
    queryKey: ["core-approvals"],
    initialPageParam: "",
    queryFn: ({ pageParam, signal }) =>
      apiFetch<{ items: Approval[]; next_cursor: string | null }>(
        `/api/capabilities/approvals${pageParam ? `?cursor=${pageParam}` : ""}`,
        { signal },
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    refetchInterval: 2000,
    retry: false,
  });
  const items =
    query.data?.pages
      .flatMap((p) => p.items)
      .filter((i) => !area || i.area_id === area) ?? [];
  return (
    <details
      className={panelClass}
      open={items.some((i) => i.state === "pending")}
    >
      <summary>
        {ja ? "承認と接続許可" : "Approvals and grants"} ({items.length})
      </summary>
      {query.isError && <Alert>{query.error.message}</Alert>}
      {items.map((item) => (
        <ApprovalCardView key={item.id} item={item} />
      ))}
      {query.hasNextPage && (
        <Button
          variant="outline"
          type="button"
          onClick={() => void query.fetchNextPage()}
        >
          {ja ? "さらに表示" : "Load more"}
        </Button>
      )}
    </details>
  );
}
