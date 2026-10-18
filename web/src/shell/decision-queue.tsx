import { useState } from "react";
import { ChevronRight } from "lucide-react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "../components/ui/popover";
import type { HumanRequest, State } from "../types";
import type { ApprovalCard } from "../generated/models";
import { ApprovalCardView } from "../capabilities/approvals";
import { Badge, useI18n } from "../ui";
import { shellCopy } from "./copy";

export type PendingDecision = { request: HumanRequest; node: string };

/** Capability approvals addressed to the signed-in approver. */
export type ApprovalInbox = {
  pending?: ApprovalCard[];
  grants?: ApprovalCard[];
};

/**
 * Global queue of everything awaiting the signed-in person: human requests,
 * capability approvals they are asked to decide, and the Run grants they gave
 * and may revoke. Hidden while empty.
 */
export function DecisionQueue({
  data,
  decisions,
  inbox,
  open,
  onOpenChange,
  select,
}: {
  data?: State;
  decisions: PendingDecision[];
  inbox: ApprovalInbox;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  select: (decision: PendingDecision) => void;
}) {
  const { locale } = useI18n();
  const copy = shellCopy[locale];
  const ja = locale === "ja-JP";
  // Ages are measured when the queue opens, keeping render pure.
  const [openedAt, setOpenedAt] = useState(0);
  const approvals = inbox.pending ?? [];
  const grants = inbox.grants ?? [];
  const waiting = decisions.length + approvals.length;
  if (waiting === 0 && grants.length === 0) return null;
  return (
    <Popover
      open={open}
      onOpenChange={(value) => {
        if (value) setOpenedAt(Date.now());
        onOpenChange(value);
      }}
    >
      <PopoverTrigger className="inline-flex h-[30px] shrink-0 items-center gap-2 rounded-md border border-brand-line bg-brand-soft pl-2.5 pr-1 text-xs font-medium text-foreground transition-colors duration-150 hover:bg-accent aria-expanded:bg-accent">
        <span className="max-sm:sr-only">{copy.decisions}</span>
        <span className="grid h-[22px] min-w-[22px] place-items-center rounded-sm bg-primary px-1.5 font-mono text-xs font-semibold text-primary-foreground tabular">
          {waiting}
        </span>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="max-h-[85vh] w-[min(420px,calc(100vw-1.5rem))] overflow-y-auto p-1.5"
      >
        <div className="flex items-center justify-between gap-2 px-2 pb-2 pt-1.5">
          <span className="text-[13px] font-semibold">
            {copy.decisionQueue}
          </span>
          <span className="font-mono text-[11px] text-faint tabular">
            {copy.decisionCount(waiting)}
          </span>
        </div>
        <ul className="flex max-h-[min(60vh,420px)] flex-col gap-px overflow-y-auto">
          {decisions.map((decision) => {
            const minutes = Math.max(
              0,
              Math.floor(
                (openedAt - Date.parse(decision.request.created_at)) / 60_000,
              ),
            );
            return (
              <li key={`${decision.node}/${decision.request.id}`}>
                <button
                  type="button"
                  onClick={() => {
                    onOpenChange(false);
                    select(decision);
                  }}
                  className="group grid w-full grid-cols-[minmax(0,1fr)_auto] items-center gap-2 rounded-md px-2 py-2 text-left transition-colors duration-150 hover:bg-accent"
                >
                  <span className="grid min-w-0 gap-1">
                    <span className="flex min-w-0 items-center gap-2">
                      <Badge value={decision.request.kind} />
                      <span className="truncate font-mono text-[11px] text-faint">
                        {decision.node === data?.node.id
                          ? (data.workspaces.find(
                              (value) =>
                                value.id === decision.request.workspace_id,
                            )?.title ?? decision.node)
                          : decision.node}
                      </span>
                      <span className="ml-auto shrink-0 font-mono text-[11px] text-faint tabular">
                        {copy.ago(minutes)}
                      </span>
                    </span>
                    <span className="line-clamp-2 text-[13px] text-foreground">
                      {decision.request.prompt}
                    </span>
                  </span>
                  <ChevronRight
                    aria-hidden
                    className="size-3.5 text-faint group-hover:text-foreground"
                  />
                </button>
              </li>
            );
          })}
        </ul>
        {approvals.length > 0 && (
          <section
            aria-label={ja ? "あなたの承認待ち" : "Approvals awaiting you"}
            className="grid gap-1 border-t border-border px-2 pt-2 text-[13px]"
          >
            <h3 className="text-[11px] font-medium text-faint">
              {ja ? "あなたの承認待ち" : "Approvals awaiting you"}
            </h3>
            {approvals.map((item) => (
              <ApprovalCardView key={item.id} item={item} />
            ))}
          </section>
        )}
        {grants.length > 0 && (
          <section
            aria-label={ja ? "あなたが出した Run grant" : "Run grants you gave"}
            className="grid gap-1 border-t border-border px-2 pt-2 text-[13px]"
          >
            <h3 className="text-[11px] font-medium text-faint">
              {ja ? "あなたが出した Run grant" : "Run grants you gave"}
            </h3>
            {grants.map((item) => (
              <ApprovalCardView key={item.id} item={item} />
            ))}
          </section>
        )}
      </PopoverContent>
    </Popover>
  );
}
