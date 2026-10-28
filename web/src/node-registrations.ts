import type { RegistrationInput, RegistrationRecord } from "./generated/models";
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
