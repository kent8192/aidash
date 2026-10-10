import { Check, Plus } from "lucide-react";
import { Button } from "./components/ui/button";
import { NativeSelect } from "./components/ui/native-select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./components/ui/table";
import { cn } from "./lib/utils";
import { TransactionComposer } from "./transaction-composer";
import { disambiguateLabels } from "./display-labels";
import {
  RecordView,
  ReferenceName,
  useNodeLabel,
  DisplayState,
} from "./record-view";
import { Fragment, useContext, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  transactions,
  transactionAbort,
  transactionDetails,
  transactionParticipants,
  transactionSubmit,
  transactionTrust,
  transactionTrustList,
} from "./generated/aidash";
import type {
  AtomicTransaction,
  TransactionManifest,
} from "./generated/models";
import { Badge, Empty, Field, Modal, Panel, useI18n } from "./ui";
import { Alert, Disclosure, Facts, Hint, Notice } from "./components/patterns";

const meta = "font-mono text-[11px] text-faint tabular";

const phase = (transaction: AtomicTransaction) => {
  if (transaction.complete)
    return transaction.decision === "COMMIT" ? "COMMITTED" : "ABORTED";
  if (transaction.decision === "ABORT") return "ABORTING";
  if (transaction.visible) return "RELEASING";
  if (transaction.decision === "COMMIT") return "COMMITTING";
  return "PREPARING";
};

export function TransactionsPage({
  nodeId,
  operator = true,
}: {
  nodeId: string;
  operator?: boolean;
}) {
  const { t, locale } = useI18n();
  const nodeLabel = useNodeLabel();
  const displayData = useContext(DisplayState);
  const transactionLabel = (manifest: TransactionManifest) =>
    `${nodeLabel(manifest.coordinator)} · ${new Date(manifest.deadline).toLocaleString()}`;
  const client = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [draft, setDraft] = useState<TransactionManifest | null>(null);
  const [review, setReview] = useState<TransactionManifest | null>(null);
  const [peer, setPeer] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const list = useQuery({
    queryKey: ["transactions"],
    queryFn: () => transactions(),
    refetchInterval: 1000,
  });
  const transactionLabels = disambiguateLabels(
    list.isError ? [] : (list.data ?? []),
    (transaction) => transaction.id,
    (transaction) => transactionLabel(transaction.manifest),
  );
  const participants = useQuery({
    queryKey: ["transactions", "local"],
    queryFn: () => transactionParticipants(),
    enabled: operator,
    refetchInterval: 1000,
  });
  const trust = useQuery({
    queryKey: ["transactions", "trust"],
    queryFn: () => transactionTrustList(),
    enabled: operator,
    refetchInterval: 5000,
  });
  const details = useQuery({
    queryKey: ["transactions", selected],
    queryFn: () => transactionDetails(selected!),
    enabled: !!selected,
    refetchInterval: 1000,
    refetchIntervalInBackground: true,
  });
  const mutate = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
      await client.invalidateQueries();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const failures = [
    list.error,
    participants.error,
    trust.error,
    details.error,
  ].filter(Boolean);
  const current = !details.isError ? details.data : undefined;
  const create = () => {
    setError("");
    setReview(null);
    setDraft({
      id: crypto.randomUUID(),
      coordinator: nodeId,
      isolation: "serializable",
      deadline: new Date(Date.now() + 300000).toISOString(),
      participants: [
        {
          node_id: nodeId,
          mutations: [
            {
              kind: "workspace_state",
              workspace_id: "",
              expected_revision: 0,
              state: {},
            },
          ],
        },
      ],
    });
  };
  const ja = locale === "ja-JP";
  const steps = ja
    ? ["マニフェスト", "確認", "送信"]
    : ["Manifest", "Review", "Submit"];
  const step = review ? (busy ? 2 : 1) : 0;
  const grantState = (grant: {
    enabled: boolean;
    pending_transactions?: string[] | null;
  }) =>
    grant.pending_transactions?.length
      ? "PENDING"
      : grant.enabled
        ? "ENABLED"
        : "DISABLED";
  return (
    <div className="grid min-w-0 gap-6">
      <Hint>{t("transactionsHelp")}</Hint>
      {error && <Alert>{error}</Alert>}
      {failures.map((failure, index) => (
        <Alert key={index}>{failure?.message}</Alert>
      ))}
      <Panel
        title={t("transactions")}
        action={
          <Button size="sm" onClick={create}>
            <Plus aria-hidden />
            {t("transactionCreate")}
          </Button>
        }
      >
        {list.isPending && <Hint>{t("loading")}</Hint>}
        {!list.isError && list.data?.length === 0 && <Empty />}
        {!list.isError && !!list.data?.length && (
          <ul className="divide-y divide-border border-y border-border">
            {list.data.map((transaction) => (
              <li
                className="flex items-start gap-3 py-2.5"
                key={transaction.id}
              >
                <div className="grid min-w-0 flex-1 gap-0.5">
                  <Button
                    variant="link"
                    className="transaction-id h-auto justify-start whitespace-normal px-0 text-left text-[13px] text-foreground hover:text-brand"
                    onClick={() => setSelected(transaction.id)}
                  >
                    {transactionLabels.get(transaction.id)}
                  </Button>
                  <span className={meta}>
                    {t("transactionDeadline")}:{" "}
                    {new Date(transaction.manifest.deadline).toLocaleString()}
                  </span>
                  {transaction.last_error && (
                    <p className="break-words text-xs text-destructive">
                      {transaction.last_error}
                    </p>
                  )}
                </div>
                <Badge value={phase(transaction)} />
              </li>
            ))}
          </ul>
        )}
      </Panel>
      {operator && (
        <>
          <Panel title={t("transactionLocalParticipants")}>
            {!participants.isError && participants.data?.length === 0 && (
              <Empty />
            )}
            {!participants.isError && !!participants.data?.length && (
              <ul className="divide-y divide-border border-y border-border">
                {participants.data.map((participant) => (
                  <li
                    className="flex items-start gap-3 py-2.5"
                    key={participant.id}
                  >
                    <div className="grid min-w-0 flex-1 gap-0.5">
                      <strong className="break-words text-[13px] font-medium text-foreground">
                        {transactionLabels.get(participant.id) ??
                          transactionLabel(participant.manifest)}
                      </strong>
                      <span className="text-xs text-muted-foreground">
                        {t("transactionCoordinator")}:{" "}
                        <ReferenceName id={participant.coordinator} />
                      </span>
                    </div>
                    <Badge value={participant.phase} />
                  </li>
                ))}
              </ul>
            )}
          </Panel>
          <Panel title={t("transactionTrust")}>
            <Hint>{t("transactionTrustHelp")}</Hint>
            <form
              className="flex flex-wrap items-end gap-2"
              onSubmit={(event) => {
                event.preventDefault();
                void mutate(() =>
                  transactionTrust({ node_id: peer.trim(), enabled: true }),
                );
              }}
            >
              <div className="min-w-0 flex-1 basis-48">
                <Field label={t("transactionPeer")}>
                  <NativeSelect
                    required
                    value={peer}
                    onChange={(event) => setPeer(event.target.value)}
                  >
                    <option value="">{t("choose")}</option>
                    {displayData?.peers.map((peer) => (
                      <option key={peer.node_id} value={peer.node_id}>
                        <ReferenceName id={peer.node_id} />
                      </option>
                    ))}
                  </NativeSelect>
                </Field>
              </div>
              <Button
                variant="outline"
                disabled={
                  busy ||
                  !!trust.data?.find((grant) => grant.node_id === peer.trim())
                    ?.pending_transactions?.length
                }
              >
                {t("transactionGrant")}
              </Button>
            </form>
            {!trust.isError && !!trust.data?.length && (
              <Table>
                <TableHeader>
                  <TableRow className="hover:bg-transparent">
                    <TableHead>{t("transactionPeer")}</TableHead>
                    <TableHead className="w-0">{t("status")}</TableHead>
                    <TableHead className="w-0" />
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {trust.data.map((grant) => (
                    <Fragment key={grant.node_id}>
                      <TableRow
                        className={cn(
                          !!grant.pending_transactions?.length && "border-b-0",
                        )}
                      >
                        <TableCell className="max-w-0 py-2">
                          <div className="truncate font-medium">
                            <ReferenceName id={grant.node_id} />
                          </div>
                        </TableCell>
                        <TableCell className="w-0 whitespace-nowrap py-2 align-top">
                          <Badge value={grantState(grant)} />
                        </TableCell>
                        <TableCell className="w-0 whitespace-nowrap py-2 text-right align-top">
                          <Button
                            variant={grant.enabled ? "ghost" : "outline"}
                            size="sm"
                            className={cn(
                              grant.enabled &&
                                !grant.pending_transactions?.length &&
                                "hover:text-destructive",
                            )}
                            disabled={busy}
                            onClick={() =>
                              void mutate(() =>
                                transactionTrust({
                                  node_id: grant.node_id,
                                  enabled:
                                    !grant.enabled &&
                                    !grant.pending_transactions?.length,
                                }),
                              )
                            }
                          >
                            {t(
                              grant.pending_transactions?.length
                                ? "transactionCheckRevocation"
                                : grant.enabled
                                  ? "transactionRevoke"
                                  : "transactionGrant",
                            )}
                          </Button>
                        </TableCell>
                      </TableRow>
                      {!!grant.pending_transactions?.length && (
                        <TableRow className="hover:bg-transparent">
                          <TableCell colSpan={3} className="h-auto pb-2.5 pt-0">
                            <Notice tone="warning" role="status">
                              {t("transactionRevocationPending")}{" "}
                              <span className="break-all font-mono">
                                {grant.pending_transactions.join(", ")}
                              </span>
                            </Notice>
                          </TableCell>
                        </TableRow>
                      )}
                    </Fragment>
                  ))}
                </TableBody>
              </Table>
            )}
          </Panel>
        </>
      )}
      {selected && (
        <Modal title={t("transactionDetails")} close={() => setSelected(null)}>
          {error && <Alert>{error}</Alert>}
          {details.isError && <Alert>{details.error.message}</Alert>}
          {!current && !details.isError && <Hint>{t("loading")}</Hint>}
          {current && (
            <div className="transaction-details grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-start gap-x-3 gap-y-4">
              <strong className="break-words text-[15px] font-semibold text-foreground">
                {transactionLabels.get(current.transaction.id) ??
                  transactionLabel(current.transaction.manifest)}
              </strong>
              <Badge value={phase(current.transaction)} />
              <Facts
                className="col-span-2"
                items={[
                  [
                    t("transactionCoordinator"),
                    <ReferenceName
                      key="coordinator"
                      id={current.transaction.manifest.coordinator}
                    />,
                  ],
                  [
                    t("transactionDeadline"),
                    new Date(
                      current.transaction.manifest.deadline,
                    ).toLocaleString(),
                    true,
                  ],
                  [
                    t("transactionVisibility"),
                    t(
                      current.transaction.visible
                        ? "transactionVisible"
                        : "transactionHidden",
                    ),
                  ],
                ]}
              />
              {current.transaction.last_error && (
                <Notice tone="warning" className="col-span-2">
                  {current.transaction.last_error}
                </Notice>
              )}
              {current.transaction.decision === "COMMIT" &&
                !current.transaction.complete && (
                  <Notice tone="warning" className="col-span-2">
                    {t("transactionRecoveryHelp")}
                  </Notice>
                )}
              {!current.transaction.decision && (
                <div className="col-span-2">
                  <Button
                    variant="destructive"
                    disabled={busy}
                    onClick={() =>
                      void mutate(() =>
                        transactionAbort(current.transaction.id),
                      )
                    }
                  >
                    {t("transactionAbort")}
                  </Button>
                </div>
              )}
              <section className="col-span-2 grid gap-2">
                <h3 className="text-xs font-semibold text-foreground">
                  {t("transactionParticipantVotes")}
                </h3>
                {current.participants.length > 0 ? (
                  <Table>
                    <TableHeader>
                      <TableRow className="hover:bg-transparent">
                        <TableHead>{t("node")}</TableHead>
                        <TableHead className="w-0">{t("status")}</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {current.participants.map((vote) => (
                        <TableRow key={vote.node_id}>
                          <TableCell className="max-w-0 truncate">
                            <ReferenceName id={vote.node_id} />
                          </TableCell>
                          <TableCell>
                            <Badge value={vote.phase} />
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                ) : (
                  <p className="text-xs text-faint">{t("empty")}</p>
                )}
              </section>
              <section className="col-span-2 grid gap-2">
                <h3 className="text-xs font-semibold text-foreground">
                  {t("transactionHistory")}
                </h3>
                {current.history.length > 0 ? (
                  <ol className="grid">
                    {current.history.map((item) => (
                      <li
                        key={item.sequence}
                        className="relative grid gap-0.5 border-l border-border pb-3 pl-4 last:pb-0"
                      >
                        <span
                          aria-hidden
                          className="absolute top-1.5 -left-[3.5px] size-1.5 rounded-full bg-brand-mark"
                        />
                        <div className="flex flex-wrap items-baseline gap-x-3">
                          <strong className="text-xs font-medium text-foreground">
                            {t(item.phase)}
                          </strong>
                          <time dateTime={item.created_at} className={meta}>
                            {new Date(item.created_at).toLocaleString()}
                          </time>
                        </div>
                        <p className="break-words text-xs text-muted-foreground">
                          {item.detail}
                        </p>
                      </li>
                    ))}
                  </ol>
                ) : (
                  <p className="text-xs text-faint">{t("empty")}</p>
                )}
              </section>
              <Disclosure
                className="col-span-2"
                summary={t("transactionManifest")}
              >
                <RecordView value={current.transaction.manifest} />
              </Disclosure>
            </div>
          )}
        </Modal>
      )}
      {draft !== null && (
        <Modal
          title={t("transactionCreate")}
          close={() => {
            if (!busy) setDraft(null);
          }}
        >
          <ol
            className="flex items-center gap-2 text-xs"
            aria-label={ja ? "作成の手順" : "Steps"}
          >
            {steps.map((name, index) => (
              <li
                key={name}
                aria-current={index === step ? "step" : undefined}
                className={cn(
                  "flex items-center gap-1.5",
                  index === step
                    ? "font-medium text-foreground"
                    : index < step
                      ? "text-muted-foreground"
                      : "text-faint",
                )}
              >
                <span
                  aria-hidden
                  className={cn(
                    "inline-flex size-5 items-center justify-center rounded-sm border font-mono text-[11px]",
                    index === step
                      ? "border-brand-line bg-brand-soft text-brand"
                      : index < step
                        ? "border-border bg-raised text-muted-foreground"
                        : "border-border text-faint",
                  )}
                >
                  {index < step ? <Check className="size-3" /> : index + 1}
                </span>
                {name}
                {index < steps.length - 1 && (
                  <span aria-hidden className="mx-1 h-px w-6 bg-border" />
                )}
              </li>
            ))}
          </ol>
          <form
            className="grid min-w-0 gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (review) {
                void mutate(async () => {
                  const admitted = await transactionSubmit(review);
                  setDraft(null);
                  setReview(null);
                  setSelected(admitted.id);
                });
              } else {
                setReview({
                  ...draft,
                  participants: [...draft.participants].sort((a, b) =>
                    a.node_id < b.node_id ? -1 : a.node_id > b.node_id ? 1 : 0,
                  ),
                });
                setError("");
              }
            }}
          >
            <Hint>{t("transactionManifestHelp")}</Hint>
            {error && <Alert>{error}</Alert>}
            {review ? (
              <>
                <Notice tone="warning">{t("transactionReviewHelp")}</Notice>
                {!operator && (
                  <ul
                    aria-label={t("transactionParticipant")}
                    className="grid gap-1 font-mono text-xs"
                  >
                    {review.participants.map((participant) => (
                      <li key={participant.node_id}>{participant.node_id}</li>
                    ))}
                  </ul>
                )}
                <RecordView value={review} />
              </>
            ) : (
              <TransactionComposer
                value={draft}
                change={setDraft}
                operator={operator}
              />
            )}
            <div className="flex flex-wrap justify-end gap-2 border-t border-border pt-4">
              {review && (
                <Button
                  variant="ghost"
                  type="button"
                  disabled={busy}
                  onClick={() => setReview(null)}
                >
                  {t("edit")}
                </Button>
              )}
              <Button disabled={busy}>
                {t(review ? "transactionSubmit" : "transactionReview")}
              </Button>
            </div>
          </form>
        </Modal>
      )}
    </div>
  );
}
