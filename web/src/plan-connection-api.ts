import type {
  PlanAuthorizationCompleted,
  PlanAuthorizationStarted,
  PlanConnectionStatus,
  PlanDisconnection,
  PlanModels,
  RequestPlanAuthorizationComplete,
  RequestPlanAuthorizationStart,
  RequestPlanDisconnect,
} from "./generated/models";

/** Shared screens depend on canonical models, while each offering owns its API availability. */
export interface PlanConnectionApi {
  status(tenant: string, options?: RequestInit): Promise<PlanConnectionStatus>;
  start(
    tenant: string,
    input: RequestPlanAuthorizationStart,
    options?: RequestInit,
  ): Promise<PlanAuthorizationStarted>;
  complete(
    tenant: string,
    input: RequestPlanAuthorizationComplete,
    options?: RequestInit,
  ): Promise<PlanAuthorizationCompleted>;
  disconnect(
    tenant: string,
    input: RequestPlanDisconnect,
    options?: RequestInit,
  ): Promise<PlanDisconnection>;
  models(tenant: string, options?: RequestInit): Promise<PlanModels>;
}
