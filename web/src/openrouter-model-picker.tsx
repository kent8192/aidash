import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { Alert, Disclosure, Hint, pairClass } from "./components/patterns";
import { Fragment, useCallback, useEffect, useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { openrouterModels } from "./generated/aidash";
import type { CatalogModel } from "./generated/models";
import {
  allowedEfforts,
  defaultEffort,
  efforts,
  perMillion,
  type Effort,
  type ModelChoice,
} from "./task-model";
import { catalogSections, compareModels, eligibleModel } from "./model-order";
import type { Entry } from "./types";
import { Field, useI18n } from "./ui";
import { apiFetch } from "./transport";

type Level =
  | "no_restriction"
  | "no_training"
  | "safety_retention"
  | "zero_data_retention";

/** Whether a call held to `level` can use a model (#963), or why not. */
export function zdrRefusal(
  routes: CatalogModel["zero_data_retention"],
  level: Level | undefined,
): "safety_retention_only" | "no_zdr_route" | null {
  if (!level || !routes) return null;
  if (level === "zero_data_retention" && routes !== "strict")
    return routes === "safety_retention_only"
      ? "safety_retention_only"
      : "no_zdr_route";
  if (level === "safety_retention" && routes === "none") return "no_zdr_route";
  return null;
}

/** The server's closed list of Token Encodings. */
export const TOKEN_ENCODINGS = ["o200k_base", "gemma4"] as const;

export function ReasoningSlider({
  levels,
  value,
  onChange,
}: {
  levels: Effort[];
  value?: Effort;
  onChange: (v: Effort) => void;
}) {
  const { locale } = useI18n();
  if (!levels.length) return null;
  const label = locale === "ja-JP" ? "推論強度" : "Reasoning Effort";
  return (
    <>
      <Field label={label}>
        <input
          type="range"
          aria-label={label}
          aria-valuetext={value}
          min={0}
          max={levels.length - 1}
          value={Math.max(0, levels.indexOf(value ?? "medium"))}
          onChange={(e) => onChange(levels[Number(e.target.value)])}
        />
      </Field>
      <div className="flex justify-between gap-1">
        {efforts.map((effort) => (
          <button
            type="button"
            key={effort}
            disabled={!levels.includes(effort)}
            aria-pressed={value === effort}
            className="text-xs text-muted-foreground aria-pressed:font-semibold aria-pressed:text-foreground disabled:opacity-40"
            onClick={() => onChange(effort)}
          >
            {effort}
          </button>
        ))}
      </div>
      <Hint>
        {locale === "ja-JP"
          ? "強くするほど丁寧に考えますが、遅く・高くなります"
          : "Higher effort reasons more carefully, takes longer and costs more."}
      </Hint>
      <input type="hidden" name="reasoning_effort" value={value ?? ""} />
    </>
  );
}

export function OpenRouterModelPicker({
  onNameChange,
  tenant,
  entries = [],
  onChoice,
  value,
}: {
  onNameChange?: (name: string) => void;
  /** Only a Tenant Administrator can read the level; others see no refusals. */
  tenant?: string;
  entries?: Entry[];
  onChoice?: (choice: ModelChoice | undefined) => void;
  value?: ModelChoice;
}) {
  const { t, locale, local } = useI18n();
  const listId = useId();
  const [search, setSearch] = useState("");
  const [localChoice, setLocalChoice] = useState<ModelChoice>();
  const choice = onChoice ? value : localChoice;
  const selectedId = choice?.catalog?.id ?? choice?.entry?.id ?? "";
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const catalog = useQuery({
    queryKey: ["openrouter-models"],
    queryFn: () => openrouterModels(),
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
  const protection = useQuery({
    queryKey: ["data-protection", tenant],
    enabled: Boolean(tenant),
    queryFn: () =>
      apiFetch<{ effective: { level: Level } }>(
        `/api/tenants/${encodeURIComponent(tenant ?? "")}/data-protection`,
      ),
    retry: false,
  });
  const level = protection.data?.effective?.level;
  const models = catalog.data?.models ?? [];
  const ja = locale === "ja-JP";
  const { all, shortcuts } = catalogSections(models);
  const registered = entries
    .filter((e) => e.kind === "model")
    .flatMap((entry) => {
      const id = String(entry.config?.model_id ?? entry.id);
      const routed = entry.config?.provider_credential === "openrouter";
      const catalog = routed ? models.find((m) => m.id === id) : undefined;
      if (
        (!onChoice && (!routed || !catalog)) ||
        !eligibleModel(catalog ?? { id })
      )
        return [];
      return [
        {
          id,
          name: local(entry.name) || entry.id,
          catalog,
          created: (
            catalog as (CatalogModel & { created?: number }) | undefined
          )?.created,
          entry,
        },
      ];
    })
    .sort(compareModels);
  const selectedEntry = registered.find(
    (m) =>
      m.entry.id === choice?.entry?.id &&
      m.entry.version === choice?.entry?.version,
  );
  const selected =
    selectedEntry?.catalog ??
    models.find((m) => m.id === choice?.catalog?.id) ??
    choice?.catalog;
  const effortLevels = allowedEfforts(selected);
  const lookup = (m: CatalogModel) => ({
    id: m.id,
    name: m.name,
    catalog: m,
    entry: registered.find((r) => r.catalog?.id === m.id)?.entry,
  });
  const query = search.trim().toLowerCase();
  const matches = [
    {
      label: ja ? "最新世代" : "Latest generation",
      items: shortcuts.map(lookup),
    },
    { label: ja ? "Tenant の登録モデル" : "Tenant models", items: registered },
    { label: "OpenRouter", items: all.map(lookup) },
  ].flatMap((section) =>
    section.items
      .filter(
        (m) =>
          choice ||
          !query ||
          (m.id + " " + m.name).toLowerCase().includes(query),
      )
      .map((m, i) => ({ ...m, heading: i === 0 ? section.label : undefined })),
  );
  const update = useCallback(
    (next: ModelChoice | undefined) => {
      if (onChoice) onChoice(next);
      else setLocalChoice(next);
      onNameChange?.(
        next?.catalog
          ? (next.catalog.id + "-" + (next.effort ?? "none"))
              .toLowerCase()
              .replace(/[^a-z0-9._-]+/g, "-")
          : "",
      );
    },
    [onChoice, onNameChange],
  );
  const refused = zdrRefusal(selected?.zero_data_retention, level);
  const effectiveEffort = defaultEffort(
    effortLevels,
    choice?.effort ?? selectedEntry?.entry.config.reasoning_effort,
  );
  useEffect(() => {
    if (!choice || !onChoice) return;
    if (refused) onChoice(undefined);
    else if (selected && effectiveEffort !== choice.effort)
      onChoice({ ...choice, effort: effectiveEffort });
  }, [choice, onChoice, effectiveEffort, refused, selected]);
  const choose = (item: (typeof matches)[number]) => {
    if (zdrRefusal(item.catalog?.zero_data_retention, level)) return;
    const allowed = allowedEfforts(item.catalog);
    const preferred = item.entry?.config.reasoning_effort;
    const effort = defaultEffort(allowed, preferred);
    update({
      catalog: item.catalog,
      entry: item.entry,
      effort,
      registrationKey: crypto.randomUUID(),
    });
    setSearch(item.name + " · " + item.id);
    setOpen(false);
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
            value={
              search ||
              (choice && selectedEntry
                ? selectedEntry.name + " · " + selectedEntry.id
                : "")
            }
            onFocus={() => setOpen(true)}
            onBlur={() => setOpen(false)}
            onChange={(e) => {
              setSearch(e.target.value);
              update(undefined);
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
            {matches.map((model, index) => {
              const refusal = zdrRefusal(
                model.catalog?.zero_data_retention,
                level,
              );
              return (
                <Fragment key={index}>
                  {model.heading && (
                    <span className="px-2 py-1 text-xs font-semibold">
                      {model.heading}
                    </span>
                  )}
                  <button
                    type="button"
                    role="option"
                    id={`${listId}-${index}`}
                    key={model.id}
                    aria-selected={index === active}
                    aria-disabled={refusal !== null}
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
                      {model.entry ? " · " + model.entry.version : ""}
                    </span>
                    <span className="truncate font-mono text-[11px] text-muted-foreground">
                      {model.id}
                    </span>
                    {refusal && (
                      <span className="text-[11px] text-destructive">
                        {t(
                          refusal === "safety_retention_only"
                            ? "modelSafetyRetentionOnly"
                            : "modelNoZdrRoute",
                        )}
                      </span>
                    )}
                  </button>
                </Fragment>
              );
            })}
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
      {catalog.data?.stale && (
        <Hint role="status">
          {t("modelCatalogStale")}{" "}
          <time className="font-mono" dateTime={catalog.data.fetched_at}>
            {new Date(catalog.data.fetched_at).toLocaleString(locale)}
          </time>
        </Hint>
      )}
      <Hint>{t("modelCatalogHelp")}</Hint>
      <Hint>{t("modelZdr")}</Hint>
      <ReasoningSlider
        levels={effortLevels}
        value={effectiveEffort}
        onChange={(effort) => choice && update({ ...choice, effort })}
      />
      {!onChoice && (
        <>
          {/* Only an OpenRouter Provider Credential model reaches this picker. */}
          <Field label={t("modelWireApi")}>
            <NativeSelect name="wire_api" defaultValue="chat">
              <option value="chat">{t("modelWireApiChat")}</option>
              <option value="responses">{t("modelWireApiResponses")}</option>
            </NativeSelect>
          </Field>
          <Hint>{t("modelWireApiHelp")}</Hint>
          <Field label={t("modelPromptCache")}>
            <NativeSelect name="cache_mode" defaultValue="">
              <option value="">{t("promptCacheDefault")}</option>
              <option value="none">{t("promptCacheOff")}</option>
            </NativeSelect>
          </Field>
          <Hint>{t("modelPromptCacheHelp")}</Hint>
          {/* Named explicitly, never inferred from the slug (ADR 0017). */}
          <Field label={t("modelTokenEncoding")}>
            <NativeSelect
              name="token_encoding"
              key={selectedId}
              defaultValue=""
            >
              <option value="">{t("modelTokenEncodingNone")}</option>
              {TOKEN_ENCODINGS.map((encoding) => (
                <option key={encoding} value={encoding}>
                  {encoding}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Hint>{t("modelTokenEncodingHelp")}</Hint>
          <input
            type="hidden"
            name="model_id"
            value={refused ? "" : (selected?.id ?? "")}
          />
          <input
            type="hidden"
            name="modalities"
            value={JSON.stringify(
              selected?.architecture.input_modalities ?? [],
            )}
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
      )}
    </>
  );
}
