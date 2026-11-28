import { useQuery } from "@tanstack/react-query";
import { approvalPreferences } from "./generated/aidash";
import type { SessionResponse } from "./generated/models";
import { useNodes } from "./nodes";

export function approvalPreferencesKey(tenant: string, node?: string) {
  return node === undefined
    ? (["approval-preferences", tenant] as const)
    : (["approval-preferences", tenant, node] as const);
}

/** The caller's own default Approval Mode, Bypass acceptance and the modes
 *  their Tenant's policy and this Node let them use. */
export function useApprovalPreferences(tenant: string) {
  const node = useNodes().selected?.name;
  return useQuery({
    queryKey: approvalPreferencesKey(tenant, node),
    queryFn: ({ signal }) =>
      approvalPreferences(
        encodeURIComponent(tenant),
        { node: node! },
        { signal },
      ),
    enabled: node !== undefined,
    retry: false,
  });
}

/** The signed-in subject, read from the session the dashboard already holds
 *  (it fetches none). Null until that session is known. */
export function useViewerSubject(): string | null {
  const session = useQuery<SessionResponse>({
    queryKey: ["session"],
    enabled: false,
  });
  const access = session.data?.access;
  return access?.kind === "subject" ? access.subject : null;
}

/** Tenant from the already authorized session; never guessed from a private ID. */
export function useViewerTenant(): string | null {
  const session = useQuery<SessionResponse>({
    queryKey: ["session"],
    enabled: false,
  });
  const access = session.data?.access;
  return access?.kind === "subject" ? access.tenant : null;
}
