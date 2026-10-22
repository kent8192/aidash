/**
 * Tool call approvals as an approver reads them. The server names the tool on
 * a Tool call approval and on a Run grant made from one, so `tool`, not
 * `kind`, tells a card about a Tenant's tool from the other approvals.
 */

type ToolFields = {
  tool?: string | null;
  version?: string | null;
  operation?: string | null;
};

/** Whether a card is a Tool call approval or a Run grant made from one. */
export function isToolCard(card: Pick<ToolFields, "tool">): boolean {
  return typeof card.tool === "string" && card.tool !== "";
}

/** The tool and the definition version the call runs, as `tool@version`. */
export function toolLabel(card: ToolFields): string {
  return card.version ? `${card.tool}@${card.version}` : (card.tool ?? "");
}

/**
 * The call the approver is asked to allow: its name and its arguments, exactly
 * as recorded. Arguments are pretty-printed JSON unless the model sent a
 * string, which is shown as sent.
 */
export function toolCallView(call: unknown): {
  name: string | null;
  arguments: string | null;
} {
  if (!call || typeof call !== "object") return { name: null, arguments: null };
  const { name, arguments: args } = call as Record<string, unknown>;
  return {
    name: typeof name === "string" ? name : null,
    arguments:
      args === undefined
        ? null
        : typeof args === "string"
          ? args
          : JSON.stringify(args, null, 2),
  };
}

/**
 * What "allow for this Run" covers on a Tool call approval. The grant names one
 * operation of the tool, so it covers later calls with other arguments, and new
 * user input does not end it.
 */
export function toolGrantScope(card: ToolFields, ja: boolean): string {
  const tool = toolLabel(card);
  const operation = card.operation ?? "";
  return ja
    ? `この Run でこれ以降の ${tool} の ${operation} 呼び出しすべて（引数が違うものを含む）。期限まで。新しい入力では終了しません`
    : `Every later ${operation} call of ${tool} in this Run, whatever its arguments, until expiry. New user input does not end it`;
}

/**
 * The state line of a Tool call approval, or null for a card that is not one,
 * which keeps the generic line. A refused call is reported to the model and
 * the Run continues; an allowed one runs exactly as shown.
 */
export function toolStateText(
  card: {
    kind: string;
    state: string;
    expires_at?: string | null;
    decided_by?: string | null;
    decided_at?: string | null;
    closed_reason?: string | null;
  } & ToolFields,
  ja: boolean,
  when: (value: string | null | undefined) => string,
): string | null {
  if (card.kind !== "tool_call") return null;
  const refused = ja
    ? "呼び出しは拒否され、Run は続行します。"
    : "The call is refused and the Run continues.";
  const by = card.decided_by ?? "?";
  switch (card.state) {
    case "pending":
      return ja
        ? `判断待ち · 期限 ${when(card.expires_at)} · 許可すると、この呼び出しがそのまま実行されます`
        : `Awaiting a decision · expires ${when(card.expires_at)} · once allowed, this exact call runs`;
    case "approved":
    case "consumed":
      return card.decided_by
        ? ja
          ? `許可されました（${by}・${when(card.decided_at)}）。この呼び出しがそのまま実行されます。`
          : `Allowed by ${by} at ${when(card.decided_at)}. This exact call runs.`
        : ja
          ? "許可されています。この呼び出しがそのまま実行されます。"
          : "Allowed. This exact call runs.";
    case "denied":
      return ja
        ? `拒否されました（${by}・${when(card.decided_at)}）。${refused}`
        : `Denied by ${by} at ${when(card.decided_at)}. ${refused}`;
    case "expired":
      return ja ? `期限切れです。${refused}` : `Expired. ${refused}`;
    case "revoked":
      return ja ? `取り消されました。${refused}` : `Revoked. ${refused}`;
    case "blocked":
      // Nobody may approve: the fix is the Tenant's policy, not a person.
      return ja
        ? `この Tenant には ${toolLabel(card)} の呼び出しを承認できる人がいません。${refused}`
        : `Nobody in this Tenant may approve calls of ${toolLabel(card)}. ${refused}`;
    case "closed":
      return card.closed_reason === "run_ended"
        ? ja
          ? "閉じられました。この Run は終了しています。呼び出しは実行されません。"
          : "Closed because the Run ended. The call does not run."
        : ja
          ? "閉じられました。呼び出しは実行されません。"
          : "Closed. The call does not run.";
    default:
      return null;
  }
}
