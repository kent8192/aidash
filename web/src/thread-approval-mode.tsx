import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { NativeSelect } from "./components/ui/native-select";
import {
  threadApprovalMode,
  threadApprovalModeUpdate,
} from "./generated/aidash";
import type { ApprovalMode } from "./generated/models";
import { useApprovalPreferences } from "./approval-settings";
import { approvalCopy } from "./approval-settings-copy";
import { threadModeOptions, threadModeUpdate } from "./approval-settings-model";
import { useI18n } from "./ui";

const message = (reason: unknown) =>
  reason instanceof Error ? reason.message : String(reason);

/**
 * The signed-in person's Approval Mode for one Thread, beside the composer
 * where they decide what their next instruction may do. It shows the mode in
 * effect, either the Thread's own or `Default: <mode>`, and sets or clears the
 * Thread's mode. A running Run of theirs in the Thread follows it at its next
 * approval.
 */
export function ThreadApprovalMode({
  tenant,
  workspace,
  thread,
}: {
  tenant: string;
  workspace: string;
  thread: string;
}) {
  const { locale } = useI18n();
  const copy = approvalCopy[locale];
  const client = useQueryClient();
  const key = ["thread-approval-mode", workspace, thread] as const;
  const view = useQuery({
    queryKey: key,
    queryFn: ({ signal }) => threadApprovalMode(workspace, thread, { signal }),
    retry: false,
  });
  const preferences = useApprovalPreferences(tenant);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  if (view.isError)
    return (
      <span
        className="min-w-0 truncate px-1 text-[11px] text-faint"
        title={view.error.message}
      >
        {copy.threadLoadFailed}
      </span>
    );
  if (!view.data) return null;
  const { selected, options } = threadModeOptions(
    view.data,
    // Unknown until the preferences load: the server refuses Bypass until the
    // person accepted it, so the option is not withheld on a guess.
    preferences.data ? preferences.data.bypass_accepted_at !== null : true,
  );
  const label = (mode: ApprovalMode) => copy.modes[mode].label;
  const text = ({ value, mode, unavailable }: (typeof options)[number]) =>
    value === "default"
      ? copy.threadDefault(label(mode))
      : unavailable
        ? `${label(mode)} (${copy.threadUnavailable[unavailable]})`
        : label(mode);
  const choose = async (value: typeof selected) => {
    setBusy(true);
    setError("");
    try {
      client.setQueryData(
        key,
        await threadApprovalModeUpdate(
          workspace,
          thread,
          threadModeUpdate(value),
        ),
      );
    } catch (reason) {
      setError(message(reason));
      await client.invalidateQueries({ queryKey: key });
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <NativeSelect
        aria-label={copy.threadSwitcher}
        title={text(options.find((option) => option.value === selected)!)}
        value={selected}
        disabled={busy}
        wrapperClassName="ml-1 w-auto min-w-0 max-w-[11rem] shrink"
        className="h-7 pl-2 pr-6 text-xs"
        onChange={(event) => void choose(event.target.value as typeof selected)}
      >
        {options.map((option) => (
          <option
            key={option.value}
            value={option.value}
            disabled={option.unavailable !== null && option.value !== selected}
          >
            {text(option)}
          </option>
        ))}
      </NativeSelect>
      {error && (
        <span
          role="alert"
          className="min-w-0 max-w-[14rem] truncate px-1 text-[11px] text-destructive"
          title={error}
        >
          {copy.threadFailed} {error}
        </span>
      )}
    </>
  );
}
