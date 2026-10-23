import { useQuery } from "@tanstack/react-query";
import { approvalDecider, approvalPreferences } from "./generated/aidash";
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

export function approvalDeciderKey(tenant: string, node?: string) {
  return node === undefined
    ? (["approval-decider", tenant] as const)
    : (["approval-decider", tenant, node] as const);
}

/** The approval Decider that judges the Tenant's Auto mode and whether this
 *  Node can use it. `enabled` withholds the request until it matters. */
export function useApprovalDecider(tenant: string, enabled = true) {
  const node = useNodes().selected?.name;
  return useQuery({
    queryKey: approvalDeciderKey(tenant, node),
    queryFn: ({ signal }) =>
      approvalDecider(encodeURIComponent(tenant), { node: node! }, { signal }),
    enabled: enabled && node !== undefined,
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
