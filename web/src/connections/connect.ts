import { connectionStart } from "../generated/aidash";
import type { ConnectionToolReference } from "../generated/models";
import { authorizePopup } from "./popup";

/** Only the Aidash server receives the authorization code and tokens. */
export function connect({
  tenant,
  resource,
  scratchSpace,
  issuer,
  scopes = [],
  tool,
  returnTo,
}: {
  tenant: string;
  resource: string;
  scratchSpace?: string;
  issuer?: string;
  scopes?: string[];
  tool?: ConnectionToolReference;
  returnTo?: string;
}) {
  return authorizePopup(async () => {
    const result = await connectionStart(encodeURIComponent(tenant), {
      resource,
      scratch_space: scratchSpace,
      issuer,
      scopes,
      tool,
      return_to:
        returnTo ??
        `${window.location.pathname}${window.location.search}${window.location.hash}`,
    });
    return result.authorization_url;
  });
}
