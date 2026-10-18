import { useQuery } from "@tanstack/react-query";
import { apiFetch } from "./transport";
import type { State } from "./types";

/** `GET /api/tenants/{tenant}/administration`: the actions the caller holds
 *  in its own Tenant, as the server computes them. */
export type TenantAdministration = {
  tenant: string;
  label: string | null;
  actions: string[];
  /** Listed only for a Tenant Administrator. */
  assignable_groups: string[];
  policy_revision: number | null;
};

/** Registry actions the overview lists beside the administration actions.
 *  Ordinary members hold some of them, so they alone confer no administration. */
export const REGISTRY_ACTIONS: readonly string[] = [
  "registry.create",
  "catalog.approve",
  "marketplace.install",
];

export function useTenantAdministration(tenant: string | null) {
  return useQuery({
    queryKey: ["tenant-administration", tenant],
    queryFn: () =>
      apiFetch<TenantAdministration>(
        `/api/tenants/${encodeURIComponent(tenant!)}/administration`,
      ),
    enabled: tenant !== null,
  });
}

/** What the signed-in subject may do in its Tenant. A Deployment
 *  Administrator, or a subject whose overview is unavailable, holds nothing;
 *  the server still refuses any action a hidden control would have sent. */
export function useTenantActions(access: State["access"]) {
  const tenant = access.kind === "subject" ? access.tenant : null;
  const overview = useTenantAdministration(tenant);
  const data = overview.isError ? undefined : overview.data;
  const actions = Array.isArray(data?.actions) ? data.actions : [];
  return {
    tenant,
    can: (action: string) => actions.includes(action),
    assignable: Array.isArray(data?.assignable_groups)
      ? data.assignable_groups
      : [],
  };
}
