import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Check, ShieldCheck, X } from "lucide-react";
import { humanAnswer, remoteAction } from "../generated/aidash";
import type { HumanRequest } from "../types";
import type { Selection } from "./details";
import { useI18n } from "../ui";
import { collaborationCopy } from "./copy";
import { workspaceCopy } from "./workspace-copy";

export function ChannelRequests({
  requests,
  localNode,
  open,
}: {
  requests: { request: HumanRequest; node: string }[];
  localNode: string;
  open: (value: Selection) => void;
}) {
  const { locale } = useI18n();
  const words = workspaceCopy[locale];
  const copy = collaborationCopy[locale];
  const [busy, setBusy] = useState<string | null>(null);
  const inFlight = useRef(false);
  const [error, setError] = useState("");
  const client = useQueryClient();
  async function answer(item: (typeof requests)[number], approved: boolean) {
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
  return (
    <section className="collab-attention" aria-label={copy.needsInput}>
      {requests.map((item) =>
        item.request.kind === "APPROVAL_REQUIRED" ? (
          <article
            className="workspace-approval"
            key={`${item.node}:${item.request.id}`}
          >
            <ShieldCheck size={20} />
            <div>
              <div className="workspace-approval-title">
                <strong>{item.request.prompt}</strong>
                <small>{words.approval}</small>
              </div>
              <div className="workspace-approval-actions">
                <button
                  type="button"
                  disabled={busy !== null}
                  className="approval-confirm"
                  onClick={() => void answer(item, true)}
                >
                  <Check size={13} />
                  {words.approve}
                </button>
                <button
                  type="button"
                  disabled={busy !== null}
                  onClick={() => open({ kind: "human", ...item })}
                >
                  {words.answer}
                </button>
                <button
                  type="button"
                  disabled={busy !== null}
                  onClick={() => void answer(item, false)}
                >
                  <X size={12} />
                  {words.reject}
                </button>
              </div>
            </div>
          </article>
        ) : (
          <button
            key={`${item.node}:${item.request.id}`}
            type="button"
            className="request-card"
            onClick={() => open({ kind: "human", ...item })}
          >
            <ShieldCheck size={18} />
            <span>
              <strong>{item.request.prompt}</strong>
              <small>{copy.answer}</small>
            </span>
          </button>
        ),
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
