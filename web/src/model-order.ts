// Issue #1154. Tier order is a product decision, independent of price.
const series = [
  {
    name: "Claude",
    prefix: /^anthropic\/claude-/,
    tiers: ["fable", "opus", "sonnet", "haiku"],
  },
  {
    name: "ChatGPT",
    prefix: /^openai\/gpt-/,
    tiers: ["astra", "sol", "terra", "luna"],
  },
  {
    name: "Gemini",
    prefix: /^google\/gemini-(?=\d)/,
    tiers: ["neon", "carbon", "argon", "hydrogen"],
  },
  { name: "DeepSeek", prefix: /^deepseek\//, tiers: ["pro", "flash"] },
  { name: "Kimi", prefix: /^moonshotai\/kimi-/, tiers: [] }, // K3+, by version and release date
  { name: "Muse", prefix: /^meta\/muse-/, tiers: ["wonder", "spark"] },
];

type Model = {
  id: string;
  created?: number | null;
  architecture?: { output_modalities: string[] };
};
const numeric = new Intl.Collator("en", { numeric: true });

function rank(id: string) {
  const index = series.findIndex((rule) => rule.prefix.test(id));
  const rule = series[index];
  const name = rule?.name;
  const slug = rule ? id.replace(rule.prefix, "") : id;
  const version =
    (name === "DeepSeek"
      ? slug.match(/(?:^|-)v(\d+(?:\.\d+)*)/)
      : slug.match(/(\d+(?:\.\d+)*)/))?.[1] ?? "0";
  const tierIndex =
    rule?.tiers.findIndex((t) => slug.split("-").includes(t)) ?? -1;
  let shortcut = tierIndex >= 0 ? rule.tiers[tierIndex] : undefined;
  if (name === "Kimi")
    shortcut = slug.replace(/^k\d+(?:\.\d+)*-?/, "") || "base";
  if (name === "Muse" && !/^(wonder|spark-\d+(?:\.\d+)*)$/.test(slug))
    shortcut = undefined;
  return {
    index: index < 0 ? series.length : index,
    name,
    version,
    major: Number(version.split(".")[0]),
    tierIndex,
    shortcut,
  };
}

export function eligibleModel(model: Model) {
  const { id, architecture } = model;
  return (
    !/(?:^~|:(?:batch|free)$|-exp(?:-|:|$))/.test(id) &&
    !(id.startsWith("openai/gpt-") && /-pro(?:$|:|-)/.test(id)) &&
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
  const x = rank(a.id);
  const y = rank(b.id);
  if (x.index !== y.index) return x.index - y.index;
  if (!x.name) return numeric.compare(a.id, b.id);
  const versions = numeric.compare(y.version, x.version);
  const releases =
    (b.created ?? 0) - (a.created ?? 0) || numeric.compare(b.id, a.id);
  if (x.name === "Kimi") return versions || releases;
  if (!!x.shortcut !== !!y.shortcut) return x.shortcut ? -1 : 1;
  const generation =
    x.name === "DeepSeek"
      ? versions
      : x.name === "Muse"
        ? 0
        : y.major - x.major;
  return generation || x.tierIndex - y.tierIndex || versions || releases;
}

export function catalogSections<T extends Model>(models: T[]) {
  const all = models.filter(eligibleModel).sort(compareModels);
  const newest = new Map<number, number>();
  for (const model of all) {
    const { index, major, shortcut } = rank(model.id);
    // Only tiered models define a generation: `gpt-oss-120b` is not GPT-120.
    if (!shortcut) continue;
    newest.set(index, Math.max(newest.get(index) ?? 0, major));
  }
  const seen = new Set<string>();
  const shortcuts = all.filter((model) => {
    const { index, major, shortcut, name } = rank(model.id);
    if (!shortcut || (name !== "Muse" && major !== newest.get(index)))
      return false;
    const key = index + "/" + shortcut;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
  return { all, shortcuts };
}
