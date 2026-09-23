import { useState } from "react";
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
import { Badge, Empty, Field, JsonView, Modal, Panel, useI18n } from "./ui";

const phase = (transaction: AtomicTransaction) => {
  if (transaction.complete)
    return transaction.decision === "COMMIT" ? "COMMITTED" : "ABORTED";
  if (transaction.decision === "ABORT") return "ABORTING";
  if (transaction.visible) return "RELEASING";
  if (transaction.decision === "COMMIT") return "COMMITTING";
  return "PREPARING";
};

export function TransactionsPage({ nodeId }: { nodeId: string }) {
  const { t } = useI18n();
  const client = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  const [review, setReview] = useState<TransactionManifest | null>(null);
  const [peer, setPeer] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const list = useQuery({
    queryKey: ["transactions"],
    queryFn: () => transactions(),
    refetchInterval: 1000,
  });
  const participants = useQuery({
    queryKey: ["transactions", "local"],
    queryFn: () => transactionParticipants(),
    refetchInterval: 1000,
  });
  const trust = useQuery({
    queryKey: ["transactions", "trust"],
    queryFn: () => transactionTrustList(),
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
    setDraft(
      JSON.stringify(
        {
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
        },
        null,
        2,
      ),
    );
  };
  return (
    <div className="generation-page transaction-page">
      <p className="notice">{t("transactionsHelp")}</p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {failures.map((failure, index) => (
        <p key={index} className="error" role="alert">
          {failure?.message}
        </p>
      ))}
      <Panel
        title={t("transactions")}
        action={
          <button className="primary" onClick={create}>
            {t("transactionCreate")}
          </button>
        }
      >
        {list.isPending && <p className="generation-padding">{t("loading")}</p>}
        {!list.isError && list.data?.length === 0 && <Empty />}
        {!list.isError &&
          list.data?.map((transaction) => (
            <div className="generation-request" key={transaction.id}>
              <div>
                <button
                  className="transaction-id"
                  onClick={() => setSelected(transaction.id)}
                >
                  {transaction.id}
                </button>
                <small>
                  {t("transactionDeadline")}:{" "}
                  {new Date(transaction.manifest.deadline).toLocaleString()}
                </small>
                {transaction.last_error && <p>{transaction.last_error}</p>}
              </div>
              <Badge value={phase(transaction)} />
            </div>
          ))}
      </Panel>
      <Panel title={t("transactionLocalParticipants")}>
        {!participants.isError && participants.data?.length === 0 && <Empty />}
        {!participants.isError &&
          participants.data?.map((participant) => (
            <div className="generation-request" key={participant.id}>
              <div>
                <strong className="transaction-id">{participant.id}</strong>
                <small>
                  {t("transactionCoordinator")}: {participant.coordinator}
                </small>
              </div>
              <Badge value={participant.phase} />
            </div>
          ))}
      </Panel>
      <Panel title={t("transactionTrust")}>
        <div className="generation-padding">
          <p>{t("transactionTrustHelp")}</p>
          <form
            className="transaction-trust-form"
            onSubmit={(event) => {
              event.preventDefault();
              void mutate(() =>
                transactionTrust({ node_id: peer.trim(), enabled: true }),
              );
            }}
          >
            <Field label={t("transactionPeer")}>
              <input
                required
                value={peer}
                placeholder="aidash://peer"
                onChange={(event) => setPeer(event.target.value)}
              />
            </Field>
            <button disabled={busy}>{t("transactionGrant")}</button>
          </form>
        </div>
        {!trust.isError &&
          trust.data?.map((grant) => (
            <div className="generation-request" key={grant.node_id}>
              <div>
                <strong>{grant.node_id}</strong>
                <Badge value={grant.enabled ? "ENABLED" : "DISABLED"} />
              </div>
              <button
                disabled={busy}
                onClick={() =>
                  void mutate(() =>
                    transactionTrust({ ...grant, enabled: !grant.enabled }),
                  )
                }
              >
                {t(grant.enabled ? "transactionRevoke" : "transactionGrant")}
              </button>
            </div>
          ))}
      </Panel>
      {selected && (
        <Modal title={t("transactionDetails")} close={() => setSelected(null)}>
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          {details.isError && (
            <p className="error" role="alert">
              {details.error.message}
            </p>
          )}
          {!current && !details.isError && <p>{t("loading")}</p>}
          {current && (
            <div className="transaction-details">
              <strong className="transaction-id">
                {current.transaction.id}
              </strong>
              <Badge value={phase(current.transaction)} />
              <dl>
                <dt>{t("transactionCoordinator")}</dt>
                <dd>{current.transaction.manifest.coordinator}</dd>
                <dt>{t("transactionDeadline")}</dt>
                <dd>
                  {new Date(
                    current.transaction.manifest.deadline,
                  ).toLocaleString()}
                </dd>
                <dt>{t("transactionVisibility")}</dt>
                <dd>
                  {t(
                    current.transaction.visible
                      ? "transactionVisible"
                      : "transactionHidden",
                  )}
                </dd>
              </dl>
              {current.transaction.last_error && (
                <p className="notice">{current.transaction.last_error}</p>
              )}
              {current.transaction.decision === "COMMIT" &&
                !current.transaction.complete && (
                  <p className="notice">{t("transactionRecoveryHelp")}</p>
                )}
              {!current.transaction.decision && (
                <button
                  disabled={busy}
                  onClick={() =>
                    void mutate(() => transactionAbort(current.transaction.id))
                  }
                >
                  {t("transactionAbort")}
                </button>
              )}
              <h3>{t("transactionParticipantVotes")}</h3>
              {current.participants.map((vote) => (
                <div className="generation-request" key={vote.node_id}>
                  <strong>{vote.node_id}</strong>
                  <Badge value={vote.phase} />
                </div>
              ))}
              <h3>{t("transactionHistory")}</h3>
              <ol className="generation-history">
                {current.history.map((item) => (
                  <li key={item.sequence}>
                    <strong>{t(item.phase)}</strong>
                    <time>{new Date(item.created_at).toLocaleString()}</time>
                    <p>{item.detail}</p>
                  </li>
                ))}
              </ol>
              <details>
                <summary>{t("transactionManifest")}</summary>
                <JsonView value={current.transaction.manifest} />
              </details>
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
          <form
            className="transaction-editor"
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
                try {
                  const parsed: unknown = JSON.parse(draft);
                  if (
                    !parsed ||
                    typeof parsed !== "object" ||
                    !Array.isArray((parsed as TransactionManifest).participants)
                  )
                    throw new Error(t("transactionInvalid"));
                  setReview(parsed as TransactionManifest);
                  setError("");
                } catch {
                  setError(t("transactionInvalid"));
                }
              }
            }}
          >
            <p>{t("transactionManifestHelp")}</p>
            {error && (
              <p className="error" role="alert">
                {error}
              </p>
            )}
            {review ? (
              <>
                <p className="notice">{t("transactionReviewHelp")}</p>
                <JsonView value={review} />
                <button type="button" onClick={() => setReview(null)}>
                  {t("edit")}
                </button>
              </>
            ) : (
              <Field label={t("transactionManifest")}>
                <textarea
                  rows={18}
                  required
                  spellCheck={false}
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                />
              </Field>
            )}
            <button className="primary" disabled={busy}>
              {t(review ? "transactionSubmit" : "transactionReview")}
            </button>
          </form>
        </Modal>
      )}
    </div>
  );
}
