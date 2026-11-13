/**
 * What a capability approval says about who decides it. Every Eligible
 * approver is offered a waiting approval, and the first decision counts
 * (ADR 0135), so a waiting card names nobody: naming the candidates would show
 * the requester part of the Tenant's policy. Once decided, it names who did.
 */
export type ApproverCard = {
  kind: string;
  state: string;
  decided_by?: string | null;
};

const ALLOWED = new Set(["approved", "attempted", "consumed", "active"]);

/**
 * Who decides, or decided, `card`; `null` when nobody did, as for an approval
 * that expired or closed while it waited.
 */
export function approverText(card: ApproverCard, ja: boolean): string | null {
  if (card.state === "blocked")
    return ja
      ? "承認できる人がいないため実行できません"
      : "Blocked: no eligible approver";
  if (card.state === "pending")
    return ja
      ? "承認できる人の判断を待っています"
      : "Waiting for an eligible approver";
  const by = card.decided_by;
  if (by && card.state === "denied")
    return ja ? `${by} が拒否しました` : `${by} denied it`;
  if (by && (ALLOWED.has(card.state) || card.kind === "grant"))
    return ja ? `${by} が許可しました` : `${by} allowed it`;
  return null;
}

/** The error envelope a capability route answers a refused decision with. */
export type DecisionError = {
  error?: {
    code?: unknown;
    details?: { decided_by?: unknown; outcome?: unknown } | null;
  } | null;
};

/**
 * What to tell a person whose decision came too late. Someone else may have
 * decided first, which the server names only to a person who may decide it
 * now; otherwise the approval expired or changed. `null` for any other error.
 */
export function decisionErrorText(
  envelope: DecisionError | undefined,
  ja: boolean,
): string | null {
  const error = envelope?.error;
  const by = error?.details?.decided_by;
  if (error?.code === "APPROVAL_ALREADY_DECIDED" && typeof by === "string")
    return error.details?.outcome === "denied"
      ? ja
        ? `${by} がすでに拒否しました。`
        : `${by} already denied this approval.`
      : ja
        ? `${by} がすでに許可しました。`
        : `${by} already allowed this approval.`;
  if (error?.code === "APPROVAL_STALE_OR_EXPIRED")
    return ja
      ? "この承認は期限切れか、表示した後に変わりました。表示を更新してください。"
      : "This approval has expired or changed since you opened it. Refresh to see it now.";
  return null;
}
