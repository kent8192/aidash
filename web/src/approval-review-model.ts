import type {
  ApprovalReview,
  CatalogApprovalRequestView,
  CatalogRestriction,
  RequestApprovalReviewSubmission,
  ReviewItem,
} from "./generated/models";
import { restrictionRequest } from "./catalog-approval-model.ts";

export type RestrictionEdits = Record<string, CatalogRestriction | null>;

/** Visible consumers are qualified subjects returned by the review. */
export const consumerLabel = (item: Pick<ReviewItem, "consumers">): string =>
  item.consumers.join(", ");

export const itemKey = (item: Pick<ReviewItem, "reference">) =>
  `${item.reference.id}@${item.reference.version}`;

/** An item the approver can act on now. An already approved item is actionable
 *  only when its restriction must widen to admit its consumers. */
export function needsAction(item: ReviewItem): boolean {
  if (!item.can_approve) return false;
  if (
    item.status === "approvable" ||
    item.status === "needs_marketplace_activation"
  )
    return true;
  return item.status === "approved" && item.widen;
}

export function defaultSelection(review: ApprovalReview): string[] {
  return review.items.filter(needsAction).map(itemKey);
}

/** The restriction to send: an omitted one keeps the stored restriction, and
 *  an explicit "Unrestricted" edit clears it with an empty restriction. */
export function restrictionPayload(
  item: ReviewItem,
  edits: RestrictionEdits,
): CatalogRestriction | null {
  const key = itemKey(item);
  if (!Object.prototype.hasOwnProperty.call(edits, key))
    return item.proposed_restriction;
  return restrictionRequest(edits[key]) ?? null;
}

export function restrictionFor(
  item: ReviewItem,
  edits: RestrictionEdits,
): CatalogRestriction | null {
  const key = itemKey(item);
  return Object.prototype.hasOwnProperty.call(edits, key)
    ? edits[key]
    : item.proposed_restriction;
}

/** One submission for every selected local approval and installation. */
export function reviewSubmission(
  review: ApprovalReview,
  selected: readonly string[],
  edits: RestrictionEdits,
  request?: string | null,
  acknowledged: Readonly<Record<string, boolean>> = {},
): RequestApprovalReviewSubmission {
  const chosen = new Set(selected);
  const items = [];
  const installations = [];
  for (const item of review.items) {
    if (!chosen.has(itemKey(item))) continue;
    // Default selection is actionable items only. An approved item becomes
    // selectable deliberately when its restriction is edited.
    if (!needsAction(item) && !(item.status === "approved" && item.can_approve))
      continue;
    if (item.installation) {
      installations.push({
        installation: item.installation.installation,
        revision: item.installation.revision,
        digest: item.installation.digest,
        expected_activation_revision:
          item.installation.expected_activation_revision,
        restriction: restrictionPayload(item, edits),
      });
    } else if (item.digest) {
      items.push({
        reference: item.reference,
        ...(acknowledged[itemKey(item)]
          ? { provider_warning_acknowledged: true }
          : {}),
        digest: item.digest,
        expected_revision: item.expected_revision,
        restriction: restrictionPayload(item, edits),
      });
    }
  }
  return {
    node: review.node,
    root: review.root,
    closure_digest: review.closure_digest,
    ...(request ? { request } : {}),
    items,
    installations,
  };
}

/** The request that belongs to this root, preferring the one still open. */
export function requestForRoot(
  requests: readonly CatalogApprovalRequestView[],
  root: { id: string; version: string },
): CatalogApprovalRequestView | undefined {
  const matching = requests.filter(
    (request) =>
      request.root.id === root.id && request.root.version === root.version,
  );
  return matching.find((request) => request.state === "open") ?? matching[0];
}

/** The request a submission answers: only one still open. A final request
 *  stays visible, but naming it would make the server refuse the submission. */
export function answeredRequest(
  request: CatalogApprovalRequestView | undefined,
  requestId?: string,
): string | undefined {
  if (request) return request.state === "open" ? request.id : undefined;
  return requestId;
}

/** The submission would decide something: a selected item the caller can act
 *  on, or an approved item whose restriction the caller edited. */
export function submittable(
  review: ApprovalReview,
  selected: readonly string[],
  edits: RestrictionEdits,
): boolean {
  const body = reviewSubmission(review, selected, edits);
  return (body.items?.length ?? 0) + (body.installations?.length ?? 0) > 0;
}

/** The closure still needs a decision this caller cannot make. */
export function needsAnotherApprover(review: ApprovalReview): boolean {
  if (review.ready) return false;
  if (review.hidden > 0) return true;
  return review.items.some(
    (item) => !needsAction(item) && (item.status !== "approved" || item.widen),
  );
}

/** Whether the requester may ask again: the pinned closure changed, or the
 *  earlier request is final while the root is still not ready. */
export function canRequestAgain(
  request: CatalogApprovalRequestView,
  review: ApprovalReview,
): boolean {
  if (review.ready) return false;
  return requestChanged(request, review) || request.state !== "open";
}

/** The pinned closure changed since the requester asked. */
export function requestChanged(
  request: CatalogApprovalRequestView,
  review: ApprovalReview,
): boolean {
  return (
    request.closure_digest !== review.closure_digest ||
    request.state === "stale"
  );
}

export function requestActionKey(
  request: CatalogApprovalRequestView,
  review: ApprovalReview,
): string {
  if (requestChanged(request, review)) return "approvalRequestChanged";
  switch (request.next_action) {
    case "waiting_for_approver":
      return "approvalRequestWaiting";
    case "ready":
      return "approvalRootReady";
    case "declined":
      return "approvalRequestDeclined";
    case "withdrawn":
      return "approvalRequestWithdrawn";
    case "stale":
      return "approvalRequestChanged";
    case "fulfilled":
      return "approvalRequestFulfilled";
  }
}

export const statusKeys: Record<ReviewItem["status"], [string, string]> = {
  approved: ["approvalStatusApproved", "approvalStatusApprovedHelp"],
  approvable: ["approvalStatusApprovable", "approvalStatusApprovableHelp"],
  needs_other_approver: [
    "approvalStatusNeedsOther",
    "approvalStatusNeedsOtherHelp",
  ],
  needs_marketplace_activation: [
    "approvalStatusMarketplace",
    "approvalStatusMarketplaceHelp",
  ],
  missing: ["approvalStatusMissing", "approvalStatusMissingHelp"],
  provider_unavailable: [
    "approvalStatusProviderUnavailable",
    "approvalStatusProviderUnavailableHelp",
  ],
  secret_or_connection_missing: [
    "approvalStatusSourceMissing",
    "approvalStatusSourceMissingHelp",
  ],
};
