import type {
  RegistrationInput,
  RegistrationRecord,
  ToolListsInput,
} from "./generated/models";
import { deploymentId, parseNodeAddress } from "./node-selection.ts";

/** What a Tenant Administrator types to register a Node. */
export type RegistrationDraft = {
  source_node: string;
  endpoint: string;
  dashboard_origin: string;
  key_id: string;
  atomic_transactions: boolean;
};

/**
 * Whether `address` names a Node of another Deployment than `own`
 * (`aidash://<name>`). Only such a Node has an endpoint and a Deployment Key to
 * pin; a Node of this Deployment is reached in-process.
 */
export function inOtherDeployment(address: string, own: string): boolean {
  const parsed = parseNodeAddress(address);
  return parsed !== null && deploymentId(address) !== own;
}

/** A first registration of `draft.source_node` at one of this Tenant's Nodes. */
export function newRegistration(
  draft: RegistrationDraft,
  own: string,
): RegistrationInput {
  const source_node = draft.source_node.trim();
  return {
    source_node,
    atomic_transactions: draft.atomic_transactions,
    enabled: true,
    expected_revision: 0,
    ...(inOtherDeployment(source_node, own)
      ? {
          endpoint: draft.endpoint.trim(),
          dashboard_origin: draft.dashboard_origin.trim() || null,
          key_id: draft.key_id.trim(),
        }
      : {}),
  };
}

/**
 * A new revision of `record` with one setting changed. Enabling, or changing a
 * setting of an enabled registration, asks the other Deployment for its key
 * again, so the endpoint and key ID it pinned go back with the request.
 * Disabling needs no answer from it.
 */
export function changedRegistration(
  record: RegistrationRecord,
  change: { enabled?: boolean; atomic_transactions?: boolean },
): RegistrationInput {
  const enabled = change.enabled ?? record.enabled;
  return {
    source_node: record.source_node,
    atomic_transactions:
      change.atomic_transactions ?? record.atomic_transactions,
    enabled,
    expected_revision: record.revision,
    ...(enabled && record.endpoint && record.key_id
      ? {
          endpoint: record.endpoint,
          dashboard_origin: record.dashboard_origin,
          key_id: record.key_id,
        }
      : {}),
  };
}

/** The four Federation tool lists of a registration, one pattern per line. */
export type ToolListsDraft = {
  request_tools: string;
  request_hosts: string;
  allowance: string;
  block: string;
};

/** The patterns of one list, one per line. */
export function patternLines(patterns: readonly string[]): string {
  return patterns.join("\n");
}

/** The patterns typed one per line; blank lines and spaces around a pattern are dropped. */
export function patternsOf(lines: string): string[] {
  return lines
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

export function toolListsDraft(record: RegistrationRecord): ToolListsDraft {
  return {
    request_tools: patternLines(record.tool_request.tools),
    request_hosts: patternLines(record.tool_request.hosts),
    allowance: patternLines(record.tool_allowance),
    block: patternLines(record.tool_block),
  };
}

/** Replaces all three lists of `record` at the revision the person loaded. */
export function changedToolLists(
  record: RegistrationRecord,
  draft: ToolListsDraft,
): ToolListsInput {
  return {
    revision: record.revision,
    request: {
      tools: patternsOf(draft.request_tools),
      hosts: patternsOf(draft.request_hosts),
    },
    allowance: patternsOf(draft.allowance),
    block: patternsOf(draft.block),
  };
}

/** Whose people answer a waiting call: the Home's, or the receiving Node's. */
export type ToolAskSide = "home" | "receiver";

/** How a side answers: allow for this Run only, add to its list, decline, or (receiver) block. */
export type ToolAskChoice = "run_only" | "add" | "decline" | "block";

/** One call of a remote Run that waits for an answer about a Federation tool list. */
export type ToolAsk = {
  side: ToolAskSide;
  /** The path id of the answer: the admission or, on the Home, the grant. */
  id: string;
  run_id: string;
  grant_id: string;
  task_id: string;
  task_title: string | null;
  tool: string;
  /** The key the lists match; an answer adds it exactly unless a pattern is typed. */
  key: string;
  /** The paused call; an answer names it and the key, so it never lands on a later call. */
  call: string;
  paused_since: string | null;
};

export type ToolAsks = { asks: ToolAsk[]; next_cursor?: string };

/** The answers a side may give; only the receiving Node keeps a Block. */
export function toolAskChoices(side: ToolAskSide): ToolAskChoice[] {
  return side === "receiver"
    ? ["run_only", "add", "decline", "block"]
    : ["run_only", "add", "decline"];
}

/** Where the waiting calls of one registration are listed, and answered under. */
export function toolAsksPath(
  tenant: string,
  node: string,
  sourceNode: string,
): string {
  return `/api/tenants/${encodeURIComponent(tenant)}/nodes/${encodeURIComponent(node)}/registrations/${encodeURIComponent(sourceNode)}/tool-asks`;
}

/** The request body of an answer. It names the call and key it answers, so a page shown
 *  before the Run moved on is refused rather than applied to another call. The pattern only
 *  goes with the choices that list something. */
export function toolAskAnswer(
  ask: Pick<ToolAsk, "key" | "call">,
  choice: ToolAskChoice,
  pattern: string,
): { choice: ToolAskChoice; key: string; call: string; pattern?: string } {
  const answer = { choice, key: ask.key, call: ask.call };
  const typed = pattern.trim();
  return choice !== "decline" && typed !== ""
    ? { ...answer, pattern: typed }
    : answer;
}
