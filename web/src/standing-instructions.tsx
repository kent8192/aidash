import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "./components/ui/button";
import { Textarea } from "./components/ui/textarea";
import {
  nodeInstructions,
  nodeInstructionsUpdate,
  tenantInstructions,
  tenantInstructionsUpdate,
} from "./generated/aidash";
import { Alert, Hint, Loading } from "./components/patterns";
import {
  MAX_STANDING_INSTRUCTIONS_BYTES,
  STANDING_INSTRUCTIONS_PROBLEM_MESSAGES,
  standingInstructionsProblems,
  utf8Bytes,
} from "./standing-instructions-model";
import { ApiError } from "./transport";
import { Field, Panel, useI18n } from "./ui";

/** Tenant Instructions, or the Node Instructions of `node` (#1010). Every
 *  member reads them; `canUpdate` comes from `tenant.instructions.update` or
 *  `node.administer`, and the server refuses everyone else. */
export function StandingInstructionsSettings({
  tenant,
  node,
  canUpdate,
}: {
  tenant: string;
  node?: string;
  canUpdate: boolean;
}) {
  const { t } = useI18n();
  const client = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [draft, setDraft] = useState<string | undefined>(undefined);
  const key = ["standing-instructions", tenant, node ?? ""];
  const query = useQuery({
    queryKey: key,
    queryFn: () =>
      node === undefined
        ? tenantInstructions(encodeURIComponent(tenant))
        : nodeInstructions(
            encodeURIComponent(tenant),
            encodeURIComponent(node),
          ),
    retry: false,
  });
  const current = !query.isError ? query.data : undefined;
  const text = draft ?? current?.text ?? "";
  const bytes = utf8Bytes(text);
  const title =
    node === undefined ? t("tenantInstructions") : t("nodeInstructions");
  const help =
    node === undefined
      ? t("tenantInstructionsHelp")
      : t("nodeInstructionsHelp");
  return (
    <Panel title={title}>
      <Hint>{help}</Hint>
      {query.isPending && <Loading />}
      {query.isError && (
        <Alert retry={() => void query.refetch()}>{query.error.message}</Alert>
      )}
      {error && <Alert>{error}</Alert>}
      {saved && !error && <Hint role="status">{t("saved")}</Hint>}
      {current && (
        <form
          className="grid min-w-0 gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (busy || !canUpdate) return;
            const problems = standingInstructionsProblems(text);
            if (problems.length > 0) {
              setError(
                problems
                  .map((problem) =>
                    t(STANDING_INSTRUCTIONS_PROBLEM_MESSAGES[problem]),
                  )
                  .join(" "),
              );
              return;
            }
            setBusy(true);
            setError("");
            setSaved(false);
            const body = { text, expected_revision: current.revision };
            const request =
              node === undefined
                ? tenantInstructionsUpdate(encodeURIComponent(tenant), body)
                : nodeInstructionsUpdate(
                    encodeURIComponent(tenant),
                    encodeURIComponent(node),
                    body,
                  );
            void request
              .then(() => {
                setSaved(true);
                setDraft(undefined);
              })
              .catch((reason) => {
                setError(
                  reason instanceof ApiError && reason.status === 409
                    ? t("instructionsConflict")
                    : reason instanceof Error
                      ? reason.message
                      : String(reason),
                );
              })
              .finally(() => {
                setBusy(false);
                void client.invalidateQueries({ queryKey: key });
              });
          }}
        >
          <Field label={title}>
            <Textarea
              name="text"
              rows={8}
              value={text}
              readOnly={!canUpdate}
              disabled={busy}
              onChange={(event) => {
                setSaved(false);
                setDraft(event.currentTarget.value);
              }}
            />
          </Field>
          <Hint>
            {bytes.toLocaleString()} /{" "}
            {MAX_STANDING_INSTRUCTIONS_BYTES.toLocaleString()}{" "}
            {t("instructionsBytes")}
          </Hint>
          {canUpdate && (
            <div className="flex justify-end">
              <Button disabled={busy}>{t("save")}</Button>
            </div>
          )}
        </form>
      )}
    </Panel>
  );
}
