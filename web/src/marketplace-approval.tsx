import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { marketplaceApproveSet } from "./generated/aidash";
import type {
  ApprovalSet,
  MarketplaceInstallationRevision,
} from "./generated/models";
import { Button } from "./components/ui/button";
import { ApiError } from "./transport";
import { catalogBindings, useNodes } from "./nodes";
import { Panel, useI18n } from "./ui";
import { RecordView } from "./record-view";
import { marketplaceCopy } from "./marketplace-copy";
import {
  Alert,
  Check,
  Disclosure,
  Hint,
  Notice,
  Pre,
} from "./components/patterns";

/** Approves and activates pending installations of the subject's own Tenant
 *  in one request (`catalog.approve`). Review retains the exact visible
 *  revision and approval fence until submission. */
export function ApprovalSetReview({
  tenant,
  context,
  revisions,
  onSaved,
}: {
  tenant: string;
  /** The Marketplace query key the review's catalog read lives under. */
  context: readonly unknown[];
  revisions: MarketplaceInstallationRevision[];
  onSaved: () => Promise<void>;
}) {
  const { locale } = useI18n();
  const copy = marketplaceCopy[locale];
  const [review, setReview] = useState<ApprovalSet>();
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  // An activation is decided against the default Node's Catalog Approvals.
  const { home } = useNodes();
  const catalog = useQuery({
    queryKey: [...context, "review-catalog", home?.name],
    queryFn: () => catalogBindings(tenant, home!.name),
    enabled: home !== undefined,
    retry: false,
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
  useEffect(() => {
    if (catalog.isError) queueMicrotask(() => setReview(undefined));
  }, [catalog.isError]);
  async function perform(action: () => Promise<void>) {
    setBusy(true);
    setMessage("");
    try {
      await action();
      await onSaved();
    } catch (error) {
      setMessage(
        error instanceof ApiError && error.status === 409
          ? copy.selectionChanged
          : String(error),
      );
    } finally {
      setBusy(false);
    }
  }
  if (catalog.isError)
    return (
      <Panel title={copy.reviewSet}>
        <Alert>{copy.approvalUnavailable}</Alert>
      </Panel>
    );
  const pending = revisions.filter(
    (item) =>
      item.revision === item.installation.latest_revision &&
      item.installation.active_revision !== item.revision,
  );
  const selectedCount = review?.installations.length ?? 0;
  return (
    <Panel
      title={copy.reviewSet}
      action={
        <Button
          variant="ghost"
          size="sm"
          disabled={busy}
          onClick={() =>
            void perform(async () => {
              setReview(undefined);
              await catalog.refetch();
            })
          }
        >
          <RefreshCw aria-hidden />
          {copy.reloadSelection}
        </Button>
      }
    >
      <Hint>{copy.reviewSetHelp}</Hint>
      {pending.length > 0 && (
        <ul className="divide-y divide-border border-y border-border">
          {pending.map((item) => (
            <li className="grid gap-1 py-2" key={item.installation.id}>
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <Check
                  className="font-mono text-xs"
                  disabled={busy || !catalog.isSuccess}
                  checked={
                    review?.installations.some(
                      (selection) =>
                        selection.installation === item.installation.id,
                    ) ?? false
                  }
                  onChange={(event) => {
                    const current: ApprovalSet = review ?? {
                      tenant,
                      installations: [],
                      approvals: [],
                    };
                    const installations = current.installations.filter(
                      (selection) =>
                        selection.installation !== item.installation.id,
                    );
                    const approvals = current.approvals.filter(
                      (selection) =>
                        selection.reference.id !== item.entry.id ||
                        selection.reference.version !== item.entry.version,
                    );
                    if (event.target.checked) {
                      const approval = catalog.data?.find(
                        (binding) =>
                          binding.entry_id === item.entry.id &&
                          binding.entry_version === item.entry.version,
                      );
                      installations.push({
                        installation: item.installation.id,
                        revision: item.revision,
                        digest: item.digest,
                        expected_activation_revision:
                          item.installation.activation_revision,
                      });
                      approvals.push({
                        reference: {
                          id: item.entry.id,
                          version: item.entry.version,
                        },
                        expected_catalog_revision: approval?.revision ?? 0,
                      });
                    }
                    setReview({ tenant, installations, approvals });
                  }}
                >
                  {item.entry.id}@{item.entry.version}
                </Check>
                <span className="min-w-0 break-all font-mono text-[11px] text-faint">
                  {copy.installationRevision}: {item.revision} · {item.digest}
                </span>
              </div>
              <Disclosure className="pl-5.5" summary={copy.reviewDefinition}>
                <RecordView
                  value={{ entry: item.entry, dependencies: item.bindings }}
                />
              </Disclosure>
            </li>
          ))}
        </ul>
      )}
      {review && selectedCount > 0 && (
        <section className="grid gap-2 rounded-md border border-brand-line bg-brand-soft/40 px-3 py-2.5">
          <h4 className="flex items-center gap-2 text-xs font-medium text-foreground">
            {copy.exactSelection}
            <span className="rounded-sm bg-primary px-1.5 font-mono text-[11px] text-primary-foreground tabular">
              {selectedCount}
            </span>
          </h4>
          <ul className="grid gap-1 font-mono text-[11px] text-muted-foreground">
            {review.installations.map((selection) => (
              <li key={selection.installation} className="break-all">
                <span className="text-foreground">
                  {selection.installation}
                </span>{" "}
                · r{selection.revision} · {selection.digest} ·{" "}
                {copy.activationRevision}{" "}
                {selection.expected_activation_revision}
              </li>
            ))}
            {review.approvals.map((selection) => (
              <li
                key={`${selection.reference.id}@${selection.reference.version}`}
                className="break-all"
              >
                <span className="text-foreground">
                  {selection.reference.id}@{selection.reference.version}
                </span>{" "}
                · {copy.catalogRevision} {selection.expected_catalog_revision}
              </li>
            ))}
          </ul>
          <Disclosure summary={copy.submittedRequest}>
            <Pre className="approval-set-selection max-h-72">
              {JSON.stringify(review, null, 2)}
            </Pre>
          </Disclosure>
        </section>
      )}
      <div>
        <Button
          disabled={
            busy ||
            !review?.installations.length ||
            review.installations.length > 32 ||
            !catalog.isSuccess
          }
          onClick={() =>
            void perform(async () => {
              await marketplaceApproveSet(review!);
              setReview(undefined);
              setMessage(copy.setApproved);
            })
          }
        >
          {copy.approveSet}
        </Button>
      </div>
      {message && <Notice role="status">{message}</Notice>}
    </Panel>
  );
}
