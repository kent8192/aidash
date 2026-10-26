import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  authorizationCatalogRequestsPending,
  authorizationDeclineCatalogRequest,
} from "./generated/aidash";
import type { CatalogApprovalRequestView } from "./generated/models";
import { ApiError } from "./transport";
import { Button } from "./components/ui/button";
import { Textarea } from "./components/ui/textarea";
import { Badge, useI18n } from "./ui";
import { Alert, Hint, Notice } from "./components/patterns";
import { ApprovalReviewPanel } from "./approval-review";

function age(created: string, format: Intl.RelativeTimeFormat): string {
  const minutes = Math.max(
    0,
    Math.round((Date.now() - new Date(created).getTime()) / 60_000),
  );
  if (minutes < 60) return format.format(-minutes, "minute");
  const hours = Math.round(minutes / 60);
  if (hours < 24) return format.format(-hours, "hour");
  return format.format(-Math.round(hours / 24), "day");
}

/** The approver's queue of open Catalog Approval Requests on one Node. */
export function CatalogRequestQueue({
  tenant,
  node,
  subject,
  canApprove,
}: {
  tenant: string;
  node: string;
  subject?: string;
  canApprove: boolean;
}) {
  const { t, locale } = useI18n();
  const relative = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  const [open, setOpen] = useState<CatalogApprovalRequestView>();
  const [declining, setDeclining] = useState<string>();
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const pending = useQuery({
    queryKey: ["catalog-requests-pending", tenant, node],
    queryFn: () =>
      authorizationCatalogRequestsPending(tenant, { node, limit: 100 }),
    staleTime: 0,
    retry: false,
  });
  const decline = async (id: string) => {
    setBusy(true);
    setMessage("");
    try {
      await authorizationDeclineCatalogRequest(tenant, id, { reason });
      setDeclining(undefined);
      setReason("");
      await pending.refetch();
      setMessage(t("approvalRequestDeclined"));
    } catch (error) {
      setMessage(
        error instanceof ApiError && error.status === 403
          ? t("approvalReviewForbidden")
          : String(error),
      );
    } finally {
      setBusy(false);
    }
  };
  if (open)
    return (
      <ApprovalReviewPanel
        tenant={tenant}
        node={node}
        root={open.root}
        requestId={open.id}
        subject={subject}
        canApprove={canApprove}
        onClose={() => setOpen(undefined)}
        onChanged={async () => {
          await pending.refetch();
        }}
      />
    );
  return (
    <section
      className="catalog-request-queue grid min-w-0 gap-2"
      aria-label={t("approvalPendingTitle")}
    >
      <div className="flex flex-wrap items-center gap-2">
        <h4 className="text-[13px] font-semibold text-foreground">
          {t("approvalPendingTitle")}
        </h4>
        <Button
          size="sm"
          variant="ghost"
          disabled={pending.isFetching}
          onClick={() => void pending.refetch()}
        >
          {t("refresh")}
        </Button>
      </div>
      <Hint>{t("approvalPendingHelp")}</Hint>
      {pending.isError && <Alert>{t("approvalReviewUnavailable")}</Alert>}
      {pending.data?.requests.length === 0 && (
        <Hint>{t("approvalPendingEmpty")}</Hint>
      )}
      <ul className="divide-y divide-border rounded-md border border-border">
        {pending.data?.requests.map((request) => (
          <li
            className="catalog-request grid gap-1.5 px-3 py-2.5"
            key={request.id}
          >
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <span className="font-medium text-foreground">
                {request.requester}
              </span>
              <Badge value={request.state} />
              <span className="font-mono text-[11px] text-muted-foreground">
                {request.root.id}@{request.root.version}
              </span>
              <span className="text-[11px] text-muted-foreground">
                {request.node} · {age(request.created_at, relative)}
              </span>
            </div>
            <Hint>{request.reason}</Hint>
            <div className="flex flex-wrap items-center gap-2">
              <Button size="sm" onClick={() => setOpen(request)}>
                {t("approvalPendingReview")}
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() =>
                  setDeclining(
                    declining === request.id ? undefined : request.id,
                  )
                }
              >
                {t("approvalPendingDecline")}
              </Button>
              {request.review && (
                <span className="text-[11px] text-muted-foreground">
                  {t("approvalPendingVisible").replace(
                    "{count}",
                    String(request.review.approvable),
                  )}
                </span>
              )}
            </div>
            {declining === request.id && (
              <div className="grid gap-1.5">
                <Textarea
                  value={reason}
                  disabled={busy}
                  placeholder={t("approvalPendingDeclineReason")}
                  onChange={(event) => setReason(event.target.value)}
                />
                <Button
                  size="sm"
                  disabled={busy || reason.trim().length === 0}
                  onClick={() => void decline(request.id)}
                >
                  {t("approvalPendingDeclineSubmit")}
                </Button>
              </div>
            )}
          </li>
        ))}
      </ul>
      {message && <Notice role="status">{message}</Notice>}
    </section>
  );
}
