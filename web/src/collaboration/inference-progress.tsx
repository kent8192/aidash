import { useEffect, useReducer, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { subscribeInference } from "../event-stream";
import {
  initialInferenceState,
  reduceInference,
  type InferenceAttempt,
  type InferenceState,
} from "../inference-progress";
import { useI18n } from "../ui";
import { collaborationCopy } from "./copy";

type Copy = (typeof collaborationCopy)[keyof typeof collaborationCopy];

function reasonLabel(copy: Copy, reason: string | undefined) {
  return (
    copy.inferenceReasons[reason as keyof Copy["inferenceReasons"]] ??
    reason ??
    copy.inferenceInterrupted
  );
}

function announcement(copy: Copy, phase: InferenceState["phase"]) {
  switch (phase?.phase) {
    case "started":
      return copy.inferenceStarted;
    case "accepted":
      return copy.inferenceAccepted;
    case "discarded":
      return copy.inferenceReasons.correction;
    case "interrupted":
      return `${copy.inferenceInterrupted}: ${reasonLabel(copy, phase.reason)}`;
    default:
      return "";
  }
}

function Attempt({ attempt, copy }: { attempt: InferenceAttempt; copy: Copy }) {
  const gap = attempt.gap && (
    <p className="inference-gap">{copy.inferenceGap}</p>
  );
  // An accepted response is shown by the Run's own data.
  if (attempt.outcome === "accepted") return gap || null;
  if (attempt.outcome === "pending")
    return (
      <>
        {gap}
        <div className="inference-attempt pending">
          <p className="inference-label">
            <span className="inference-phase">{copy.inferencePending}</span>
            {copy.inferenceTentative}
          </p>
          {attempt.text && <p className="inference-text">{attempt.text}</p>}
          {attempt.truncated && (
            <p className="inference-note">{copy.inferenceTruncated}</p>
          )}
          {attempt.toolCalls.length > 0 && (
            <ul className="inference-tools">
              {attempt.toolCalls.map((call) => (
                <li className="inference-tool" key={call.index}>
                  <strong>
                    {call.name ??
                      call.id ??
                      copy.inferenceUnnamedTool.replace(
                        "{index}",
                        String(call.index),
                      )}
                  </strong>
                  {copy.inferenceToolCall.replace(
                    "{bytes}",
                    String(call.argumentBytes),
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      </>
    );
  return (
    <>
      {gap}
      <details className="inference-attempt ended">
        <summary>
          <span className="inference-phase">
            {attempt.outcome === "discarded"
              ? copy.inferenceReasons.correction
              : copy.inferenceInterrupted}
          </span>
          {attempt.outcome === "interrupted" && (
            <span className="inference-reason">
              {reasonLabel(copy, attempt.reason)}
            </span>
          )}
        </summary>
        {attempt.text && <s className="inference-text">{attempt.text}</s>}
        {attempt.truncated && (
          <p className="inference-note">{copy.inferenceTruncated}</p>
        )}
      </details>
    </>
  );
}

/**
 * Tentative, display-only progress of a local Run's Inference Attempts. It is
 * never final and never feeds forms or artifacts.
 */
export function InferenceProgress({
  run,
  node,
  active,
}: {
  run: string;
  node: string;
  /** Subscribe while the Run is local and not terminal. */
  active: boolean;
}) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  const client = useQueryClient();
  const [state, dispatch] = useReducer(reduceInference, initialInferenceState);
  const cursor = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (!active) return;
    const controller = new AbortController();
    void subscribeInference(controller.signal, run, cursor.current, (frame) => {
      if (frame.id) cursor.current = frame.id;
      dispatch(frame);
    });
    return () => controller.abort();
  }, [active, run]);
  useEffect(() => {
    if (state.accepted !== null)
      void client.invalidateQueries({ queryKey: ["run", node, run] });
  }, [client, node, run, state.accepted]);
  return (
    <>
      <p className="sr-only" role="status">
        {announcement(copy, state.phase)}
      </p>
      {state.attempts.length > 0 && (
        <section
          className="inference-progress"
          aria-label={copy.inferenceProgress}
        >
          {state.attempts.map((attempt) => (
            <Attempt attempt={attempt} copy={copy} key={attempt.id} />
          ))}
        </section>
      )}
    </>
  );
}
