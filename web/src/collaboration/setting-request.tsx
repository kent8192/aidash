import { useState } from "react";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Alert, Hint } from "../components/patterns";
import { Field, useI18n } from "../ui";
import type { HumanRequest } from "../types";
import type { Submit } from "../forms";
import type { SettingRequestForm } from "../generated/models";
import {
  providerAuthorizationStart,
  settingRequestAnswer,
  tenantNodePurge,
  tenantsPurge,
} from "../generated/aidash";
import { connect } from "../connections/connect";
import { outcomeMessage } from "../connections/install";
import { PlanConnectionsPage } from "../plan-connections";
import { settingAuthorization } from "./setting-authorization";
import { answerValue } from "./setting-validation";

export function SettingRequest({
  request,
  tenant,
  subject,
  submit,
}: {
  request: HumanRequest;
  tenant: string;
  subject: string;
  submit: Submit;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const form = JSON.parse(request.prompt) as SettingRequestForm;
  const answer = (action: "accept" | "decline" | "done", value?: unknown) =>
    submit(() =>
      settingRequestAnswer(request.id, {
        action,
        value,
        language: ja ? "ja" : "en",
      }),
    );
  const perform = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setError("");
    setBusy(true);
    try {
      await action();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const path = encodeURIComponent(tenant);
  const start = async () => {
    if (form.kind === "purge") {
      if (form.target.type === "node")
        await tenantNodePurge(path, encodeURIComponent(form.target.node));
      else await tenantsPurge(path);
      return answer("done");
    }
    if (form.kind === "oauth") {
      if (form.target.type === "tool") {
        const outcome = await connect({
          tenant,
          resource: form.target.resource,
          scopes: form.target.scopes,
          returnTo: settingAuthorization(request.id, false),
        });
        if (outcome === "connected" || outcome === "reconnected")
          return answer("done");
        if (outcome === "denied") return answer("decline");
        setError(outcomeMessage(outcome, ja));
      } else if (form.target.type === "provider") {
        const started = await providerAuthorizationStart(
          path,
          form.target.provider,
          { return_to: settingAuthorization(request.id, true) },
        );
        window.location.assign(started.authorization_url);
      }
    }
  };
  const actionForm =
    form.kind === "oauth" || form.kind === "payment" || form.kind === "purge";
  return (
    <div className="grid gap-4">
      {error && <Alert>{error}</Alert>}
      {actionForm ? (
        <>
          <p>{form.label}</p>
          {form.kind === "oauth" && form.target.type === "plan" ? (
            <PlanConnectionsPage
              tenant={tenant}
              subject={subject}
              settingLabel={
                ja ? `${form.label}と接続` : `Connect to ${form.label}`
              }
              onConnected={() => answer("done")}
            />
          ) : (
            <Button
              type="button"
              variant={form.kind === "purge" ? "destructive" : "default"}
              disabled={busy || form.kind === "payment"}
              onClick={() => void perform(start)}
            >
              {form.kind === "purge"
                ? ja
                  ? "完全に消去する"
                  : "Purge permanently"
                : form.kind === "payment"
                  ? ja
                    ? "支払い画面を開く"
                    : "Open payment screen"
                  : ja
                    ? `${form.label}と接続`
                    : `Connect to ${form.label}`}
            </Button>
          )}
          {form.kind === "purge" && (
            <strong>
              {form.target.type === "node" ? form.target.node : tenant}
            </strong>
          )}
          {form.kind === "oauth" && form.target.type === "tool" && (
            <Hint>{form.target.resource}</Hint>
          )}
          {form.kind === "payment" && (
            <Hint>
              {ja
                ? "この環境では支払い画面を利用できません。"
                : "Checkout is unavailable on this deployment."}
            </Hint>
          )}
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => void perform(() => answer("done"))}
          >
            {ja ? "完了したことを報告" : "Report completed"}
          </Button>
        </>
      ) : (
        <form
          className="grid gap-3"
          noValidate
          onSubmit={(event) => {
            event.preventDefault();
            const element = event.currentTarget;
            void perform(async () => {
              const value = await answerValue(form, new FormData(element), ja);
              if (form.kind === "secret") element.reset();
              await answer("accept", value);
            });
          }}
        >
          {form.kind === "secret" && (
            <>
              <Field label={form.label}>
                <Input
                  type="password"
                  name="secret"
                  autoComplete="new-password"
                  onPaste={(event) => {
                    if (
                      /[\p{Cc}\u2028\u2029]/u.test(
                        event.clipboardData.getData("text"),
                      )
                    ) {
                      event.preventDefault();
                      setError(
                        ja
                          ? "改行や制御文字は使用できません。"
                          : "Newlines and control characters are not allowed.",
                      );
                    }
                  }}
                />
              </Field>
              <div className="rounded border p-3">
                <strong>{ja ? "送信先ホスト" : "Destination hosts"}</strong>
                <p>{form.hosts.join(", ")}</p>
                <p>{form.name}</p>
              </div>
            </>
          )}
          {form.kind === "choice" && (
            <Field label={form.label}>
              <select
                name="choice"
                multiple={form.multiple}
                defaultValue={form.multiple ? [] : ""}
              >
                {!form.multiple && (
                  <option value="" disabled>
                    {ja ? "選んでください" : "Select"}
                  </option>
                )}
                {form.options.map((option) => (
                  <option key={option}>{option}</option>
                ))}
              </select>
            </Field>
          )}
          {form.kind === "text" &&
            form.fields.map((field) => (
              <Field key={field.name} label={field.label}>
                <Input name={field.name} />
                {field.constraint?.type === "number" && (
                  <span>
                    {field.constraint.min}–{field.constraint.max}{" "}
                    {field.constraint.unit}
                  </span>
                )}
              </Field>
            ))}
          <Button type="submit" disabled={busy}>
            {ja ? "送信" : "Submit"}
          </Button>
        </form>
      )}
      <Button
        type="button"
        variant="ghost"
        disabled={busy}
        onClick={() => void perform(() => answer("decline"))}
      >
        {ja ? "あとで" : "Later"}
      </Button>
    </div>
  );
}
