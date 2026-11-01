import { connectionStart } from "../generated/aidash";
import type { ConnectionToolReference } from "../generated/models";
import { authorizePopup } from "./popup";

/** Only the Aidash server receives the authorization code and tokens. */
export function connect({
  tenant,
  resource,
  scopes = [],
  tool,
}: {
  tenant: string;
  resource: string;
  scopes?: string[];
  tool?: ConnectionToolReference;
}) {
  return authorizePopup(async () => {
    const result = await connectionStart(encodeURIComponent(tenant), {
      resource,
      scopes,
      tool,
      return_to: `${window.location.pathname}${window.location.search}${window.location.hash}`,
    });
    return result.authorization_url;
  });
}
