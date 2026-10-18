import type { ReactNode } from "react";
import { TenantPolicyAdministration } from "./authorization";
import { TenantIdentityAdministration } from "./dashboard-identity";
import { OwnCredentials } from "./subject-credentials";
import { useTenantAdministration } from "./tenant-administration";

/** What a subject manages of its own Tenant through its Mapping: its own
 *  Subject Credentials, then the people, policy, credentials and Peer
 *  Mappings its Tenant's policy lets it administer (#363). */
export function TenantAccess({
  tenant,
  subject,
  fallback,
}: {
  tenant: string;
  subject: string | null;
  fallback: ReactNode;
}) {
  const overview = useTenantAdministration(tenant);
  const actions = overview.isError ? [] : (overview.data?.actions ?? []);
  const can = (action: string) => actions.includes(action);
  return (
    <>
      {subject && overview.data && (
        <OwnCredentials
          tenant={tenant}
          subject={subject}
          canIssue={can("credential.create")}
        />
      )}
      <TenantIdentityAdministration
        key={tenant}
        tenant={tenant}
        fallback={subject ? null : fallback}
      />
      {can("policy.read") && (
        <TenantPolicyAdministration tenant={tenant} can={can} />
      )}
    </>
  );
}
