import { toolLabel } from "./capabilities/tool-approval-model.ts";

/**
 * The one line a notification shows for a capability approval: what is
 * asked, where it would go, which Agent asks and for whom. It never carries
 * a tool call's arguments, which only the card shows.
 */
export function approvalNotice(
  card: {
    kind: string;
    operation?: string | null;
    targets: string[];
    tool?: string | null;
    version?: string | null;
    agent_id?: string | null;
    agent_version?: string | null;
    requester: string;
    sampling_target?: { plugin: string; server: string } | null;
  },
  ja: boolean,
): string {
  const what =
    card.kind === "sampling"
      ? ja
        ? "Sampling の承認"
        : "Sampling approval"
      : card.kind === "tool_call"
        ? ja
          ? "ツール呼び出し"
          : "Tool call"
        : ja
          ? "外部接続"
          : "Outbound request";
  const agent = card.agent_id
    ? `${card.agent_id}@${card.agent_version ?? ""}`
    : null;
  return [
    what,
    // A tool call's target is an internal address, so the tool stands for it.
    card.kind === "sampling"
      ? card.sampling_target &&
        `${card.sampling_target.plugin}/${card.sampling_target.server}`
      : card.kind === "tool_call"
        ? toolLabel(card)
        : card.targets.join(", "),
    agent,
    ja ? `依頼者 ${card.requester}` : `requested by ${card.requester}`,
  ]
    .filter(Boolean)
    .join(" · ");
}
