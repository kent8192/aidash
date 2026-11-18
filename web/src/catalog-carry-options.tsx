import { useI18n } from "./ui";

export type ApprovalScope = "node" | "deployment";

/** Choices made by the approver for the selected Catalog Approvals. */
export function CatalogCarryOptions({
  carry,
  setCarry,
  scope,
  setScope,
  disabled,
}: {
  carry: boolean;
  setCarry: (value: boolean) => void;
  scope: ApprovalScope;
  setScope: (value: ApprovalScope) => void;
  disabled: boolean;
}) {
  const { locale } = useI18n();
  const japanese = locale === "ja-JP";
  return (
    <fieldset disabled={disabled} className="grid gap-2 text-xs">
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={carry}
          onChange={(event) => setCarry(event.target.checked)}
        />
        {japanese
          ? "後のバージョンにも承認を引き継ぐ"
          : "Carry approval to later versions"}
      </label>
      <label className="grid gap-1">
        {japanese ? "承認の範囲" : "Approval scope"}
        <select
          value={scope}
          onChange={(event) => setScope(event.target.value as ApprovalScope)}
          className="rounded border border-border bg-background px-2 py-1"
        >
          <option value="deployment">
            {japanese
              ? "この Deployment のすべての Node"
              : "Every Node on this Deployment"}
          </option>
          <option value="node">
            {japanese ? "この Node のみ" : "This Node only"}
          </option>
        </select>
      </label>
    </fieldset>
  );
}
