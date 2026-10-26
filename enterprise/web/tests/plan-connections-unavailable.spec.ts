// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

import { expect, test } from "@playwright/test";
import { installBearerDashboard } from "../../../web/tests/auth-fixture";
import {
  routeTenantActions,
  routeRegistryVersions,
} from "../../../web/tests/registry-fixture";

test("hosted bundles omit personal Plan entrypoints and never call Plan endpoints", async ({
  page,
}) => {
  await installBearerDashboard(page, "fixture", {
    tenant: "acme",
    name: "alice",
  });
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  const planRequests: string[] = [];
  const access = { kind: "subject", tenant: "acme", subject: "alice" };
  await page.route("**/api/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.includes("/plan-connections/")) {
      planRequests.push(path);
      return route.fulfill({ status: 404, json: { error: "Unavailable" } });
    }
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: "aidash://test" } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          deployment: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            key: null,
          },
          registry: [
            {
              id: "tenant-model",
              config: {
                provider: "openrouter",
                provider_credential: "openrouter",
              },
            },
            {
              id: "plan-model",
              config: { provider: "chatgpt", plan_connection: "chatgpt" },
            },
          ].map((entry) => ({
            ...entry,
            kind: "model",
            version: "1.0.0",
            name: { en: entry.id },
            description: { en: entry.id },
            capabilities: [],
            languages: [],
            tags: [],
            skills: [],
            schema: {},
          })),
          workspaces: [],
          tasks: [],
          artifacts: [],
          events: [],
          runs: [],
          conversations: [],
          human_requests: [],
          installations: [],
        },
      });
    return route.fulfill({ json: [] });
  });
  await routeTenantActions(page, ["registry.create"], {
    deployment: "aidash://test",
  });
  await routeRegistryVersions(page, () => []);
  await page.goto("/settings?view=planConnections");
  await expect(
    page.getByText(
      "ChatGPT Plan Connections are unavailable on this deployment.",
    ),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Plan Connections", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Connect ChatGPT", exact: true }),
  ).toHaveCount(0);
  await expect(page.getByRole("alert")).toHaveCount(0);
  await page.goto("/settings?view=registry");
  await page
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog
      .getByLabel("Provider", { exact: true })
      .locator('option[value="plan_connection:chatgpt"]'),
  ).toHaveCount(0);
  await expect(
    dialog
      .getByLabel("Provider", { exact: true })
      .locator('option[value="provider_credential:openrouter"]'),
  ).toHaveCount(1);
  await dialog.getByLabel("Entity type", { exact: true }).selectOption("model");
  await expect(
    dialog
      .getByLabel("Provider", { exact: true })
      .locator('option[value="chatgpt"]'),
  ).toHaveCount(0);
  expect(planRequests).toEqual([]);
});
