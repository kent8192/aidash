// Issue #1154. Tier order is a product decision, independent of price.
const series = [
  { prefix: "anthropic/claude-", tiers: ["fable", "opus", "sonnet", "haiku"] },
  { prefix: "openai/gpt-", tiers: ["astra", "sol", "terra", "luna"] },
  { prefix: "google/gemini-", tiers: ["neon", "carbon", "argon", "hydrogen"] },
  { prefix: "deepseek/", tiers: ["pro", "flash"] },
  { prefix: "moonshotai/kimi-", tiers: ["pro", "base"] },
  { prefix: "meta/muse-", tiers: ["wonder", "spark"] },
];

type Model = {
  id: string;
  created?: number;
  architecture?: { output_modalities: string[] };
};
const numeric = new Intl.Collator("en", { numeric: true });

function rank(id: string) {
  const index = series.findIndex((s) => id.startsWith(s.prefix));
  const rule = series[index];
  const slug = rule ? id.slice(rule.prefix.length) : id;
  const version =
    (index === 3
      ? slug.match(/(?:^|-)v(\d+(?:\.\d+)*)/)
      : slug.match(/(\d+(?:\.\d+)*)/))?.[1] ?? "0";
  let tier = rule?.tiers.findIndex((t) => slug.split("-").includes(t)) ?? -1;
  if (index === 4 && /^k\d+(?:\.\d+)*$/.test(slug)) tier = 1;
  if (index === 5 && !/^(wonder|spark-\d+(?:\.\d+)*)$/.test(slug)) tier = -1;
  return {
    index: index < 0 ? series.length : index,
    version,
    major: Number(version.split(".")[0]),
    tier: tier < 0 ? Infinity : tier,
  };
}

export function eligibleModel(model: Model) {
  const { id, architecture } = model;
  return (
    !/(?:^~|:(?:batch|free)$|-exp(?:-|:|$))/.test(id) &&
    !(id.startsWith("openai/gpt-") && /-pro(?:$|:)/.test(id)) &&
    !(
      id.startsWith("moonshotai/kimi-") &&
      !/^moonshotai\/kimi-k(?:[3-9]|\d{2,})(?:\.|-|$)/.test(id)
    ) &&
    (!architecture ||
      (architecture.output_modalities.length === 1 &&
        architecture.output_modalities[0] === "text"))
  );
}

export function compareModels(a: Model, b: Model) {
  const x = rank(a.id),
    y = rank(b.id);
  const versions = numeric.compare(y.version, x.version);
  return (
    x.index - y.index ||
    (x.index === series.length ? numeric.compare(a.id, b.id) : 0) ||
    Number(!Number.isFinite(x.tier)) - Number(!Number.isFinite(y.tier)) ||
    (x.index === 3 || x.index === 4
      ? versions
      : x.index === 5
        ? 0
        : y.major - x.major) ||
    x.tier - y.tier ||
    versions ||
    (b.created ?? 0) - (a.created ?? 0) ||
    numeric.compare(b.id, a.id)
  );
}

export function catalogSections<T extends Model>(models: T[]) {
  const all = models.filter(eligibleModel).sort(compareModels);
  const newest = new Map<number, number>();
  for (const model of all) {
    const { index, major } = rank(model.id);
    newest.set(index, Math.max(newest.get(index) ?? 0, major));
  }
  const seen = new Set<string>();
  const shortcuts = all.filter((model) => {
    const { index, major, tier } = rank(model.id);
    if (!Number.isFinite(tier) || (index !== 5 && major !== newest.get(index)))
      return false;
    const key = index + "/" + tier;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  return { all, shortcuts };
}
