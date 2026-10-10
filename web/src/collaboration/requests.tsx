import { Button } from "../components/ui/button";
import { useRef, useState } from "react";
import { Alert, Facts } from "../components/patterns";
import { useQueryClient } from "@tanstack/react-query";
import { Check, ChevronRight, X } from "lucide-react";
import { humanAnswer, remoteAction } from "../generated/aidash";
import type { HumanRequest } from "../types";
import type { Selection } from "./details";
import { useI18n } from "../ui";
import { cn } from "../lib/utils";
import { collaborationCopy } from "./copy";
import { workspaceCopy } from "./workspace-copy";

type PendingRequest = { request: HumanRequest; node: string };

/** Decision cards for pending human requests; approvals are answered in place. */
export function ChannelRequests({
  requests,
  localNode,
  open,
  requester,
  className,
}: {
  requests: PendingRequest[];
  localNode: string;
  open: (value: Selection) => void;
  /** Display name of the agent whose run asked, when known. */
  requester?: string;
  className?: string;
}) {
  const { locale, t } = useI18n();
  const words = workspaceCopy[locale];
  const copy = collaborationCopy[locale];
  const [busy, setBusy] = useState<string | null>(null);
  const inFlight = useRef(false);
  const [error, setError] = useState("");
  const client = useQueryClient();
  async function answer(item: PendingRequest, approved: boolean) {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(`${item.node}:${item.request.id}`);
    setError("");
    try {
      const response = { approved };
      if (item.node === localNode) await humanAnswer(item.request.id, response);
      else
        await remoteAction({
          node_id: item.node,
          control: {
            run_id: item.request.run_id,
            request_id: item.request.id,
            action: "answer",
            response,
          },
        });
      await Promise.all([
        client.invalidateQueries({ queryKey: ["state"] }),
        client.invalidateQueries({ queryKey: ["mesh"] }),
        client.invalidateQueries({ queryKey: ["workspace"] }),
      ]);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      inFlight.current = false;
      setBusy(null);
    }
  }
  if (!requests.length) return null;
  const since = (request: HumanRequest) =>
    new Date(request.created_at).toLocaleTimeString(locale, {
      hour: "2-digit",
      minute: "2-digit",
    });
  return (
    <section
      className={cn("grid gap-2", className)}
      aria-label={copy.needsInput}
    >
      {requests.map((item) => {
        const meta = (
          <Facts
            className="mt-2.5 border-t border-border pt-2.5"
            items={[
              ...(requester
                ? [[locale === "ja-JP" ? "依頼元" : "From", requester] as const]
                : []),
              [
                locale === "ja-JP" ? "待機" : "Waiting",
                locale === "ja-JP"
                  ? `${since(item.request)} から`
                  : `since ${since(item.request)}`,
                true,
              ] as const,
            ]}
          />
        );
        const kicker = (
          <span className="flex items-center gap-1.5 text-[11px] font-medium text-warning">
            <span aria-hidden className="size-1.5 rounded-full bg-warning" />
            {item.request.kind === "APPROVAL_REQUIRED"
              ? words.approval
              : t(item.request.kind)}
          </span>
        );
        const frame =
          "relative rounded-lg border border-border-strong bg-popover p-3.5 pl-4 text-left shadow-overlay before:absolute before:inset-y-3.5 before:-left-px before:w-0.5 before:rounded-r-sm before:bg-warning";
        return item.request.kind === "APPROVAL_REQUIRED" ? (
          <article className={frame} key={`${item.node}:${item.request.id}`}>
            {kicker}
            <h3 className="mt-1.5 text-sm font-semibold leading-snug text-foreground">
              {item.request.prompt}
            </h3>
            {meta}
            <div className="mt-3 flex flex-wrap items-center gap-1.5">
              <Button
                type="button"
                size="sm"
                disabled={busy !== null}
                onClick={() => void answer(item, true)}
              >
                <Check />
                {words.approve}
              </Button>
              <Button
                variant="outline"
                size="sm"
                type="button"
                disabled={busy !== null}
                onClick={() => void answer(item, false)}
              >
                <X />
                {words.reject}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                type="button"
                disabled={busy !== null}
                onClick={() => open({ kind: "human", ...item })}
              >
                {words.answer}
              </Button>
            </div>
          </article>
        ) : (
          <button
            key={`${item.node}:${item.request.id}`}
            type="button"
            className={cn(
              frame,
              "request-card block w-full transition-colors hover:border-warning",
            )}
            onClick={() => open({ kind: "human", ...item })}
          >
            {kicker}
            <span className="mt-1.5 block text-sm font-semibold leading-snug text-foreground">
              {item.request.prompt}
            </span>
            {meta}
            <span className="mt-2.5 inline-flex items-center gap-1 text-xs font-medium text-brand">
              {copy.answer}
              <ChevronRight aria-hidden className="size-3.5" />
            </span>
          </button>
        );
      })}
      {error && (
        <Alert>{error}</Alert>
      )}
    </section>
  );
}
