import { useId, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Badge as Tag } from "./components/ui/badge";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import {
  Alert,
  Check,
  Disclosure,
  Facts,
  Hint,
  Loading,
  Notice,
  inlineFormClass,
} from "./components/patterns";
import {
  approvalPreferencesUpdate,
  approvalRuleCreate,
  approvalRuleDelete,
  approvalRules,
  approvalSettings,
} from "./generated/aidash";
import type {
  ApprovalKind,
  ApprovalMode,
  ApprovalModeReason,
  ApprovalRuleView,
  RuleEffect,
} from "./generated/models";
import { CatalogApprovalLink } from "./approval-decider";
import {
  approvalPreferencesKey,
  useApprovalDecider,
  useApprovalPreferences,
} from "./approval-queries";
import { versionKey } from "./approval-decider-model";
import { approvalCopy } from "./approval-settings-copy";
import {
  approvalKinds,
  approvalModes,
  bypassNeedsConfirmation,
  modeAllowed,
  modeReason,
  needsApprovalDecider,
  preferencesUpdate,
  ruleEffects,
  ruleInput,
  ruleTargetText,
  targetExamples,
} from "./approval-settings-model";
import { Field, Modal, Panel, useI18n, type StatusTone } from "./ui";

const effectTones: Record<RuleEffect, StatusTone> = {
  allow: "success",
  ask: "warning",
  deny: "danger",
};

const message = (reason: unknown) =>
  reason instanceof Error ? reason.message : String(reason);

function BypassConfirmation({
  busy,
  confirm,
  close,
}: {
  busy: boolean;
  confirm: () => void;
  close: () => void;
}) {
  const { locale } = useI18n();
  const copy = approvalCopy[locale];
  return (
    <Modal title={copy.bypassTitle} close={close}>
      {copy.bypassBody.map((paragraph) => (
        <p key={paragraph} className="text-[13px] leading-relaxed">
          {paragraph}
        </p>
      ))}
      <div className={inlineFormClass}>
        <Button
          type="button"
          variant="destructive"
          disabled={busy}
          onClick={confirm}
        >
          {copy.bypassConfirm}
        </Button>
        <Button type="button" variant="outline" disabled={busy} onClick={close}>
          {copy.cancel}
        </Button>
      </div>
    </Modal>
  );
}

function ModeOption({
  mode,
  checked,
  allowed,
  reason,
  decider,
  disabled,
  choose,
}: {
  mode: ApprovalMode;
  checked: boolean;
  allowed: boolean;
  reason: ApprovalModeReason | null;
  /** The Tenant's approval Decider, once known. */
  decider: { id: string; version: string } | null;
  disabled: boolean;
  choose: () => void;
}) {
  const { locale } = useI18n();
  const copy = approvalCopy[locale];
  const id = useId();
  return (
    <div className="grid min-w-0 gap-1">
      <label className="flex min-w-0 cursor-pointer items-start gap-3 rounded-md border border-border bg-surface px-3 py-2.5 transition-colors hover:bg-raised has-[:checked]:border-brand-line has-[:checked]:bg-brand-soft has-[:disabled]:cursor-not-allowed has-[:disabled]:hover:bg-surface">
        <input
          type="radio"
          name="approval-mode"
          className="mt-[3px] size-3.5 shrink-0 cursor-pointer accent-primary disabled:cursor-not-allowed"
          checked={checked}
          disabled={disabled || !allowed}
          aria-labelledby={`${id}-label`}
          aria-describedby={`${id}-description`}
          onChange={choose}
        />
        <span className="grid min-w-0 gap-0.5">
          <span
            id={`${id}-label`}
            className="text-[13px] font-medium text-foreground"
          >
            {copy.modes[mode].label}
          </span>
          <span
            id={`${id}-description`}
            className="text-xs leading-relaxed text-muted-foreground [overflow-wrap:anywhere]"
          >
            {copy.modes[mode].description}
            {reason &&
              ` ${copy.modeReason[reason](decider ? versionKey(decider) : "")}`}
          </span>
        </span>
      </label>
      {reason === "decider_unapproved" && decider && (
        <CatalogApprovalLink
          reference={decider}
          className="ml-9 w-fit text-xs text-brand underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-ring"
        />
      )}
    </div>
  );
}

function DefaultMode({ tenant }: { tenant: string }) {
  const { locale } = useI18n();
  const copy = approvalCopy[locale];
  const client = useQueryClient();
  const preferences = useApprovalPreferences(tenant);
  // Only a mode held back by the Decider needs to name it.
  const decider = useApprovalDecider(
    tenant,
    preferences.data ? needsApprovalDecider(preferences.data.modes) : false,
  );
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");
  if (preferences.isError)
    return (
      <Alert retry={() => void preferences.refetch()} retryLabel={copy.retry}>
        <p>{copy.loadFailed}</p>
        <p className="text-muted-foreground">{preferences.error.message}</p>
      </Alert>
    );
  if (!preferences.data) return <Loading />;
  const current = preferences.data;
  const save = async (mode: ApprovalMode, confirmedBypass: boolean) => {
    setBusy(true);
    setSaved(false);
    setError("");
    try {
      client.setQueryData(
        approvalPreferencesKey(tenant),
        await approvalPreferencesUpdate(
          encodeURIComponent(tenant),
          preferencesUpdate(current, mode, confirmedBypass),
        ),
      );
      setSaved(true);
    } catch (reason) {
      setError(message(reason));
      // A stale revision or a lowered ceiling: show what the server holds now.
      await client.invalidateQueries({
        queryKey: approvalPreferencesKey(tenant),
      });
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  };
  return (
    <Panel title={copy.modeTitle}>
      <Hint>{copy.modeHelp}</Hint>
      <fieldset className="grid min-w-0 gap-2" aria-label={copy.modeTitle}>
        {approvalModes.map((mode) => (
          <ModeOption
            key={mode}
            mode={mode}
            checked={current.default_mode === mode}
            allowed={modeAllowed(current.modes, mode)}
            reason={modeReason(current.modes, mode)}
            decider={decider.data?.decider ?? null}
            disabled={busy}
            choose={() =>
              bypassNeedsConfirmation(current, mode)
                ? setConfirming(true)
                : void save(mode, false)
            }
          />
        ))}
      </fieldset>
      {busy && <Loading>{copy.saving}</Loading>}
      {saved && (
        <Hint role="status" className="text-success">
          {copy.saved}
        </Hint>
      )}
      {error && <Alert>{error}</Alert>}
      {confirming && (
        <BypassConfirmation
          busy={busy}
          confirm={() => void save("bypass", true)}
          close={() => setConfirming(false)}
        />
      )}
    </Panel>
  );
}

function RuleRow({
  rule,
  remove,
}: {
  rule: ApprovalRuleView;
  /** Absent in the read-only view of another member's rules. */
  remove?: () => void;
}) {
  const { locale } = useI18n();
  const copy = approvalCopy[locale];
  const target = ruleTargetText(rule.target);
  return (
    <li className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5 py-2.5">
      <Tag tone="neutral">{copy.kinds[rule.kind].label}</Tag>
      <span
        className={
          target === null
            ? "min-w-0 flex-1 text-[13px] text-muted-foreground"
            : "min-w-0 flex-1 font-mono text-xs [overflow-wrap:anywhere]"
        }
      >
        {target ?? copy.everyTarget}
      </span>
      <Tag tone={effectTones[rule.effect]}>{copy.effects[rule.effect]}</Tag>
      {!rule.effective && <Tag tone="warning">{copy.disabledByPolicy}</Tag>}
      {remove && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          aria-label={copy.deleteRule(
            copy.kinds[rule.kind].label,
            target ?? copy.everyTarget,
          )}
          onClick={remove}
        >
          {copy.deleteAction}
        </Button>
      )}
    </li>
  );
}

function RuleList({
  rules,
  remove,
}: {
  rules: readonly ApprovalRuleView[];
  remove?: (rule: ApprovalRuleView) => void;
}) {
  const { locale } = useI18n();
  const copy = approvalCopy[locale];
  if (rules.length === 0)
    return <p className="text-xs text-muted-foreground">{copy.noRules}</p>;
  return (
    <>
      <ul className="divide-y divide-border border-y border-border">
        {rules.map((rule) => (
          <RuleRow
            key={rule.id}
            rule={rule}
            remove={remove && (() => remove(rule))}
          />
        ))}
      </ul>
      {rules.some((rule) => !rule.effective) && (
        <Hint>{copy.disabledByPolicyHelp}</Hint>
      )}
    </>
  );
}

function AddRule({ tenant, added }: { tenant: string; added: () => void }) {
  const { locale } = useI18n();
  const copy = approvalCopy[locale];
  const id = useId();
  const [kind, setKind] = useState<ApprovalKind>("tool_call");
  const [every, setEvery] = useState(false);
  const [target, setTarget] = useState("");
  const [effect, setEffect] = useState<RuleEffect>("allow");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const submit = async () => {
    const draft = ruleInput({ kind, every, target, effect });
    if (!draft.ok) {
      setError(copy.targetRequired);
      return;
    }
    setBusy(true);
    setError("");
    try {
      await approvalRuleCreate(encodeURIComponent(tenant), draft.input);
      setTarget("");
      added();
    } catch (reason) {
      setError(message(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form
      className="grid min-w-0 gap-3 rounded-md border border-border bg-surface p-3"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <div className="grid min-w-0 gap-3 sm:grid-cols-2">
        <Field label={copy.kindLabel}>
          <NativeSelect
            value={kind}
            disabled={busy}
            onChange={(event) => {
              setKind(event.target.value as ApprovalKind);
              setTarget("");
              setError("");
            }}
          >
            {approvalKinds.map((value) => (
              <option key={value} value={value}>
                {copy.kinds[value].label}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <Field label={copy.effectLabel}>
          <NativeSelect
            value={effect}
            disabled={busy}
            onChange={(event) => setEffect(event.target.value as RuleEffect)}
          >
            {ruleEffects.map((value) => (
              <option key={value} value={value}>
                {copy.effects[value]}
              </option>
            ))}
          </NativeSelect>
        </Field>
      </div>
      <Hint>{copy.effectHelp[effect]}</Hint>
      <Check
        checked={every}
        disabled={busy}
        onChange={(event) => setEvery(event.target.checked)}
      >
        {copy.everyTargetOf(copy.kinds[kind].label)}
      </Check>
      {!every && (
        <>
          <Field label={copy.targetLabel}>
            <Input
              value={target}
              disabled={busy}
              placeholder={targetExamples[kind]}
              spellCheck={false}
              autoCapitalize="off"
              aria-describedby={`${id}-help`}
              className="font-mono"
              onChange={(event) => setTarget(event.target.value)}
            />
          </Field>
          <Hint id={`${id}-help`}>{copy.kinds[kind].help}</Hint>
        </>
      )}
      {error && <Alert>{error}</Alert>}
      <Button type="submit" disabled={busy} className="justify-self-start">
        {copy.addRule}
      </Button>
    </form>
  );
}

function Rules({ tenant }: { tenant: string }) {
  const { locale } = useI18n();
  const copy = approvalCopy[locale];
  const client = useQueryClient();
  const key = ["approval-rules", tenant] as const;
  const rules = useQuery({
    queryKey: key,
    queryFn: ({ signal }) =>
      approvalRules(encodeURIComponent(tenant), { signal }),
    retry: false,
  });
  const [error, setError] = useState("");
  const refresh = () => client.invalidateQueries({ queryKey: key });
  return (
    <Panel title={copy.rulesTitle}>
      <Hint>{copy.rulesHelp}</Hint>
      {rules.isError ? (
        <Alert retry={() => void rules.refetch()} retryLabel={copy.retry}>
          <p>{copy.loadFailed}</p>
          <p className="text-muted-foreground">{rules.error.message}</p>
        </Alert>
      ) : !rules.data ? (
        <Loading />
      ) : (
        <RuleList
          rules={rules.data.rules}
          remove={(rule) => {
            setError("");
            approvalRuleDelete(encodeURIComponent(tenant), rule.id)
              .catch((reason) => setError(message(reason)))
              .finally(() => void refresh());
          }}
        />
      )}
      {error && <Alert>{error}</Alert>}
      <AddRule tenant={tenant} added={() => void refresh()} />
    </Panel>
  );
}

/** Settings, Approvals: the signed-in person's own Approval Mode and Rules. */
export function ApprovalSettingsPage({ tenant }: { tenant: string }) {
  const { locale } = useI18n();
  const copy = approvalCopy[locale];
  return (
    <div className="grid max-w-3xl min-w-0 gap-6">
      <Notice>{copy.intro}</Notice>
      <DefaultMode tenant={tenant} />
      <Rules tenant={tenant} />
    </div>
  );
}

function MemberSettings({
  tenant,
  subject,
}: {
  tenant: string;
  subject: string;
}) {
  const { locale } = useI18n();
  const copy = approvalCopy[locale];
  const settings = useQuery({
    queryKey: ["approval-settings", tenant, subject],
    queryFn: ({ signal }) =>
      approvalSettings(
        encodeURIComponent(tenant),
        encodeURIComponent(subject),
        { signal },
      ),
    retry: false,
  });
  if (settings.isError) return <Alert>{settings.error.message}</Alert>;
  if (!settings.data) return <Loading>{copy.memberLoading}</Loading>;
  const { default_mode, bypass_accepted_at, rules } = settings.data;
  return (
    <>
      <Hint>{copy.memberReadOnly}</Hint>
      <Facts
        items={[
          [copy.memberMode, copy.modes[default_mode].label],
          [
            copy.modes.bypass.label,
            copy.memberBypass(bypass_accepted_at, (value) =>
              new Date(value).toLocaleString(locale),
            ),
          ],
        ]}
      />
      <RuleList rules={rules} />
    </>
  );
}

/** A member's Approval Mode and Rules, read only, for a Tenant Administrator
 *  holding `approval.read`. The member alone changes them. */
export function MemberApprovalSettings({
  tenant,
  subject,
}: {
  tenant: string;
  subject: string;
}) {
  const { locale } = useI18n();
  const [opened, setOpened] = useState(false);
  return (
    <Disclosure
      className="basis-full"
      summary={approvalCopy[locale].memberOpen}
      onToggle={(event) => setOpened(event.currentTarget.open)}
    >
      {opened && <MemberSettings tenant={tenant} subject={subject} />}
    </Disclosure>
  );
}
