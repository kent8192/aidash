import { useState } from "react";
import { Link } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { NativeSelect } from "./components/ui/native-select";
import { Alert, Facts, Hint, Loading, Notice } from "./components/patterns";
import { approvalDeciderUpdate } from "./generated/aidash";
import { approvalCopy } from "./approval-settings-copy";
import {
  BUILT_IN_CHOICE,
  approvalDeciderChoices,
  deciderUpdate,
  registryVersionOf,
  versionKey,
} from "./approval-decider-model";
import {
  approvalDeciderKey,
  approvalPreferencesKey,
  useApprovalDecider,
} from "./approval-queries";
import {
  ApprovalBadge,
  DeciderRecipients,
  useRegistryVersions,
} from "./catalog-approval";
import { deciderRecipients } from "./decider";
import { Field, Panel, useI18n } from "./ui";
import { useNodes } from "./nodes";

/** A link to the Registry with one Decider version's Catalog Approval open. */
export function CatalogApprovalLink({
  reference,
  className,
}: {
  reference: { id: string; version: string };
  className?: string;
}) {
  const { locale } = useI18n();
  return (
    <Link
      to="/$section"
      params={{ section: "settings" }}
      search={{ view: "registry", focus: versionKey(reference) }}
      className={
        className ??
        "w-fit text-xs text-brand underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-ring"
      }
    >
      {approvalCopy[locale].reviewCatalogApproval}
    </Link>
  );
}

const message = (reason: unknown) =>
  reason instanceof Error ? reason.message : String(reason);

/** Tenant administration: the Decider that judges Auto, its Catalog Approval
 *  on this Node and where its Routes send Run state. A holder of
 *  `tenant.update` chooses another Decider of hook `approval`, or the built-in
 *  one. */
export function ApprovalDeciderPanel({
  tenant,
  subject,
  canUpdate,
}: {
  tenant: string;
  subject: string;
  canUpdate: boolean;
}) {
  const { locale, entityName } = useI18n();
  const copy = approvalCopy[locale].decider;
  const client = useQueryClient();
  const node = useNodes().selected?.name;
  const decider = useApprovalDecider(tenant);
  const versions = useRegistryVersions({ kind: "subject", tenant, subject });
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");
  if (decider.isError)
    return (
      <Panel title={copy.title}>
        <Alert
          retry={() => void decider.refetch()}
          retryLabel={approvalCopy[locale].retry}
        >
          <p>{approvalCopy[locale].loadFailed}</p>
          <p className="text-muted-foreground">{decider.error.message}</p>
        </Alert>
      </Panel>
    );
  if (!decider.data) return <Loading />;
  const current = decider.data;
  const key = versionKey(current.decider);
  const version = versions.data
    ? registryVersionOf(versions.data, current.decider)
    : null;
  const recipients = version ? deciderRecipients(version.entry) : null;
  const choices = approvalDeciderChoices(versions.data ?? []);
  const value = current.built_in ? BUILT_IN_CHOICE : key;
  const choose = async (choice: string) => {
    setBusy(true);
    setSaved(false);
    setError("");
    try {
      client.setQueryData(
        approvalDeciderKey(tenant, node),
        await approvalDeciderUpdate(
          encodeURIComponent(tenant),
          deciderUpdate(current, choice),
          { node },
        ),
      );
      await client.invalidateQueries({ queryKey: approvalDeciderKey(tenant) });
      setSaved(true);
      // Which modes this Tenant's people can use follows from the Decider.
      await client.invalidateQueries({
        queryKey: approvalPreferencesKey(tenant),
      });
      await client.invalidateQueries({ queryKey: ["thread-approval-mode"] });
    } catch (reason) {
      setError(message(reason));
      // Another administrator may have changed it first: show what holds now.
      await client.invalidateQueries({ queryKey: approvalDeciderKey(tenant) });
    } finally {
      setBusy(false);
    }
  };
  return (
    <Panel title={copy.title}>
      <Hint>{copy.help}</Hint>
      <Facts
        items={[
          [
            copy.chosen,
            <span key="chosen" className="grid min-w-0 gap-0.5">
              {version && <span>{entityName(version.entry)}</span>}
              <span className="font-mono text-xs text-muted-foreground">
                {key}
              </span>
              <span className="text-xs text-muted-foreground">
                {current.built_in ? copy.builtIn : copy.chosenByTenant}
              </span>
            </span>,
          ],
          [
            copy.catalogApproval,
            <span key="approval" className="flex flex-wrap items-center gap-2">
              {version && <ApprovalBadge version={version} />}
              <span>{current.approved ? copy.approved : copy.notApproved}</span>
            </span>,
          ],
        ]}
      />
      {current.registered ? (
        <>
          {recipients && <DeciderRecipients recipients={recipients} />}
          <CatalogApprovalLink reference={current.decider} />
        </>
      ) : (
        <Notice tone="warning" role="status">
          {copy.notRegistered}
        </Notice>
      )}
      {canUpdate && (
        <>
          <Field label={copy.select}>
            <NativeSelect
              value={value}
              disabled={busy}
              onChange={(event) => void choose(event.target.value)}
            >
              <option value={BUILT_IN_CHOICE}>{copy.builtInOption}</option>
              {choices.map(({ entry }) => (
                <option key={versionKey(entry)} value={versionKey(entry)}>
                  {entityName(entry)} · {versionKey(entry)}
                </option>
              ))}
              {value !== BUILT_IN_CHOICE &&
                !choices.some(({ entry }) => versionKey(entry) === value) && (
                  <option value={value}>{value}</option>
                )}
            </NativeSelect>
          </Field>
          {choices.length === 0 && value === BUILT_IN_CHOICE && (
            <Hint>{copy.noChoices}</Hint>
          )}
          {busy && <Loading>{copy.saving}</Loading>}
          {saved && (
            <Hint role="status" className="text-success">
              {copy.saved}
            </Hint>
          )}
          {error && <Alert>{error}</Alert>}
        </>
      )}
    </Panel>
  );
}
