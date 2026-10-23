import { TenantSecrets } from "./tenant-secrets";
import type { ReactNode } from "react";
import { ApprovalDeciderPanel } from "./approval-decider";
import { TenantPolicyAdministration } from "./authorization";
import { TenantIdentityAdministration } from "./dashboard-identity";
import { OwnCredentials } from "./subject-credentials";
import { LeaveTenantButton } from "./tenant-deletion";
import { useTenantAdministration } from "./tenant-administration";

/** What a subject manages of its own Tenant through its Mapping: its own
 *  Subject Credentials, the approval Decider that judges its Tenant's Auto
 *  mode, then the people, policy, credentials and Peer Mappings its Tenant's
 *  policy lets it administer (#363). */
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
          key={tenant}
          tenant={tenant}
          subject={subject}
          canIssue={can("credential.create")}
        />
      )}
      {subject && overview.data && (
        <ApprovalDeciderPanel
          tenant={tenant}
          subject={subject}
          canUpdate={can("tenant.update")}
        />
      )}
      <TenantIdentityAdministration
        key={tenant}
        tenant={tenant}
        // A member without administration actions still leaves from here.
        fallback={subject ? <LeaveTenantButton tenant={tenant} /> : fallback}
      />
      {(can("secret.manage") || can("secret.use")) && (
        <TenantSecrets
          key={tenant}
          tenant={tenant}
          canManage={can("secret.manage")}
        />
      )}
      {([
        "policy.read",
        "credential.read",
        "credential.issue",
        "peer_mapping.read",
        "peer_mapping.update",
        "decision.read",
      ].some(can) ||
        !!overview.data?.issuable_subjects?.length) && (
        <TenantPolicyAdministration
          key={tenant}
          tenant={tenant}
          can={can}
          issuableSubjects={overview.data?.issuable_subjects ?? []}
        />
      )}
    </>
  );
}
