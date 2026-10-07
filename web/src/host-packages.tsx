import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
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
import { ApiError } from "./transport";
import { Panel, useI18n } from "./ui";
import { RecordView } from "./record-view";
import "./host-packages.css";

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
  if (catalog.isError)
    return (
      <Panel title={japanese ? "Host パッケージ" : "Host packages"}>
        <p className="host-packages" role="alert">
          {japanese
            ? "現在の承認状態を取得できません。"
            : "Current approval state is unavailable."}
        </p>
      </Panel>
    );
  return (
    <Panel title={japanese ? "Host パッケージ" : "Host packages"}>
      <div className="host-packages">
        <p>
          {japanese
            ? "Node が提供する操作を承認待ちとして作成します。内容を確認してから、対象をまとめて承認してください。"
            : "Prepare Node operations as pending installations. Review their definitions before approving the selected set."}
        </p>
        <div className="host-package-groups">
          {groups.map((group) => (
            <label key={group}>
              <input
                type="checkbox"
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
        <button
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
        </button>
        {receipt && (
          <>
            <p role="status">
              {japanese
                ? `${receipt.installations.length} 件を承認待ちとして準備しました。`
                : `${receipt.installations.length} pending definitions prepared.`}
            </p>
            {Object.entries(receipt.unavailable).map(([group, reason]) => (
              <p key={group} role="status">
                {group}: {reason}
              </p>
            ))}
          </>
        )}
        <h3>{japanese ? "まとめて承認" : "Approve a reviewed set"}</h3>
        <button
          disabled={busy}
          onClick={() =>
            void perform(async () => {
              setReview(undefined);
              setReceipt(undefined);
              await catalog.refetch();
            })
          }
        >
          {japanese ? "承認対象を再読み込み" : "Reload approval selection"}
        </button>
        {catalog.isError && (
          <p role="alert">
            {japanese
              ? "現在の承認状態を取得できません。"
              : "Current approval state is unavailable."}
          </p>
        )}
        {revisions
          .filter(
            (item) =>
              item.revision === item.installation.latest_revision &&
              item.installation.active_revision !== item.revision,
          )
          .map((item) => (
            <article className="host-package-review" key={item.installation.id}>
              <label className="host-package-choice">
                <input
                  type="checkbox"
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
                />
                <span>
                  {item.entry.id}@{item.entry.version}
                </span>
              </label>
              <p>
                {japanese ? "導入 revision" : "Installation revision"}:{" "}
                {item.revision} · {item.digest}
              </p>
              <details>
                <summary>
                  {japanese
                    ? "定義と依存関係を確認"
                    : "Review definition and dependencies"}
                </summary>
                <RecordView
                  value={{ entry: item.entry, dependencies: item.bindings }}
                />
              </details>
            </article>
          ))}
        {review && review.installations.length > 0 && (
          <details open>
            <summary>
              {japanese ? "承認する対象" : "Exact approval selection"}
            </summary>
            <pre className="host-package-selection">
              {JSON.stringify(review, null, 2)}
            </pre>
          </details>
        )}
        <button
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
        </button>
        {message && <p role="status">{message}</p>}
      </div>
    </Panel>
  );
}
