import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { Alert, Disclosure, Hint, pairClass } from "./components/patterns";
import { useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { openrouterModels } from "./generated/aidash";
import { Field, useI18n } from "./ui";

export function OpenRouterModelPicker({
  onNameChange,
}: {
  onNameChange?: (name: string) => void;
}) {
  const { t, locale } = useI18n();
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
      <div className="relative min-w-0">
        <Field label={t("modelId")}>
          <Input
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
            className="absolute inset-x-0 top-full z-20 mt-1 grid max-h-64 overflow-y-auto rounded-lg border border-border-strong bg-popover p-1 shadow-overlay"
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
                className="grid min-w-0 cursor-pointer gap-0.5 rounded-md px-2.5 py-1.5 text-left transition-colors duration-150 hover:bg-accent aria-selected:bg-brand-soft"
              >
                <span className="truncate text-[13px] font-medium text-foreground">
                  {model.name}
                </span>
                <span className="truncate font-mono text-[11px] text-muted-foreground">
                  {model.id}
                </span>
              </button>
            ))}
            {!catalog.isPending && !catalog.isError && matches.length === 0 && (
              <p
                role="status"
                className="px-2.5 py-2 text-xs text-muted-foreground"
              >
                {t("modelNoMatches")}
              </p>
            )}
          </div>
        )}
      </div>
      {catalog.isPending && <Hint role="status">{t("modelLoading")}</Hint>}
      {catalog.isError && (
        <Alert retry={() => void catalog.refetch()}>
          {t("modelLoadError")}
        </Alert>
      )}
      <Hint>{t("modelCatalogHelp")}</Hint>
      <Hint>{t("modelZdr")}</Hint>
      <Field label="Reasoning Effort">
        <NativeSelect
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
        </NativeSelect>
      </Field>
      <input type="hidden" name="model_id" value={selected?.id ?? ""} />
      <input
        type="hidden"
        name="modalities"
        value={JSON.stringify(selected?.architecture.input_modalities ?? [])}
      />
      {selected?.architecture.input_modalities.some(
        (modality) => modality === "image" || modality === "audio",
      ) && (
        <Disclosure
          summary={
            locale === "ja-JP"
              ? "確認済みメディア経路"
              : "Verified media routes"
          }
        >
          <Hint>
            {locale === "ja-JP"
              ? "画像・音声を使うには、経路ごとの対応形式と確認根拠、有効期限を登録してください。期限切れや未確認の経路では送信しません。"
              : "To use image or audio input, register the formats, evidence, and expiry for each provider route. Unverified or expired routes cannot receive media."}
          </Hint>
          <Textarea
            key={selectedId}
            name="media_routes"
            aria-label={
              locale === "ja-JP"
                ? "メディア経路の確認根拠 JSON"
                : "Media route evidence JSON"
            }
            rows={5}
            defaultValue="[]"
            spellCheck={false}
            className="font-mono text-xs"
          />
        </Disclosure>
      )}
      <input
        type="hidden"
        name="endpoint"
        value="https://openrouter.ai/api/v1"
      />
      <div className={pairClass}>
        <Field label={t("contextWindow")}>
          <Input
            name="context_window"
            readOnly
            value={selected?.context_length ?? ""}
            className="bg-raised font-mono text-xs"
          />
        </Field>
        <Field label={t("modelMaxOutputTokens")}>
          <Input
            name="max_output_tokens"
            type="number"
            readOnly
            value={selected?.top_provider?.max_completion_tokens ?? ""}
            className="bg-raised font-mono text-xs"
          />
        </Field>
      </div>
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
        <Hint className="font-mono tabular">
          {t("modelPricing")} {perMillion(selected.pricing.prompt) ?? "—"} /{" "}
          {perMillion(selected.pricing.completion) ?? "—"}
        </Hint>
      )}
    </>
  );
}
