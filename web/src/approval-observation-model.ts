/** Non-content HTTP projections only. Never add state, arguments or frame references. */
export type ObserverReference = {
  id: string;
  version: string;
  registry_deployment?: string;
};
export type ObserverSetting = {
  revision: number;
  decider: ObserverReference | null;
  node: string;
};
export type ObservationKind =
  | "tool_call"
  | "outbound"
  | "web_search"
  | "page_reading"
  | "generation";
export type ObservationVerdict = "allowed" | { blocked: string };
export type ObservationProjection = {
  kind: ObservationKind;
  settings_revision: number;
  decider: ObserverReference;
  configuration_digest: string;
  definition_digest: string;
  outcome:
    | { status: "pending" }
    | { status: "available"; verdict: ObservationVerdict }
    | { status: "unavailable"; reason: string };
  routes: {
    route: number;
    provider_contract: string;
    model: string;
    hosts: string[];
    effective_threshold: string;
    answers: Record<string, string>;
  }[];
  human_label: "allow" | "deny" | null;
};
export type ObservationRate = {
  numerator: number;
  denominator: number;
  value: number | null;
};
export type ObservationMetrics = {
  total_approvals: number;
  available_observations: number;
  explicit_labels: number;
  comparable: number;
  confusion: {
    shadow_allow_human_allow: number;
    shadow_allow_human_deny: number;
    shadow_block_human_allow: number;
    shadow_block_human_deny: number;
  };
  agreement: ObservationRate;
  false_allow_among_human_denials: ObservationRate;
  false_allow_among_shadow_allows: ObservationRate;
  false_block_among_human_approvals: ObservationRate;
  availability: ObservationRate;
  label_coverage: ObservationRate;
  comparable_coverage: ObservationRate;
  exclusions: Record<
    | "rule"
    | "auto"
    | "bypass"
    | "expired"
    | "cancelled"
    | "revoked"
    | "pending"
    | "missing_observations"
    | "unavailable_observations"
    | "pending_observations",
    number
  >;
  risk_bands: {
    lower_inclusive: number;
    upper: number;
    upper_inclusive: boolean;
    observations: number;
    explicit_labels: number;
    human_denials: number;
    empirical_human_denial_rate: ObservationRate;
  }[];
};
export type ObservationReport = {
  range: { start: string; end: string };
  metrics: ObservationMetrics;
  groups: {
    group: {
      decider: ObserverReference;
      configuration_digest: string;
      definition_digest: string;
      settings_revision: number;
      kind: ObservationKind;
      operation_class: string | null;
      effect_class: string | null;
      route: number | null;
      provider_contract: string | null;
      model: string | null;
      hosts: string[];
      effective_threshold: string;
      served_models: string[];
      served_hosts: string[];
      includes_unknown_model: boolean;
      includes_unknown_host: boolean;
    };
    metrics: ObservationMetrics;
  }[];
};
/** Domain Probability uses exact binary64 hexadecimal serialization. Invalid or
 * unavailable evidence is N/A, never a fabricated zero. */
export function observationProbability(bits: string): number | null {
  if (!/^[0-9a-f]{16}$/.test(bits)) return null;
  const view = new DataView(new ArrayBuffer(8));
  view.setBigUint64(0, BigInt(`0x${bits}`));
  const value = view.getFloat64(0);
  return Number.isFinite(value) && value >= 0 && value <= 1 ? value : null;
}
export function observationRate(rate: ObservationRate, locale: string): string {
  if (
    rate.denominator === 0 ||
    rate.value === null ||
    !Number.isFinite(rate.value)
  )
    return `N/A (${rate.numerator} / ${rate.denominator})`;
  return `${new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 1 }).format(rate.value)} (${rate.numerator} / ${rate.denominator})`;
}
export function observationRange(
  start: string,
  end: string,
): { start: string; end: string } {
  const from = new Date(`${start}T00:00:00Z`);
  const to = new Date(`${end}T00:00:00Z`);
  const duration = to.getTime() - from.getTime();
  if (
    !/^\d{4}-\d{2}-\d{2}$/.test(start) ||
    !/^\d{4}-\d{2}-\d{2}$/.test(end) ||
    !Number.isFinite(duration) ||
    duration <= 0 ||
    duration > 31 * 86400000 ||
    from.toISOString().slice(0, 10) !== start ||
    to.toISOString().slice(0, 10) !== end
  )
    throw new Error("Choose a positive UTC interval of at most 31 days");
  return { start: from.toISOString(), end: to.toISOString() };
}
