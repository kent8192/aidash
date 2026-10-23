import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { Textarea } from "./components/ui/textarea";
import {
  authorizationCredentials,
  authorizationIssueCredential,
  authorizationRevokeCredential,
} from "./generated/aidash";
import type { Credential, IssuedCredential } from "./generated/models";
import { Badge, Empty, Field, Modal, Panel, useI18n } from "./ui";
import { Alert, Hint, Loading, Notice, RowList } from "./components/patterns";

/** The Subject Credentials a member issued for itself through its Mapping
 *  (#358, ADR 0063). They act as the member and end with that Mapping. */
export function OwnCredentials({
  tenant,
  subject,
  canIssue,
}: {
  tenant: string;
  subject: string;
  /** Whether the Tenant's policy grants `credential.create`. */
  canIssue: boolean;
}) {
  const { t, locale } = useI18n();
  const client = useQueryClient();
  const path = encodeURIComponent(tenant);
  const [issuing, setIssuing] = useState(false);
  const [issued, setIssued] = useState<IssuedCredential | null>(null);
  const [revoking, setRevoking] = useState<Credential | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const credentials = useQuery({
    queryKey: ["authorization", tenant, "credentials", "mine"],
    queryFn: async () => {
      const all: Credential[] = [];
      for (let offset = 0; ; offset += 200) {
        const page = await authorizationCredentials(path, {
          mine: true,
          offset,
        });
        all.push(...page);
        if (page.length < 200) return all;
      }
    },
    refetchInterval: 5000,
  });
  const act = async (action: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await action();
      await client.invalidateQueries({ queryKey: ["authorization", tenant] });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  const close = () => {
    if (busy) return;
    setIssuing(false);
    setRevoking(null);
    setError("");
  };
  return (
    <Panel
      title={t("ownCredentials")}
      action={
        canIssue && (
          <Button
            variant="outline"
            size="sm"
            onClick={() => {
              setError("");
              setIssued(null);
              setIssuing(true);
            }}
          >
            {t("authIssueCredential")}
          </Button>
        )
      }
    >
      <Hint>{t("ownCredentialsHelp")}</Hint>
      {error && !issuing && !revoking && <Alert>{error}</Alert>}
      {credentials.isPending && <Loading />}
      {credentials.isError && <Alert>{credentials.error.message}</Alert>}
      {!credentials.isError && credentials.data?.length === 0 && <Empty />}
      {!credentials.isError && !!credentials.data?.length && (
        <RowList>
          {credentials.data.map((credential) => {
            const state = credential.revoked_at
              ? "authRevoked"
              : Date.parse(credential.expires_at) <= credentials.dataUpdatedAt
                ? "authExpired"
                : "authActive";
            return (
              <div
                className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-x-4 py-2"
                key={credential.id}
              >
                <span className="truncate text-[11px] text-faint">
                  <time className="font-mono" dateTime={credential.created_at}>
                    {new Date(credential.created_at).toLocaleString(locale)}
                  </time>
                  {" · "}
                  {t("authExpires")}{" "}
                  <time className="font-mono" dateTime={credential.expires_at}>
                    {new Date(credential.expires_at).toLocaleString(locale)}
                  </time>
                </span>
                <Badge
                  value={state}
                  tone={
                    state === "authActive"
                      ? "success"
                      : state === "authExpired"
                        ? "warning"
                        : "danger"
                  }
                />
                {credential.revoked_at ? (
                  <span />
                ) : (
                  <Button
                    variant="destructive"
                    size="sm"
                    disabled={busy}
                    onClick={() => {
                      setError("");
                      setRevoking(credential);
                    }}
                  >
                    {t("authRevoke")}
                  </Button>
                )}
              </div>
            );
          })}
        </RowList>
      )}
      {issuing && (
        <Modal title={t("authIssueCredential")} close={close}>
          <form
            className="grid min-w-0 gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              const seconds = Number(
                new FormData(event.currentTarget).get("seconds"),
              );
              void act(async () => {
                setIssued(
                  await authorizationIssueCredential(path, {
                    subject,
                    expires_in_seconds: seconds,
                  }),
                );
                setIssuing(false);
              });
            }}
          >
            {error && <Alert>{error}</Alert>}
            <Field label={t("authLifetime")}>
              <Input
                name="seconds"
                type="number"
                required
                min={1}
                max={2592000}
                defaultValue={3600}
                className="font-mono tabular"
              />
            </Field>
            <div className="flex justify-end gap-2 border-t border-border pt-4">
              <Button disabled={busy}>{t("authIssueCredential")}</Button>
            </div>
          </form>
        </Modal>
      )}
      {issued && (
        <Modal title={t("authIssuedCredential")} close={() => setIssued(null)}>
          <div className="grid min-w-0 gap-3">
            <Notice tone="warning">{t("authTokenOnce")}</Notice>
            <Field label={t("authIssuedToken")}>
              <Textarea
                readOnly
                value={issued.token}
                autoComplete="off"
                spellCheck={false}
                rows={4}
                className="font-mono text-xs break-all"
                onFocus={(event) => event.target.select()}
              />
            </Field>
            <div className="flex justify-end border-t border-border pt-4">
              <Button variant="outline" onClick={() => setIssued(null)}>
                {t("authDismissToken")}
              </Button>
            </div>
          </div>
        </Modal>
      )}
      {revoking && (
        <Modal title={t("authRevoke")} close={close}>
          <div className="grid min-w-0 gap-3">
            <p className="text-[13px] leading-relaxed text-muted-foreground">
              {t("authRevokeHelp")}
            </p>
            {error && <Alert>{error}</Alert>}
            <div className="flex flex-wrap justify-end gap-2 border-t border-border pt-4">
              <Button variant="outline" disabled={busy} onClick={close}>
                {t("cancel")}
              </Button>
              <Button
                variant="destructive"
                disabled={busy}
                onClick={() =>
                  void act(async () => {
                    await authorizationRevokeCredential(path, revoking.id);
                    setRevoking(null);
                  })
                }
              >
                {t("authConfirmRevoke")}
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </Panel>
  );
}
