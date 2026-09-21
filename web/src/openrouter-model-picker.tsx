import { useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { openrouterModels } from "./generated/aidash";
import { Field, useI18n } from "./ui";

export function OpenRouterModelPicker({
  onNameChange,
}: {
  onNameChange?: (name: string) => void;
}) {
  const { t } = useI18n();
  const listId = useId();
  const [search, setSearch] = useState("");
  const [selectedId, setSelectedId] = useState("");
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const catalog = useQuery({
    queryKey: ["openrouter-models"],
    queryFn: () => openrouterModels(),
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
  const selected = catalog.data?.find((m) => m.id === selectedId);
  const effortLevels = (selected?.reasoning?.supported_efforts ?? []).filter(
    (effort) =>
      ["max", "xhigh", "high", "medium", "low", "minimal", "none"].includes(
        effort,
      ) && !(selected?.reasoning?.mandatory && effort === "none"),
  );
  const query = selected ? "" : search.trim().toLowerCase();
  const matches = (catalog.data ?? []).filter(
    (m) => !query || `${m.name} ${m.id}`.toLowerCase().includes(query),
  );
  const suggestName = (
    model: NonNullable<typeof catalog.data>[number],
    effort = "",
  ) => {
    const level =
      effort ||
      model.reasoning?.default_effort ||
      (model.reasoning?.supported_efforts?.length ? "default" : "none");
    const id = model.id.includes("/") ? model.id : `openrouter/${model.id}`;
    onNameChange?.(
      `${id}-${level}`.toLowerCase().replace(/[^a-z0-9._-]+/g, "-"),
    );
  };
  const choose = (model: NonNullable<typeof catalog.data>[number]) => {
    setSelectedId(model.id);
    setSearch(`${model.name} · ${model.id}`);
    setOpen(false);
    suggestName(model);
  };
  const perMillion = (price: string | undefined) => {
    if (price === undefined || price.trim() === "") return null;
    const value = Number(price);
    return Number.isFinite(value) && value >= 0 ? value * 1_000_000 : null;
  };
  return (
    <>
      <div className="model-picker">
        <Field label={t("modelId")}>
          <input
            role="combobox"
            aria-autocomplete="list"
            aria-expanded={open}
            aria-controls={listId}
            aria-activedescendant={
              open && matches[active] ? `${listId}-${active}` : undefined
            }
            placeholder={t("modelSearch")}
            autoComplete="off"
            required
            value={search}
            onFocus={() => setOpen(true)}
            onBlur={() => setOpen(false)}
            onChange={(e) => {
              setSearch(e.target.value);
              setSelectedId("");
              onNameChange?.("");
              setActive(0);
              setOpen(true);
            }}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                setOpen(true);
                setActive((i) =>
                  Math.max(
                    0,
                    Math.min(
                      matches.length - 1,
                      i + (e.key === "ArrowDown" ? 1 : -1),
                    ),
                  ),
                );
              } else if (e.key === "Enter" && open) {
                e.preventDefault();
                if (matches[active]) choose(matches[active]);
              } else if (e.key === "Escape" && open) {
                e.stopPropagation();
                setOpen(false);
              }
            }}
          />
        </Field>
        {open && (
          <div
            className="model-options"
            role="listbox"
            id={listId}
            aria-label={t("modelId")}
          >
            {matches.map((model, index) => (
              <button
                type="button"
                role="option"
                id={`${listId}-${index}`}
                key={model.id}
                aria-selected={index === active}
                tabIndex={-1}
                ref={(element) => {
                  if (index === active)
                    element?.scrollIntoView({ block: "nearest" });
                }}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => choose(model)}
              >
                <strong>{model.name}</strong>
                <span>{model.id}</span>
              </button>
            ))}
            {!catalog.isPending && !catalog.isError && matches.length === 0 && (
              <p role="status">{t("modelNoMatches")}</p>
            )}
          </div>
        )}
      </div>
      {catalog.isPending && <p role="status">{t("modelLoading")}</p>}
      {catalog.isError && (
        <div role="alert">
          <p>{t("modelLoadError")}</p>
          <button type="button" onClick={() => void catalog.refetch()}>
            {t("retry")}
          </button>
        </div>
      )}
      <p className="muted">{t("modelCatalogHelp")}</p>
      <p className="muted">{t("modelZdr")}</p>
      <Field label="Reasoning Effort">
        <select
          name="reasoning_effort"
          key={selectedId}
          defaultValue=""
          disabled={effortLevels.length === 0}
          onChange={(event) => {
            if (selected) suggestName(selected, event.target.value);
          }}
        >
          <option value="">
            {effortLevels.length
              ? t("modelReasoningDefault")
              : t("modelReasoningUnavailable")}
          </option>
          {effortLevels.map((effort) => (
            <option key={effort} value={effort}>
              {effort}
            </option>
          ))}
        </select>
      </Field>
      <input type="hidden" name="model_id" value={selected?.id ?? ""} />
      <input
        type="hidden"
        name="endpoint"
        value="https://openrouter.ai/api/v1"
      />
      <Field label={t("contextWindow")}>
        <input
          name="context_window"
          readOnly
          value={selected?.context_length ?? ""}
        />
      </Field>
      <input
        type="hidden"
        name="cost"
        value={JSON.stringify({
          input_per_million: perMillion(selected?.pricing.prompt),
          output_per_million: perMillion(selected?.pricing.completion),
          currency: "USD",
        })}
      />
      {selected && (
        <p className="muted">
          {t("modelPricing")} {perMillion(selected.pricing.prompt) ?? "—"} /{" "}
          {perMillion(selected.pricing.completion) ?? "—"}
        </p>
      )}
    </>
  );
}
