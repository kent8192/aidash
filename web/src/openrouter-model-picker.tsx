import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "./components/ui/popover";
import { Alert, Disclosure, Hint, pairClass } from "./components/patterns";
import { cn } from "./lib/utils";
import { useId, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Check, ChevronsUpDown, Ellipsis, Star } from "lucide-react";
import { openrouterModels } from "./generated/aidash";
import type { CatalogModel } from "./generated/models";
import { Field, useI18n } from "./ui";
import { apiFetch } from "./transport";
import {
  FAVORITES_TAB,
  OTHER_TAB,
  VENDORS,
  effortLabel,
  effortOptions,
  favoritesKey,
  initialTab,
  modelsForTab,
  railTabs,
  readFavorites,
  searchModels,
  toggleFavorite,
  vendorMark,
  vendorName,
  vendorOf,
  vendorRail,
  writeFavorites,
} from "./model-picker-catalog";
import { SIMPLE_ICON_PATHS } from "./vendor-logos";

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

/** A bundled vendor logo, or a one-letter mark; never fetched (#1173). */
function VendorLogo({
  vendor,
  className,
}: {
  vendor: string | null;
  className?: string;
}) {
  const path = vendor ? SIMPLE_ICON_PATHS[VENDORS[vendor]?.icon ?? ""] : "";
  if (path)
    return (
      <svg
        viewBox="0 0 24 24"
        aria-hidden="true"
        fill="currentColor"
        className={cn("size-4 shrink-0", className)}
      >
        <path d={path} />
      </svg>
    );
  return (
    <span
      aria-hidden="true"
      className={cn(
        "grid size-4 shrink-0 place-items-center rounded-[4px] bg-muted text-[10px] font-semibold leading-none text-foreground",
        className,
      )}
    >
      {vendor ? vendorMark(vendor) : "?"}
    </span>
  );
}

/** Favorites per Tenant and Subject in `localStorage` (#1173). */
function useFavorites(tenant?: string, subject?: string) {
  const key = favoritesKey(tenant, subject);
  const storage = typeof window === "undefined" ? undefined : localStorage;
  const [stored, setStored] = useState(() => ({
    key,
    list: readFavorites(storage, key),
  }));
  if (stored.key !== key) setStored({ key, list: readFavorites(storage, key) });
  const favorites = stored.key === key ? stored.list : [];
  const toggle = (slug: string) => {
    const list = toggleFavorite(favorites, slug);
    writeFavorites(storage, key, list);
    setStored({ key, list });
  };
  return [favorites, toggle] as const;
}

export function OpenRouterModelPicker({
  onNameChange,
  tenant,
  subject,
}: {
  onNameChange?: (name: string) => void;
  /** Only a Tenant Administrator can read the level; others see no refusals. */
  tenant?: string;
  /** Scopes the favorites with `tenant`; absent outside a Subject session. */
  subject?: string;
}) {
  const { t, locale } = useI18n();
  const listId = useId();
  const effortLabelId = useId();
  const searchRef = useRef<HTMLInputElement>(null);
  const [search, setSearch] = useState("");
  const [selectedId, setSelectedId] = useState("");
  const [effort, setEffort] = useState("");
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState(FAVORITES_TAB);
  const [active, setActive] = useState(0);
  const [favorites, toggleFavoriteSlug] = useFavorites(tenant, subject);
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
  const level = protection.data?.effective.level;
  const models = useMemo(() => catalog.data?.models ?? [], [catalog.data]);
  const rail = useMemo(() => vendorRail(models), [models]);
  const tabs = railTabs(rail);
  const selected = models.find((m) => m.id === selectedId);
  const efforts = effortOptions(selected?.reasoning);
  const query = search.trim();
  // Text searches every vendor; clearing it returns to the tab.
  const matches = query
    ? searchModels(models, query)
    : modelsForTab(models, tab, rail, favorites);
  const suggestName = (model: CatalogModel, effort = "") => {
    const level =
      effort ||
      model.reasoning?.default_effort ||
      (model.reasoning?.supported_efforts?.length ? "default" : "none");
    const id = model.id.includes("/") ? model.id : `openrouter/${model.id}`;
    onNameChange?.(
      `${id}-${level}`.toLowerCase().replace(/[^a-z0-9._-]+/g, "-"),
    );
  };
  /** Selects a model, keeping the picker open so an effort can follow. */
  const select = (model: CatalogModel) => {
    if (zdrRefusal(model.zero_data_retention, level)) return false;
    if (model.id !== selectedId) {
      setSelectedId(model.id);
      setEffort("");
      suggestName(model);
    }
    return true;
  };
  const chooseEffort = (value: string) => {
    setEffort(value);
    if (selected) suggestName(selected, value);
  };
  const openPicker = (next: boolean) => {
    setOpen(next);
    if (!next) return;
    const start = initialTab(models, rail, favorites);
    setTab(start);
    setSearch("");
    setActive(
      Math.max(
        0,
        modelsForTab(models, start, rail, favorites).findIndex(
          (m) => m.id === selectedId,
        ),
      ),
    );
  };
  const switchTab = (next: string) => {
    setTab(next);
    setActive(0);
  };
  const tabName = (id: string) =>
    id === FAVORITES_TAB
      ? t("modelFavorites")
      : id === OTHER_TAB
        ? t("modelVendorOther")
        : vendorName(id);
  const perMillion = (price: string | undefined) => {
    if (price === undefined || price.trim() === "") return null;
    const value = Number(price);
    return Number.isFinite(value) && value >= 0 ? value * 1_000_000 : null;
  };
  const selectedVendor = selected ? vendorOf(selected.id) : null;
  const defaultEffort = selected?.reasoning?.default_effort;
  return (
    <>
      <Popover open={open} onOpenChange={openPicker}>
        <Field label={t("modelId")}>
          <PopoverTrigger asChild>
            <button
              type="button"
              aria-haspopup="dialog"
              className="flex h-8 w-full min-w-0 cursor-pointer items-center gap-2 rounded-md border border-input bg-surface px-2.5 text-left text-[13px] text-foreground transition-colors hover:border-edge focus-visible:border-brand-line focus-visible:outline-2 focus-visible:outline-offset-0 focus-visible:outline-brand-line"
            >
              {selected ? (
                <>
                  <VendorLogo vendor={selectedVendor} />
                  <span className="min-w-0 truncate font-medium">
                    {selected.name}
                  </span>
                  {efforts.length > 0 && (
                    <span className="shrink-0 text-xs text-muted-foreground">
                      · {effort ? effortLabel(effort) : t("modelEffortDefault")}
                    </span>
                  )}
                </>
              ) : (
                <span className="min-w-0 truncate text-faint">
                  {t("modelPickerPlaceholder")}
                </span>
              )}
              <ChevronsUpDown
                aria-hidden="true"
                className="ml-auto size-3.5 shrink-0 text-muted-foreground"
              />
            </button>
          </PopoverTrigger>
        </Field>
        <PopoverContent
          aria-label={t("modelId")}
          className="flex w-[min(360px,calc(100vw-1rem))] flex-col overflow-hidden p-0"
          collisionPadding={8}
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            searchRef.current?.focus();
          }}
        >
          <div className="border-b border-border p-2">
            <Input
              ref={searchRef}
              role="combobox"
              aria-label={t("modelSearch")}
              aria-autocomplete="list"
              aria-expanded
              aria-controls={listId}
              aria-activedescendant={
                matches[active] ? `${listId}-${active}` : undefined
              }
              placeholder={t("modelSearch")}
              autoComplete="off"
              value={search}
              onChange={(e) => {
                setSearch(e.target.value);
                setActive(0);
              }}
              onKeyDown={(e) => {
                if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  e.preventDefault();
                  setActive((i) =>
                    Math.max(
                      0,
                      Math.min(
                        matches.length - 1,
                        i + (e.key === "ArrowDown" ? 1 : -1),
                      ),
                    ),
                  );
                } else if (
                  (e.key === "ArrowLeft" || e.key === "ArrowRight") &&
                  !search
                ) {
                  // Tabs only matter without text, so the caret keeps ←/→ then.
                  e.preventDefault();
                  const at = tabs.indexOf(tab);
                  const step = e.key === "ArrowRight" ? 1 : -1;
                  switchTab(tabs[(at + step + tabs.length) % tabs.length]);
                } else if (e.key === "Enter") {
                  e.preventDefault();
                  if (matches[active] && select(matches[active]))
                    setOpen(false);
                }
              }}
            />
          </div>
          <div className="flex h-72 min-h-0">
            <div
              role="tablist"
              aria-orientation="vertical"
              aria-label={t("modelVendors")}
              className="flex w-11 shrink-0 flex-col items-center gap-0.5 overflow-y-auto border-r border-border py-1.5"
            >
              {tabs.map((id) => (
                <button
                  key={id}
                  type="button"
                  role="tab"
                  tabIndex={-1}
                  aria-selected={!query && id === tab}
                  aria-controls={listId}
                  aria-label={tabName(id)}
                  title={tabName(id)}
                  onClick={() => {
                    setSearch("");
                    switchTab(id);
                    searchRef.current?.focus();
                  }}
                  className="grid size-8 shrink-0 cursor-pointer place-items-center rounded-md text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground aria-selected:bg-brand-soft aria-selected:text-foreground"
                >
                  {id === FAVORITES_TAB ? (
                    <Star aria-hidden="true" className="size-4" />
                  ) : id === OTHER_TAB ? (
                    <Ellipsis aria-hidden="true" className="size-4" />
                  ) : (
                    <VendorLogo vendor={id} />
                  )}
                </button>
              ))}
            </div>
            <div
              role="listbox"
              id={listId}
              aria-label={query ? t("modelSearch") : tabName(tab)}
              className="grid min-w-0 flex-1 content-start overflow-y-auto overscroll-contain p-1"
            >
              {matches.map((model, index) => {
                const refusal = zdrRefusal(model.zero_data_retention, level);
                const favorite = favorites.includes(model.id);
                return (
                  <div
                    key={model.id}
                    className={cn(
                      "flex min-w-0 items-center rounded-md transition-colors duration-150 hover:bg-accent",
                      index === active && "bg-brand-soft hover:bg-brand-soft",
                    )}
                  >
                    <button
                      type="button"
                      role="option"
                      id={`${listId}-${index}`}
                      aria-selected={index === active}
                      aria-disabled={refusal !== null}
                      tabIndex={-1}
                      ref={(element) => {
                        if (index === active)
                          element?.scrollIntoView?.({ block: "nearest" });
                      }}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => {
                        setActive(index);
                        select(model);
                      }}
                      className="flex min-w-0 flex-1 cursor-pointer items-start gap-2 px-2 py-1.5 text-left aria-disabled:cursor-not-allowed"
                    >
                      <VendorLogo
                        vendor={vendorOf(model.id)}
                        className="mt-0.5 text-muted-foreground"
                      />
                      <span className="grid min-w-0 flex-1 gap-0.5">
                        <span className="flex min-w-0 items-center gap-1 text-[13px] font-medium text-foreground">
                          <span className="truncate">{model.name}</span>
                          {model.id === selectedId && (
                            <Check
                              aria-hidden="true"
                              className="size-3.5 shrink-0 text-brand"
                            />
                          )}
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
                      </span>
                    </button>
                    <button
                      type="button"
                      tabIndex={-1}
                      aria-pressed={favorite}
                      aria-label={`${t(
                        favorite ? "modelFavoriteRemove" : "modelFavoriteAdd",
                      )}: ${model.name}`}
                      title={t(
                        favorite ? "modelFavoriteRemove" : "modelFavoriteAdd",
                      )}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => toggleFavoriteSlug(model.id)}
                      className={cn(
                        "mr-1 grid size-6 shrink-0 cursor-pointer place-items-center rounded-sm text-[13px] leading-none transition-colors hover:bg-accent hover:text-foreground",
                        favorite ? "text-warning" : "text-faint",
                      )}
                    >
                      {favorite ? "★" : "☆"}
                    </button>
                  </div>
                );
              })}
              {!catalog.isPending &&
                !catalog.isError &&
                matches.length === 0 && (
                  <p
                    role="status"
                    className="px-2.5 py-2 text-xs text-muted-foreground"
                  >
                    {!query && tab === FAVORITES_TAB
                      ? t("modelFavoritesEmpty")
                      : t("modelNoMatches")}
                  </p>
                )}
            </div>
          </div>
          {selected && efforts.length > 0 && (
            <div className="grid gap-1.5 border-t border-border p-2">
              <div className="flex min-w-0 items-baseline justify-between gap-2 text-[11px] text-muted-foreground">
                <span id={effortLabelId} className="font-medium">
                  {t("modelReasoningEffort")}
                </span>
                {defaultEffort && (
                  <span className="truncate">
                    {t("modelEffortDefaultIs")} {effortLabel(defaultEffort)}
                  </span>
                )}
              </div>
              <div
                role="radiogroup"
                aria-labelledby={effortLabelId}
                className="flex flex-wrap gap-0.5 rounded-md bg-muted p-0.5"
              >
                {["", ...efforts].map((value) => (
                  <button
                    key={value || "default"}
                    type="button"
                    role="radio"
                    aria-checked={effort === value}
                    onClick={() => chooseEffort(value)}
                    className="flex-auto cursor-pointer whitespace-nowrap rounded-sm px-1.5 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:text-foreground aria-checked:bg-surface aria-checked:text-foreground aria-checked:shadow-sm"
                  >
                    {value ? effortLabel(value) : t("modelEffortDefault")}
                  </button>
                ))}
              </div>
            </div>
          )}
        </PopoverContent>
      </Popover>
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
      {/* Chosen in the picker; empty is Default and omits the effort. */}
      <input type="hidden" name="reasoning_effort" value={effort} />
      {/* Named explicitly, never inferred from the slug (ADR 0017). */}
      <Field label={t("modelTokenEncoding")}>
        <NativeSelect name="token_encoding" key={selectedId} defaultValue="">
          <option value="">{t("modelTokenEncodingNone")}</option>
          {TOKEN_ENCODINGS.map((encoding) => (
            <option key={encoding} value={encoding}>
              {encoding}
            </option>
          ))}
        </NativeSelect>
      </Field>
      <Hint>{t("modelTokenEncodingHelp")}</Hint>
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
