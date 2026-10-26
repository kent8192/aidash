// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

import type { PlanConnectionApi } from "@/plan-connection-api";

/** Cloud and Dedicated have no Plan endpoints or generated endpoint functions (#248). */
export const selfHostedPlanConnections = false;
async function unavailable(): Promise<never> {
  throw new Error(
    "ChatGPT Plan Connections are unavailable on this deployment.",
  );
}
export const planConnectionApi: PlanConnectionApi = {
  status: unavailable,
  start: unavailable,
  complete: unavailable,
  disconnect: unavailable,
  models: unavailable,
};
