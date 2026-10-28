import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  authorizationCatalogRequestsMine,
  authorizationCatalogReview,
  authorizationCreateCatalogRequest,
  authorizationSubmitCatalogReview,
  authorizationWithdrawCatalogRequest,
} from "./generated/aidash";
import type {
  ApprovalReview,
  CatalogApprovalRequestView,
  CatalogRestriction,
  ReviewItem,
} from "./generated/models";
import { ApiError } from "./transport";
import { useTenantAdministration } from "./tenant-administration";
import { Badge, useI18n } from "./ui";
import { Button } from "./components/ui/button";
import { Textarea } from "./components/ui/textarea";
import { Alert, Check, GroupChoice, Hint, Notice } from "./components/patterns";
import { restrictionLabel } from "./catalog-approval-model";
import {
  answeredRequest,
  canRequestAgain,
  defaultSelection,
  itemKey,
  needsAction,
  needsAnotherApprover,
  requestActionKey,
  requestChanged,
  requestForRoot,
  restrictionFor,
  reviewSubmission,
  statusKeys,
  submittable,
  type RestrictionEdits,
} from "./approval-review-model";

const statusTone = (status: ReviewItem["status"]) => {
  if (status === "approved") return "success" as const;
  if (status === "approvable" || status === "needs_marketplace_activation")
    return "warning" as const;
  if (status === "missing" || status === "provider_unavailable")
    return "danger" as const;
  return "neutral" as const;
};

function RestrictionEditor({
  assignable,
  value,
  save,
}: {
  assignable: string[];
  value: CatalogRestriction | null;
  save: (value: CatalogRestriction | null) => void;
}) {
  const { t } = useI18n();
  const [subjects, setSubjects] = useState(value?.subjects.join(", ") ?? "");
  const [groups, setGroups] = useState(value?.groups ?? []);
  return (
    <div className="grid gap-1.5 rounded-md border border-border bg-surface px-2.5 py-2">
      <label className="grid gap-1 text-xs">
        <span>{t("catalogRestrictionSubjects")}</span>
        <input
          className="min-w-0 rounded-md border border-border bg-background px-2 py-1 font-mono text-xs text-foreground"
          value={subjects}
          onChange={(event) => setSubjects(event.target.value)}
          placeholder="alice, acme/agents/writer@1.0.0"
        />
      </label>
      {assignable.length > 0 && (
        <div
          className="grid gap-1"
          role="group"
          aria-label={t("catalogRestrictionGroups")}
        >
          <span className="text-xs font-medium">
            {t("catalogRestrictionGroups")}
          </span>
          <GroupChoice
            groups={assignable}
            selected={groups}
            disabled={false}
            change={setGroups}
          />
        </div>
      )}
      <Hint>{t("catalogRestrictionHelp")}</Hint>
      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          onClick={() => {
            const names = subjects
              .split(",")
              .map((name) => name.trim())
              .filter(Boolean);
            save(
              names.length === 0 && groups.length === 0
                ? null
                : { subjects: names, groups },
            );
          }}
        >
          {t("save")}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => save(value)}>
          {t("cancel")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            setSubjects("");
            setGroups([]);
            save(null);
          }}
        >
          {t("catalogUnrestricted")}
        </Button>
      </div>
    </div>
  );
}

function RequestState({
  request,
  review,
  subject,
  busy,
  withdraw,
}: {
  request: CatalogApprovalRequestView;
  review: ApprovalReview;
  subject?: string;
  busy: boolean;
  withdraw: () => void;
}) {
  const { t } = useI18n();
  const changed = requestChanged(request, review);
  return (
    <Notice
      tone={changed || request.state === "declined" ? "warning" : "info"}
      role="status"
      className="approval-request-state"
    >
      <div className="grid gap-1">
        <span>{t(requestActionKey(request, review))}</span>
        {request.decline_reason && (
          <span className="text-xs">
            {t("approvalRequestDeclinedReason")}: {request.decline_reason}
          </span>
        )}
        {request.state === "open" &&
          !changed &&
          subject === request.requester && (
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={withdraw}
            >
              {t("approvalRequestWithdraw")}
            </Button>
          )}
      </div>
    </Notice>
  );
}

/** The server-driven Approval Review for one root on one Node. */
export function ApprovalReviewPanel({
  tenant,
  node,
  root,
  requestId,
  subject,
  onClose,
  onChanged,
}: {
  tenant: string;
  node: string;
  root: { id: string; version: string };
  requestId?: string;
  subject?: string;
  onClose?: () => void;
  onChanged?: () => void | Promise<void>;
}) {
  const { t, local } = useI18n();
  const client = useQueryClient();
  const administration = useTenantAdministration(tenant);
  const assignable = administration.data?.assignable_groups ?? [];
  const review = useQuery({
    queryKey: ["catalog-review", tenant, node, root.id, root.version],
    queryFn: () =>
      authorizationCatalogReview(tenant, {
        node,
        entry: `${root.id}@${root.version}`,
      }),
    staleTime: 0,
    retry: false,
  });
  const mine = useQuery({
    queryKey: ["catalog-requests-mine", tenant, node],
    queryFn: () =>
      authorizationCatalogRequestsMine(tenant, { node, limit: 100 }),
    staleTime: 0,
    retry: false,
  });
  const [selection, setSelection] = useState<{
    digest: string;
    keys: string[];
  }>();
  const [edits, setEdits] = useState<RestrictionEdits>({});
  const [editing, setEditing] = useState<string | null>(null);
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [stale, setStale] = useState(false);
  const current = review.data;
  const selected =
    current === undefined
      ? []
      : selection?.digest === current.closure_digest
        ? selection.keys
        : defaultSelection(current);
  const request = requestId
    ? mine.data?.requests.find((value) => value.id === requestId)
    : current
      ? requestForRoot(mine.data?.requests ?? [], current.root)
      : undefined;
  // Item-level authority comes from the server: a grant on specific
  // definitions is enough, without collection-wide `catalog.approve`.
  const canApproveAny = current?.items.some(needsAction) ?? false;
  const canSubmit =
    current !== undefined &&
    (canApproveAny || submittable(current, selected, edits));
  const askAnother = current !== undefined && needsAnotherApprover(current);
  const reload = async () => {
    await review.refetch();
    await mine.refetch();
    await client.invalidateQueries({ queryKey: ["registry-versions"] });
    await onChanged?.();
  };
  const submit = async () => {
    if (busy || !current) return;
    setBusy(true);
    setMessage("");
    setStale(false);
    try {
      const body = reviewSubmission(
        current,
        selected,
        edits,
        answeredRequest(request, requestId),
      );
      await authorizationSubmitCatalogReview(tenant, body);
      // The refreshed review may keep the same closure digest; start from its
      // actionable items again instead of resubmitting decided ones.
      setSelection(undefined);
      setEdits({});
      await reload();
      setMessage(t("approvalReviewSubmitted"));
    } catch (error) {
      if (
        error instanceof ApiError &&
        error.status === 409 &&
        error.message.includes("APPROVAL_REVIEW_STALE")
      ) {
        setStale(true);
        await reload();
      } else if (
        error instanceof ApiError &&
        error.status === 409 &&
        error.message.includes("APPROVAL_REQUEST_STALE")
      ) {
        setStale(true);
        await reload();
      } else {
        setMessage(
          error instanceof ApiError && error.status === 403
            ? t("approvalReviewForbidden")
            : String(error),
        );
      }
    } finally {
      setBusy(false);
    }
  };
  const createRequest = async () => {
    if (busy || !current) return;
    setBusy(true);
    setMessage("");
    try {
      await authorizationCreateCatalogRequest(tenant, {
        node,
        root: current.root,
        reason,
      });
      setReason("");
      await mine.refetch();
      setMessage(t("approvalRequestCreated"));
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusy(false);
    }
  };
  const withdraw = async () => {
    if (busy || !request) return;
    setBusy(true);
    setMessage("");
    try {
      await authorizationWithdrawCatalogRequest(tenant, request.id);
      await mine.refetch();
      setMessage(t("approvalRequestWithdrawn"));
    } catch (error) {
      setMessage(String(error));
    } finally {
      setBusy(false);
    }
  };
  const choose = (key: string, value: boolean) => {
    if (!current) return;
    const next = value
      ? [...new Set([...selected, key])]
      : selected.filter((item) => item !== key);
    setSelection({ digest: current.closure_digest, keys: next });
  };
  if (review.isPending)
    return (
      <section
        className="approval-review grid gap-2"
        aria-label={t("approvalReview")}
      >
        <h4 className="text-[13px] font-semibold">{t("approvalReview")}</h4>
        <Hint>{t("approvalReviewLoading")}</Hint>
      </section>
    );
  if (review.isError || !current)
    return (
      <section
        className="approval-review grid gap-2"
        aria-label={t("approvalReview")}
      >
        <h4 className="text-[13px] font-semibold">{t("approvalReview")}</h4>
        <Alert>{t("approvalReviewUnavailable")}</Alert>
      </section>
    );
  return (
    <section
      className="approval-review grid min-w-0 gap-2 border-t border-border pt-4"
      aria-label={t("approvalReview")}
    >
      <div className="flex flex-wrap items-center gap-2">
        <h4 className="min-w-0 text-[13px] font-semibold text-foreground">
          {t("approvalReview")}
        </h4>
        <Badge value={t("approvalReviewNode") + ` · ${node}`} tone="neutral" />
        {onClose && (
          <Button size="sm" variant="ghost" onClick={onClose}>
            {t("close")}
          </Button>
        )}
      </div>
      <Hint>
        {t("approvalReviewRoot")}:{" "}
        <span className="font-mono">
          {root.id}@{root.version}
        </span>
      </Hint>
      {current.ready && <Notice role="status">{t("approvalRootReady")}</Notice>}
      {current.hidden > 0 && (
        <Notice tone="warning" role="status" className="approval-hidden">
          {t("approvalHiddenItems").replace("{count}", String(current.hidden))}
        </Notice>
      )}
      {stale && <Alert>{t("approvalReviewStale")}</Alert>}
      {requestId && current && <Hint>{t("approvalAnsweringRequest")}</Hint>}
      <ul className="divide-y divide-border rounded-md border border-border">
        {current.items.map((item) => {
          const key = itemKey(item);
          const checked = selected.includes(key);
          const restriction = restrictionFor(item, edits);
          const actionable = needsAction(item);
          const editable =
            item.can_approve &&
            (needsAction(item) || item.status === "approved");
          return (
            <li
              className="approval-review-item grid min-w-0 gap-1.5 px-3 py-2.5"
              key={key}
              data-status={item.status}
            >
              <div className="flex min-w-0 flex-wrap items-center gap-2">
                {actionable ? (
                  <Check
                    checked={checked}
                    disabled={busy}
                    onChange={(event) => choose(key, event.target.checked)}
                  >
                    <span className="font-medium text-foreground">
                      {local(item.name) || item.reference.id}
                    </span>
                  </Check>
                ) : (
                  <span className="font-medium text-foreground">
                    {local(item.name) || item.reference.id}
                  </span>
                )}
                <Badge value={item.kind} tone="neutral" />
                <span className="font-mono text-[11px] text-muted-foreground">
                  {item.reference.id}@{item.reference.version}
                </span>
                <Badge
                  value={t(statusKeys[item.status][0])}
                  tone={statusTone(item.status)}
                />
                {item.widen && (
                  <Badge value={t("approvalWidens")} tone="warning" />
                )}
              </div>
              {local(item.description) && (
                <Hint>{local(item.description)}</Hint>
              )}
              <Hint>{t(statusKeys[item.status][1])}</Hint>
              {item.installation && (
                <Hint>
                  {t("approvalMarketplaceActivation")}:{" "}
                  <span className="font-mono">
                    {item.installation.installation} · r
                    {item.installation.revision}
                  </span>
                </Hint>
              )}
              <div className="flex min-w-0 flex-wrap items-center gap-2 text-[11px] text-muted-foreground">
                <span>
                  {t("approvalRestriction")}:{" "}
                  {restrictionLabel(restriction) || t("catalogUnrestricted")}
                </span>
                {editable && (
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => setEditing(editing === key ? null : key)}
                  >
                    {t("approvalEditRestriction")}
                  </Button>
                )}
              </div>
              {editing === key && editable && (
                <RestrictionEditor
                  assignable={assignable}
                  value={restriction}
                  save={(value) => {
                    setEdits((edits) => ({ ...edits, [key]: value }));
                    if (!selected.includes(key))
                      setSelection({
                        digest: current.closure_digest,
                        keys: [...selected, key],
                      });
                    setEditing(null);
                  }}
                />
              )}
            </li>
          );
        })}
      </ul>
      {message && <Notice role="status">{message}</Notice>}
      {canSubmit && (
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            disabled={busy || selected.length === 0}
            onClick={() => void submit()}
          >
            {t("approvalApproveSelected")} ({selected.length})
          </Button>
        </div>
      )}
      {request ? (
        <>
          <RequestState
            request={request}
            review={current}
            subject={subject}
            busy={busy}
            withdraw={() => void withdraw()}
          />
          {canRequestAgain(request, current) && (
            <div
              className="grid gap-1.5"
              role="group"
              aria-label={t("approvalRequest")}
            >
              <span className="text-xs font-semibold">
                {t("approvalRequest")}
              </span>
              <Textarea
                value={reason}
                disabled={busy}
                placeholder={t("approvalRequestReason")}
                onChange={(event) => setReason(event.target.value)}
              />
              <Button
                size="sm"
                disabled={busy || reason.trim().length === 0}
                onClick={() => void createRequest()}
              >
                {t("approvalRequestCreate")}
              </Button>
            </div>
          )}
        </>
      ) : (
        !current.ready &&
        (!canApproveAny || askAnother) && (
          <div
            className="grid gap-1.5"
            role="group"
            aria-label={t("approvalRequest")}
          >
            <span className="text-xs font-semibold">
              {t("approvalRequest")}
            </span>
            <Hint>{t("approvalRequestHelp")}</Hint>
            <Textarea
              value={reason}
              disabled={busy}
              placeholder={t("approvalRequestReason")}
              onChange={(event) => setReason(event.target.value)}
            />
            <Button
              size="sm"
              disabled={busy || reason.trim().length === 0}
              onClick={() => void createRequest()}
            >
              {t("approvalRequestCreate")}
            </Button>
          </div>
        )
      )}
    </section>
  );
}
