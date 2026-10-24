import type { PlanConnectionApi } from "../plan-connection-api";
import {
  planConnectionStatus,
  planConnectionDisconnect,
  planAuthorizationStart,
  planAuthorizationComplete,
  planConnectionModels,
} from "../generated/aidash";

/** Self-hosted alone exposes Plan endpoints; hosted builds replace this entire adapter (#248). */
export const selfHostedPlanConnections = true;
export const planConnectionApi: PlanConnectionApi = {
  status: (tenant, options) =>
    planConnectionStatus(encodeURIComponent(tenant), "chatgpt", options),
  start: (tenant, input, options) =>
    planAuthorizationStart(
      encodeURIComponent(tenant),
      "chatgpt",
      input,
      options,
    ),
  complete: (tenant, input, options) =>
    planAuthorizationComplete(
      encodeURIComponent(tenant),
      "chatgpt",
      input,
      options,
    ),
  disconnect: (tenant, input, options) =>
    planConnectionDisconnect(
      encodeURIComponent(tenant),
      "chatgpt",
      input,
      options,
    ),
  models: (tenant, options) =>
    planConnectionModels(encodeURIComponent(tenant), "chatgpt", options),
};
