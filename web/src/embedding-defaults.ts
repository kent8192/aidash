import { useQuery } from "@tanstack/react-query";
import { openrouterModels } from "./generated/aidash";
import { apiFetch } from "./transport";

export type DataProtectionLevel =
  | "no_restriction"
  | "no_training"
  | "safety_retention"
  | "zero_data_retention";
type ZdrRoutes = "strict" | "safety_retention_only" | "none";

/**
 * The prefilled OpenRouter embedding model for a level (#963). Gemini has only
 * Safety Retention routes, so Zero Data Retention starts with Titan, the one
 * text-and-image embedding model on a strict route. Aidash ships no
 * Deployment-hosted embedding model yet (ADR 0036), so none is offered here.
 */
export function defaultEmbedding(level: DataProtectionLevel | undefined): {
  model: string;
  dimensions: number;
} {
  return level === "zero_data_retention"
    ? { model: "amazon/titan-embed-image-v1", dimensions: 1024 }
    : { model: "google/gemini-embedding-2", dimensions: 3072 };
}

/** The Tenant's effective level; only Tenant Administrators can read it. */
export function useTenantLevel(
  tenant: string | undefined,
): DataProtectionLevel | undefined {
  const query = useQuery({
    queryKey: ["data-protection", tenant],
    enabled: Boolean(tenant),
    queryFn: () =>
      apiFetch<{ effective: { level: DataProtectionLevel } }>(
        `/api/tenants/${encodeURIComponent(tenant ?? "")}/data-protection`,
      ),
    retry: false,
  });
  return query.data?.effective.level;
}

/** What OpenRouter's ZDR list offered each model, from the shared catalog. */
export function useZdrRoutes(): Record<string, ZdrRoutes> {
  const catalog = useQuery({
    queryKey: ["openrouter-models"],
    queryFn: () => openrouterModels(),
    staleTime: 5 * 60 * 1000,
    retry: false,
  });
  return (
    (catalog.data as { zero_data_retention?: Record<string, ZdrRoutes> })
      ?.zero_data_retention ?? {}
  );
}

/** Why an OpenRouter embedding model cannot meet `level`, or null. */
export function embeddingRefusal(
  model: string,
  level: DataProtectionLevel | undefined,
  routes: Record<string, ZdrRoutes>,
  ja: boolean,
): string | null {
  if (
    !level ||
    level === "no_restriction" ||
    level === "no_training" ||
    Object.keys(routes).length === 0
  )
    return null;
  const offered = routes[model.trim()] ?? "none";
  if (level === "zero_data_retention" && offered === "safety_retention_only")
    return ja
      ? `${model} は Safety Retention の経路しかないため、Zero Data Retention では埋め込みに使えません。`
      : `${model} has only Safety Retention routes, so it cannot embed at Zero Data Retention.`;
  if (offered === "none")
    return ja
      ? `${model} にはゼロデータ保持の経路がないため、このデータ保護レベルでは埋め込みに使えません。`
      : `${model} has no zero data retention route, so it cannot embed at this Data Protection Level.`;
  return null;
}
