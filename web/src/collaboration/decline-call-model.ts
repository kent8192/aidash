/** The kinds of refusal a person may decline (#763), as the server names them. */
export type DeclineKind =
  | "policy"
  | "catalog_approval"
  | "agent_configuration"
  | "other_node";

/** The call a paused Run stopped on, offered only to someone who may decline it. */
export type DeclinableCall = {
  call: string;
  tool: string;
  kind: DeclineKind;
};

const KIND_KEYS: Record<DeclineKind, string> = {
  policy: "declineKindPolicy",
  catalog_approval: "declineKindCatalog",
  agent_configuration: "declineKindAgent",
  other_node: "declineKindOtherNode",
};

/** The locale key naming who refused. */
export function declineKindKey(kind: DeclineKind): string {
  return KIND_KEYS[kind];
}

/**
 * The call to offer for declining. The server sends it only to someone who may decline, and only
 * while the Run is paused on it; this also hides it for a Run that is no longer paused, so a stale
 * response never shows a button that would answer `409`.
 */
export function offeredDecline(
  response: unknown,
  control: string,
): DeclinableCall | undefined {
  if (control !== "PAUSED" || typeof response !== "object" || !response)
    return undefined;
  const call = (response as { declinable_call?: Partial<DeclinableCall> })
    .declinable_call;
  if (!call?.call || !call.tool || !call.kind || !(call.kind in KIND_KEYS))
    return undefined;
  return call as DeclinableCall;
}

/**
 * Which step of a decline comes next. The sentence is an optional Run message. Where the Run
 * takes messages while it is paused it goes first; on another Node the receiver refuses Home
 * messages while the Run is paused, so the call is declined first and the sentence follows.
 * Each step is done once: a retry after a failure repeats only what has not succeeded, never a
 * second decline (which would answer `409`) or a second message.
 */
export type DeclineSteps = {
  sentence: string;
  sent: boolean;
  declined: boolean;
  order: "sentence-first" | "decline-first";
};

export function nextDeclineStep(
  steps: DeclineSteps,
): "message" | "decline" | "done" {
  const message = !!steps.sentence.trim() && !steps.sent;
  const decline = !steps.declined;
  if (steps.order === "decline-first")
    return decline ? "decline" : message ? "message" : "done";
  return message ? "message" : decline ? "decline" : "done";
}
