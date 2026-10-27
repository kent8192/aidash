import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Textarea } from "../components/ui/textarea";
import { Alert, Hint } from "../components/patterns";
import { Panel, Field, useI18n } from "../ui";
import { apiFetch } from "../transport";
import {
  factsForTarget,
  safetyStopSignature,
  safetyStopTarget,
  type StopInput,
} from "./safety-stop-model";

/** What a Deployment Administrator may read of a Run (ADR 0101). */
type RunControlFacts = {
  id: string;
  phase: string;
  control: string;
  control_revision: number;
  control_source: string;
};

/** The acknowledgement of one command; it says nothing of the Run's state afterwards. */
type Receipt = {
  operation_id: string;
  run_id: string;
  tenant: string;
  action: "pause" | "cancel";
  outcome: "accepted" | "no_op";
};

/**
 * A Deployment Administrator's Safety Stop form: names a Tenant and a Run,
 * shows the allowlisted metadata, and pauses or cancels it with a reason. It
 * never resumes a Run and shows no Run content.
 */
export function SafetyStopPanel({ run: initialRun = "" }: { run?: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [tenant, setTenant] = useState("");
  const [run, setRun] = useState(initialRun);
  const [loaded, setLoaded] = useState<{
    target: string;
    facts: RunControlFacts;
  }>();
  const [reason, setReason] = useState("");
  const [receipt, setReceipt] = useState<Receipt>();
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  // One operation ID per submission: a retry of the same command reuses it, so
  // the server replays the receipt instead of acting twice.
  const attempt = useRef<{ signature: string; key: string }>(undefined);
  // Only the newest lookup may write; an older response is ignored.
  const lookups = useRef(0);
  const path = `/api/runs/${encodeURIComponent(run.trim())}/management`;
  const target = safetyStopTarget(tenant, run);
  // Facts are valid only for the target they were read for, so editing the
  // Tenant or Run ID drops them and a stop cannot reuse another Run's revision.
  const facts = factsForTarget(loaded, target);

  const read = async () => {
    const [named, id] = JSON.parse(target) as [string, string];
    const facts = await apiFetch<RunControlFacts>(
      `/api/runs/${encodeURIComponent(id)}/management?tenant=${encodeURIComponent(named)}`,
    );
    return { target, facts };
  };

  const look = async () => {
    const lookup = ++lookups.current;
    setBusy(true);
    setError("");
    setReceipt(undefined);
    setLoaded(undefined);
    try {
      const result = await read();
      if (lookup === lookups.current) setLoaded(result);
    } catch (cause) {
      if (lookup === lookups.current)
        setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (lookup === lookups.current) setBusy(false);
    }
  };

  const stop = async (action: StopInput["action"]) => {
    if (!facts) return;
    if (
      action === "cancel" &&
      !window.confirm(
        ja
          ? "この Run の取り消しを要求します。外部に送信済みの操作は取り消せません。"
          : "Request cancellation of this Run. Operations already sent to external systems cannot be undone.",
      )
    )
      return;
    const input: StopInput = {
      tenant: tenant.trim(),
      action,
      expected_control_revision: facts.control_revision,
      reason: reason.trim(),
    };
    const signature = safetyStopSignature(run.trim(), input);
    if (attempt.current?.signature !== signature)
      attempt.current = { signature, key: crypto.randomUUID() };
    setBusy(true);
    setError("");
    try {
      setReceipt(
        await apiFetch<Receipt>(path, {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "idempotency-key": attempt.current.key,
          },
          body: JSON.stringify(input),
        }),
      );
      attempt.current = undefined;
      setReason("");
      await client.invalidateQueries();
      setLoaded(await read());
    } catch (cause) {
      // The operation ID is kept, so pressing the button again is a retry.
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const ended =
    !!facts &&
    (["COMPLETED", "FAILED", "CANCELLED"].includes(facts.phase) ||
      facts.control === "CANCELLED");
  const named = tenant.trim() !== "" && run.trim() !== "";
  return (
    <Panel title={ja ? "Run の安全停止" : "Safety Stop"}>
      <Hint>
        {ja
          ? "Tenant と Run を指定して、内容を見ずに一時停止または取り消しを要求します。再開はできません。理由は必須で、Tenant の監査履歴に残ります。"
          : "Name a Tenant and a Run to pause or cancel it without seeing its content. It cannot resume a Run. The reason is required and appears in the Tenant's audit history."}
      </Hint>
      <div className="grid min-w-0 gap-3 sm:grid-cols-2">
        <Field label={ja ? "Tenant ID" : "Tenant ID"}>
          <Input
            value={tenant}
            disabled={busy}
            onChange={(event) => setTenant(event.target.value)}
            autoComplete="off"
          />
        </Field>
        <Field label={ja ? "Run ID" : "Run ID"}>
          <Input
            value={run}
            disabled={busy}
            onChange={(event) => setRun(event.target.value)}
            autoComplete="off"
          />
        </Field>
      </div>
      <Button
        type="button"
        variant="outline"
        className="justify-self-start"
        disabled={busy || !named}
        onClick={() => void look()}
      >
        {ja ? "状態を確認" : "Look up"}
      </Button>
      {error && <Alert>{error}</Alert>}
      {facts && (
        <dl
          aria-label={ja ? "Run の制御状態" : "Run control state"}
          className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-xs"
        >
          <dt className="text-muted-foreground">phase</dt>
          <dd className="font-mono">{facts.phase}</dd>
          <dt className="text-muted-foreground">control</dt>
          <dd className="font-mono">{facts.control}</dd>
          <dt className="text-muted-foreground">control_source</dt>
          <dd className="font-mono">{facts.control_source}</dd>
          <dt className="text-muted-foreground">control_revision</dt>
          <dd className="font-mono">{facts.control_revision}</dd>
        </dl>
      )}
      {facts && !ended && (
        <>
          <Field label={ja ? "理由" : "Reason"}>
            <Textarea
              value={reason}
              maxLength={1000}
              onChange={(event) => setReason(event.target.value)}
            />
          </Field>
          <div className="flex min-w-0 flex-wrap gap-2">
            <Button
              type="button"
              variant="outline"
              disabled={busy || reason.trim() === ""}
              onClick={() => void stop("pause")}
            >
              {ja ? "一時停止" : "Pause"}
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={busy || reason.trim() === ""}
              onClick={() => void stop("cancel")}
            >
              {ja ? "取り消し" : "Cancel"}
            </Button>
          </div>
        </>
      )}
      {ended && (
        <Hint role="status">
          {ja
            ? "この Run はすでに終了または取り消し済みです。"
            : "This Run has already ended or been cancelled."}
        </Hint>
      )}
      {receipt && (
        <Hint role="status">
          {receipt.outcome === "accepted"
            ? ja
              ? "要求を記録しました。実行がすでに止まったことを意味しません。一時停止は進行中の推論を中断しません。"
              : "The request was recorded. It does not mean execution has stopped, and a pause does not interrupt an inference in progress."
            : ja
              ? "何も変更されませんでした。この Run はすでに終了または取り消し済みです。"
              : "Nothing changed: the Run had already ended or been cancelled."}
        </Hint>
      )}
    </Panel>
  );
}
