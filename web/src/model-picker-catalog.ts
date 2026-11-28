// Pure rules behind the OpenRouter model picker (#1173): vendor rail,
// search, reasoning effort choices and favorites storage.
import type { CatalogModel, ReasoningOptions } from "./generated/models";

/** Rail tab ids for ★ and Other; a vendor slug never contains `/`. */
export const FAVORITES_TAB = "/favorites";
export const OTHER_TAB = "/other";

/** How many vendors get their own rail tab before "Other". */
export const TOP_VENDORS = 10;

/** The vendor a catalog slug belongs to: the prefix before `/`. */
export function vendorOf(id: string): string | null {
  const slash = id.indexOf("/");
  if (slash <= 0) return null;
  // `~openai/...` aliases belong with `openai/...`.
  return id.slice(0, slash).replace(/^~+/, "").toLowerCase() || null;
}

/**
 * Display names for the vendors most often in the OpenRouter catalog, and
 * the bundled Simple Icons logo each uses. A vendor without an icon, or not
 * listed, shows a one-letter mark.
 */
export const VENDORS: Readonly<
  Record<string, { name: string; icon?: string }>
> = {
  openai: { name: "OpenAI" },
  anthropic: { name: "Anthropic", icon: "anthropic" },
  google: { name: "Google", icon: "google" },
  qwen: { name: "Qwen", icon: "qwen" },
  mistralai: { name: "Mistral AI", icon: "mistralai" },
  "z-ai": { name: "Z.ai" },
  deepseek: { name: "DeepSeek", icon: "deepseek" },
  meta: { name: "Meta", icon: "meta" },
  "meta-llama": { name: "Meta Llama", icon: "meta" },
  moonshotai: { name: "Moonshot AI", icon: "moonshotai" },
  nvidia: { name: "NVIDIA", icon: "nvidia" },
  "x-ai": { name: "xAI" },
  minimax: { name: "MiniMax" },
  cohere: { name: "Cohere" },
  amazon: { name: "Amazon" },
  microsoft: { name: "Microsoft" },
  perplexity: { name: "Perplexity", icon: "perplexity" },
};

export function vendorName(vendor: string): string {
  return VENDORS[vendor]?.name ?? vendor;
}

/** The one-letter mark for a vendor without a logo. */
export function vendorMark(vendor: string): string {
  return (vendor.match(/[a-z0-9]/i)?.[0] ?? "?").toUpperCase();
}

export type Rail = {
  /** The vendors with the most catalog models, largest first. */
  vendors: string[];
  /** Whether any model falls outside those vendors. */
  other: boolean;
};

/** The top vendors by model count (ties by slug), then Other for the rest. */
export function vendorRail(
  models: readonly Pick<CatalogModel, "id">[],
  limit = TOP_VENDORS,
): Rail {
  const counts = new Map<string, number>();
  let unvendored = 0;
  for (const model of models) {
    const vendor = vendorOf(model.id);
    if (vendor) counts.set(vendor, (counts.get(vendor) ?? 0) + 1);
    else unvendored++;
  }
  const ranked = [...counts].sort(
    ([a, x], [b, y]) => y - x || (a < b ? -1 : a > b ? 1 : 0),
  );
  return {
    vendors: ranked.slice(0, limit).map(([vendor]) => vendor),
    other: unvendored > 0 || ranked.length > limit,
  };
}

/** Every tab in rail order: ★, the top vendors, then Other when needed. */
export function railTabs(rail: Rail): string[] {
  return [FAVORITES_TAB, ...rail.vendors, ...(rail.other ? [OTHER_TAB] : [])];
}

/** The models one rail tab lists, in catalog order. */
export function modelsForTab<T extends Pick<CatalogModel, "id">>(
  models: readonly T[],
  tab: string,
  rail: Rail,
  favorites: readonly string[],
): T[] {
  if (tab === FAVORITES_TAB) {
    const wanted = new Set(favorites);
    return models.filter((model) => wanted.has(model.id));
  }
  if (tab === OTHER_TAB) {
    const top = new Set(rail.vendors);
    return models.filter((model) => {
      const vendor = vendorOf(model.id);
      return !vendor || !top.has(vendor);
    });
  }
  return models.filter((model) => vendorOf(model.id) === tab);
}

/** Models whose name or slug holds every word of the query, any vendor. */
export function searchModels<T extends Pick<CatalogModel, "id" | "name">>(
  models: readonly T[],
  query: string,
): T[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...models];
  return models.filter((model) => {
    const text = `${model.name} ${model.id}`.toLowerCase();
    return words.every((word) => text.includes(word));
  });
}

/** The tab the picker opens on: ★ when a favorite is listed, else the largest vendor. */
export function initialTab(
  models: readonly Pick<CatalogModel, "id">[],
  rail: Rail,
  favorites: readonly string[],
): string {
  const wanted = new Set(favorites);
  if (models.some((model) => wanted.has(model.id))) return FAVORITES_TAB;
  return rail.vendors[0] ?? (rail.other ? OTHER_TAB : FAVORITES_TAB);
}

/** The server's reasoning efforts, weakest first, with their labels. */
export const EFFORTS = [
  ["none", "None"],
  ["minimal", "Minimal"],
  ["low", "Low"],
  ["medium", "Medium"],
  ["high", "High"],
  ["xhigh", "Extra High"],
  ["max", "Max"],
] as const;

export type Effort = (typeof EFFORTS)[number][0];

export function effortLabel(effort: string): string {
  return EFFORTS.find(([value]) => value === effort)?.[1] ?? effort;
}

/**
 * The efforts a model may be registered with, besides Default: the catalog's
 * supported efforts in canonical order, never None when reasoning is
 * mandatory, and none at all for a model without reasoning.
 */
export function effortOptions(
  reasoning: ReasoningOptions | null | undefined,
): Effort[] {
  if (!reasoning) return [];
  const supported = new Set(reasoning.supported_efforts ?? []);
  return EFFORTS.map(([value]) => value).filter(
    (effort) =>
      supported.has(effort) && !(reasoning.mandatory && effort === "none"),
  );
}

/** At most this many favorites are kept per Tenant and Subject. */
export const FAVORITES_LIMIT = 100;

/** The `localStorage` key for one Subject's favorites in one Tenant. */
export function favoritesKey(tenant?: string, subject?: string): string {
  const base = "aidash-model-favorites";
  if (!tenant) return base;
  const scoped = `${base}:${encodeURIComponent(tenant)}`;
  return subject ? `${scoped}:${encodeURIComponent(subject)}` : scoped;
}

/** Stored favorites, ignoring anything that is not a list of slugs. */
export function parseFavorites(raw: string | null | undefined): string[] {
  if (!raw) return [];
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(value)) return [];
  const slugs: string[] = [];
  for (const item of value) {
    if (typeof item !== "string") continue;
    const slug = item.trim();
    if (slug && slug.length <= 256 && !slugs.includes(slug)) slugs.push(slug);
    if (slugs.length === FAVORITES_LIMIT) break;
  }
  return slugs;
}

/** Adds or removes a slug; a new favorite comes first and the oldest beyond the limit drop. */
export function toggleFavorite(
  favorites: readonly string[],
  slug: string,
): string[] {
  if (favorites.includes(slug))
    return favorites.filter((item) => item !== slug);
  return [slug, ...favorites].slice(0, FAVORITES_LIMIT);
}

type StorageLike = Pick<Storage, "getItem" | "setItem">;

/** Reads favorites; storage that throws, as in some private modes, reads as empty. */
export function readFavorites(
  storage: StorageLike | undefined,
  key: string,
): string[] {
  try {
    return parseFavorites(storage?.getItem(key));
  } catch {
    return [];
  }
}

/** Writes favorites; returns whether the browser kept them. */
export function writeFavorites(
  storage: StorageLike | undefined,
  key: string,
  favorites: readonly string[],
): boolean {
  try {
    storage?.setItem(key, JSON.stringify(favorites.slice(0, FAVORITES_LIMIT)));
    return Boolean(storage);
  } catch {
    return false;
  }
}
