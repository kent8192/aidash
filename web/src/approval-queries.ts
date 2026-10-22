import { useQuery } from "@tanstack/react-query";
import { approvalDecider, approvalPreferences } from "./generated/aidash";
import type { SessionResponse } from "./generated/models";

export function approvalPreferencesKey(tenant: string) {
  return ["approval-preferences", tenant] as const;
}

/** The caller's own default Approval Mode, Bypass acceptance and the modes
 *  their Tenant's policy and this Node let them use. */
export function useApprovalPreferences(tenant: string) {
  return useQuery({
    queryKey: approvalPreferencesKey(tenant),
    queryFn: ({ signal }) =>
      approvalPreferences(encodeURIComponent(tenant), { signal }),
    retry: false,
  });
}

export function approvalDeciderKey(tenant: string) {
  return ["approval-decider", tenant] as const;
}

/** The approval Decider that judges the Tenant's Auto mode and whether this
 *  Node can use it. `enabled` withholds the request until it matters. */
export function useApprovalDecider(tenant: string, enabled = true) {
  return useQuery({
    queryKey: approvalDeciderKey(tenant),
    queryFn: ({ signal }) =>
      approvalDecider(encodeURIComponent(tenant), { signal }),
    enabled,
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
