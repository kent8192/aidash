import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import {
  authorizationCatalog,
  marketplaceApproveSet,
  marketplaceHostPackages,
} from "./generated/aidash";
import type {
  ApprovalSet,
  MarketplaceInstallationRevision,
  PendingHostPackages,
} from "./generated/models";
import { Button } from "./components/ui/button";
import { ApiError } from "./transport";
import { Panel, useI18n } from "./ui";
import { RecordView } from "./record-view";
import {
  Alert,
  Check,
  Disclosure,
  Hint,
  Notice,
  Pre,
} from "./components/patterns";

const groups = [
  "shell",
  "python",
  "outbound_get",
  "apply_patch",
  "file_share",
  "task_assign",
] as const;

/** Review retains the exact visible revision and approval fence until submission. */
export function HostPackages({
  tenant,
  revisions,
  onSaved,
}: {
  tenant: string;
  revisions: MarketplaceInstallationRevision[];
  onSaved: () => Promise<void>;
}) {
  const { locale } = useI18n();
  const japanese = locale === "ja-JP";
  const [selectedGroups, setSelectedGroups] = useState<string[]>([]);
  const [receipt, setReceipt] = useState<PendingHostPackages>();
  const [review, setReview] = useState<ApprovalSet>();
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const attempt = useRef<{ fingerprint: string; key: string }>(null);
  const catalog = useQuery({
    queryKey: ["marketplace", "operator", tenant, "review-catalog"],
    queryFn: () => authorizationCatalog(tenant),
    enabled: !!tenant,
    retry: false,
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
  useEffect(() => {
    if (catalog.isError) {
      queueMicrotask(() => {
        setReview(undefined);
        setReceipt(undefined);
      });
    }
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
          ? japanese
            ? "承認対象が変更されました。一覧を再読み込みし、内容を再確認してください。"
            : "The reviewed selection changed. Reload the list and review it again."
          : String(error),
      );
    } finally {
      setBusy(false);
    }
  }
  const unavailable = japanese
    ? "現在の承認状態を取得できません。"
    : "Current approval state is unavailable.";
  if (catalog.isError)
    return (
      <Panel title={japanese ? "Host パッケージ" : "Host packages"}>
        <Alert>{unavailable}</Alert>
      </Panel>
    );
  const pending = revisions.filter(
    (item) =>
      item.revision === item.installation.latest_revision &&
      item.installation.active_revision !== item.revision,
  );
  const selectedCount = review?.installations.length ?? 0;
  return (
    <Panel title={japanese ? "Host パッケージ" : "Host packages"}>
      <Hint>
        {japanese
          ? "Node が提供する操作を承認待ちとして作成します。内容を確認してから、対象をまとめて承認してください。"
          : "Prepare Node operations as pending installations. Review their definitions before approving the selected set."}
      </Hint>
      <div className="flex flex-wrap items-center gap-2">
        {groups.map((group) => (
          <label
            key={group}
            className="inline-flex h-7 cursor-pointer items-center gap-2 rounded-md border border-border bg-surface px-2.5 font-mono text-xs text-foreground transition-colors hover:bg-raised has-[:checked]:border-brand-line has-[:checked]:bg-brand-soft"
          >
            <input
              type="checkbox"
              className="size-3.5 shrink-0 accent-primary"
              disabled={busy}
              checked={selectedGroups.includes(group)}
              onChange={(event) =>
                setSelectedGroups((previous) =>
                  event.target.checked
                    ? [...previous, group]
                    : previous.filter((name) => name !== group),
                )
              }
            />
            {group}
          </label>
        ))}
      </div>
      <div>
        <Button
          variant="outline"
          disabled={!tenant || !selectedGroups.length || busy}
          onClick={() =>
            void perform(async () => {
              const fingerprint = JSON.stringify([tenant, selectedGroups]);
              if (attempt.current?.fingerprint !== fingerprint)
                attempt.current = { fingerprint, key: crypto.randomUUID() };
              const result = await marketplaceHostPackages({
                tenant,
                groups: selectedGroups,
                idempotency_key: attempt.current!.key,
              });
              setReceipt(result);
              attempt.current = null;
            })
          }
        >
          {japanese ? "承認待ちを作成" : "Prepare pending packages"}
        </Button>
      </div>
      {receipt && (
        <div className="grid gap-1">
          <Notice role="status">
            {japanese
              ? `${receipt.installations.length} 件を承認待ちとして準備しました。`
              : `${receipt.installations.length} pending definitions prepared.`}
          </Notice>
          {Object.entries(receipt.unavailable).map(([group, reason]) => (
            <p
              key={group}
              role="status"
              className="font-mono text-xs text-warning"
            >
              {group}: {reason}
            </p>
          ))}
        </div>
      )}
      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border pt-3">
        <h3 className="text-xs font-semibold text-foreground">
          {japanese ? "まとめて承認" : "Approve a reviewed set"}
        </h3>
        <Button
          variant="ghost"
          size="sm"
          disabled={busy}
          onClick={() =>
            void perform(async () => {
              setReview(undefined);
              setReceipt(undefined);
              await catalog.refetch();
            })
          }
        >
          <RefreshCw aria-hidden />
          {japanese ? "承認対象を再読み込み" : "Reload approval selection"}
        </Button>
      </div>
      {pending.length > 0 && (
        <ul className="divide-y divide-border border-y border-border">
          {pending.map((item) => (
            <li className="grid gap-1 py-2" key={item.installation.id}>
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <Check
                  className="font-mono text-xs"
                  disabled={busy || !catalog.isSuccess || catalog.isError}
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
                  {japanese ? "導入 revision" : "Installation revision"}:{" "}
                  {item.revision} · {item.digest}
                </span>
              </div>
              <Disclosure
                className="pl-5.5"
                summary={
                  japanese
                    ? "定義と依存関係を確認"
                    : "Review definition and dependencies"
                }
              >
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
            {japanese ? "承認する対象" : "Exact approval selection"}
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
                {japanese ? "有効化revision" : "activation revision"}{" "}
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
                · {japanese ? "カタログrevision" : "catalog revision"}{" "}
                {selection.expected_catalog_revision}
              </li>
            ))}
          </ul>
          <Disclosure
            summary={
              japanese ? "送信する内容（JSON）" : "Submitted request (JSON)"
            }
          >
            <Pre className="host-package-selection max-h-72">
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
            !catalog.isSuccess ||
            catalog.isError
          }
          onClick={() =>
            void perform(async () => {
              await marketplaceApproveSet(review!);
              setReview(undefined);
              setMessage(
                japanese
                  ? "選択した対象を承認し、有効化しました。"
                  : "The reviewed set is approved and active.",
              );
            })
          }
        >
          {japanese
            ? "選択した対象を承認して有効化"
            : "Approve and activate selected set"}
        </Button>
      </div>
      {message && <Notice role="status">{message}</Notice>}
    </Panel>
  );
}
